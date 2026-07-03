//! [`RemoteBackend`]: a [`CommanderBackend`] implemented over HTTP against a
//! `claude-commander-server`.
//!
//! Each trait method maps to one route on the server's `/api` surface (see the
//! table in the crate docs). Requests carry the bearer token (when configured)
//! and their failures classify into the shared [`BackendError`] categories via
//! [`crate::error`]. A background [`Poller`] drives the change-feed and the
//! connection state machine; see [`crate::poller`].

use std::any::Any;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use claude_commander_core::api::{
    AgentStatesSnapshot, BranchInfo, CreateOptions, CreateSessionOpts, DiffSide, NewComment,
    OperationStatus, PreviewData, PreviewTarget, ReviewSnapshot, SessionDetail, ToggleReviewed,
    WorkspaceSnapshot,
};
use claude_commander_core::backend::{
    AttachConnection, AttachKind, BResult, BackendCapabilities, BackendChangeFeed,
    BackendDescriptor, BackendError, BackendKind, CommanderBackend, ConnectionState,
};
use claude_commander_core::comment::{ApplyOutcome, Comment};
use claude_commander_core::session::{ProjectId, ScanResult, SessionId};
use claude_commander_protocol::ws::AttachKind as WsAttachKind;
use reqwest::{Client, RequestBuilder, Response, StatusCode, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;
use xxhash_rust::xxh3::Xxh3;

use crate::poller::{self, ConnectionFeed, PollConfig, Poller};
use crate::spec::{RemoteServerSpec, SecretString};

/// How long to wait for a TCP connection before treating the server as
/// unreachable. Kept short so the poller's backoff engages promptly.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Overall per-request ceiling (a slow branch-diff still fits comfortably).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The shared, cloneable core of a [`RemoteBackend`]: the HTTP client, the
/// resolved base URL, and the (redacted) bearer token. The poll task holds an
/// `Arc` of this — never of the `RemoteBackend` itself — so there's no cycle.
pub(crate) struct RemoteInner {
    name: String,
    client: Client,
    base: Url,
    token: Option<SecretString>,
}

impl RemoteInner {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// The `/ws/attach` WebSocket URL for this server (scheme mapped, path
    /// prefix preserved). Used by [`CommanderBackend::attach`].
    fn ws_attach_url(&self) -> String {
        crate::attach::ws_attach_url(self.base.as_str())
    }

    /// The raw bearer token, if configured. Crate-internal and only handed to
    /// the attach handshake's `auth` frame — never logged.
    fn token(&self) -> Option<&str> {
        self.token.as_ref().map(|t| t.expose())
    }

    /// Build a `/api/<segments…>` URL against the base. `path_segments_mut`
    /// percent-encodes each segment, so a free-form session query or a diff path
    /// is escaped safely.
    fn endpoint(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        {
            let mut path = url
                .path_segments_mut()
                .expect("a validated http(s) base URL is always a base");
            // Drop the trailing empty segment of a bare `http://host/` so we
            // don't produce `//api`, then append the API prefix + segments.
            path.pop_if_empty();
            path.push("api");
            path.extend(segments);
        }
        url
    }

    /// Attach the bearer header (when set) and send, mapping a transport failure
    /// (no response) to [`BackendError::Unavailable`]/`Protocol`.
    async fn send(&self, request: RequestBuilder) -> BResult<Response> {
        let request = match &self.token {
            Some(token) => request.bearer_auth(token.expose()),
            None => request,
        };
        request.send().await.map_err(|err| {
            tracing::debug!(server = %self.name, error = %err, "remote request failed in transport");
            crate::error::transport_error(err)
        })
    }

    /// Turn a non-success status into the matching [`BackendError`], reading the
    /// server's error body for the reason; pass a success response through.
    async fn check(&self, response: Response) -> BResult<Response> {
        let status = response.status();
        if status.is_success() {
            Ok(response)
        } else {
            Err(crate::error::status_error(
                status,
                crate::error::error_message(response).await,
            ))
        }
    }

    async fn get_json<T: DeserializeOwned>(&self, url: Url) -> BResult<T> {
        let response = self.send(self.client.get(url)).await?;
        let response = self.check(response).await?;
        decode_json(response).await
    }

    /// GET where a `404` means "no such thing" rather than an error (the detail
    /// route resolves a free-form query and 404s when nothing matches).
    async fn get_json_opt<T: DeserializeOwned>(&self, url: Url) -> BResult<Option<T>> {
        let response = self.send(self.client.get(url)).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = self.check(response).await?;
        Ok(Some(decode_json(response).await?))
    }

    /// GET where a `204 No Content` means "unchanged" (the review-refresh route).
    async fn get_json_if_present<T: DeserializeOwned>(&self, url: Url) -> BResult<Option<T>> {
        let response = self.send(self.client.get(url)).await?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        let response = self.check(response).await?;
        Ok(Some(decode_json(response).await?))
    }

    async fn get_text(&self, url: Url) -> BResult<String> {
        let response = self.send(self.client.get(url)).await?;
        let response = self.check(response).await?;
        response.text().await.map_err(crate::error::body_error)
    }

    async fn get_bytes(&self, url: Url) -> BResult<Vec<u8>> {
        let response = self.send(self.client.get(url)).await?;
        let response = self.check(response).await?;
        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(crate::error::body_error)
    }

    /// POST with no body, decoding the JSON response (cascade routes → 202 +
    /// `OperationStatus`).
    async fn post_empty_json<T: DeserializeOwned>(&self, url: Url) -> BResult<T> {
        let response = self.send(self.client.post(url)).await?;
        let response = self.check(response).await?;
        decode_json(response).await
    }

    /// POST a JSON body, decoding the JSON response (create routes → 201 + id).
    async fn post_json<T: DeserializeOwned, B: Serialize>(&self, url: Url, body: &B) -> BResult<T> {
        let response = self.send(self.client.post(url).json(body)).await?;
        let response = self.check(response).await?;
        decode_json(response).await
    }

    /// POST with no body, discarding the (204/empty) response.
    async fn post_empty_ok(&self, url: Url) -> BResult<()> {
        let response = self.send(self.client.post(url)).await?;
        self.check(response).await?;
        Ok(())
    }

    /// PATCH a JSON body, discarding the (204) response.
    async fn patch_json_ok<B: Serialize>(&self, url: Url, body: &B) -> BResult<()> {
        let response = self.send(self.client.patch(url).json(body)).await?;
        self.check(response).await?;
        Ok(())
    }

    /// DELETE, discarding the (204) response.
    async fn delete_ok(&self, url: Url) -> BResult<()> {
        let response = self.send(self.client.delete(url)).await?;
        self.check(response).await?;
        Ok(())
    }

    /// Fetch the workspace + agent-state snapshots and content-hash them
    /// together. The poll loop compares this against the previous hash to decide
    /// whether observable state moved. Any HTTP/transport failure propagates so
    /// the poller can go [`ConnectionState::Degraded`].
    pub(crate) async fn poll_hashes(&self) -> BResult<u64> {
        let workspace = self.get_bytes(self.endpoint(&["workspace"])).await?;
        let agent_states = self.get_bytes(self.endpoint(&["agent-states"])).await?;
        let mut hasher = Xxh3::new();
        hasher.update(&workspace);
        hasher.update(b"\x00");
        hasher.update(&agent_states);
        Ok(hasher.digest())
    }
}

async fn decode_json<T: DeserializeOwned>(response: Response) -> BResult<T> {
    response.json::<T>().await.map_err(crate::error::body_error)
}

/// The server wraps created-resource ids as `{ "id": … }`.
#[derive(serde::Deserialize)]
struct IdEnvelope<T> {
    id: T,
}

/// `POST /sessions/{id}/files/reviewed` → `{ "reviewed": bool }`.
#[derive(serde::Deserialize)]
struct ReviewedBody {
    reviewed: bool,
}

/// `GET /projects/scan` → `{ added, skipped }` (core's `ScanResult` isn't
/// `Deserialize`, so we mirror the fields and rebuild it).
#[derive(serde::Deserialize)]
struct ScanBody {
    added: usize,
    skipped: usize,
}

fn diff_side_param(side: DiffSide) -> &'static str {
    match side {
        DiffSide::Old => "old",
        DiffSide::New => "new",
    }
}

/// A [`CommanderBackend`] that drives a remote `claude-commander-server` over
/// HTTP. Construct with [`RemoteBackend::new`]; the change-feed and connection
/// health are served by a background [`Poller`] spawned at construction and
/// aborted when the backend is dropped.
pub struct RemoteBackend {
    inner: Arc<RemoteInner>,
    poller: Poller,
}

impl RemoteBackend {
    /// Connect to the server described by `spec` with the default poll cadence.
    /// Fails only on a malformed `base_url` or an un-buildable HTTP client — the
    /// first *reachability* result surfaces through [`Self::connection_state`],
    /// not here, so the TUI can show a "connecting" server row immediately.
    pub fn new(spec: RemoteServerSpec) -> BResult<Self> {
        Self::with_config(spec, PollConfig::default())
    }

    /// Like [`Self::new`], with an explicit poll cadence + backoff (tests inject
    /// a fast interval; Phase G will wire this from config).
    pub fn with_config(spec: RemoteServerSpec, config: PollConfig) -> BResult<Self> {
        let base = Url::parse(&spec.base_url)
            .map_err(|e| BackendError::InvalidRequest(format!("invalid server url: {e}")))?;
        if base.cannot_be_a_base() || base.host().is_none() {
            return Err(BackendError::InvalidRequest(
                "server url must include a host (e.g. http://host:port)".to_string(),
            ));
        }
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| BackendError::Unavailable {
                reason: format!("could not build http client: {e}"),
            })?;
        let inner = Arc::new(RemoteInner {
            name: spec.name,
            client,
            base,
            token: spec.token,
        });
        let poller = poller::spawn(Arc::clone(&inner), config);
        Ok(Self { inner, poller })
    }

    /// The current connection health (cheap; no `.await`). The TUI reaches this
    /// via [`CommanderBackend::as_any`] downcast to render the server header.
    pub fn connection_state(&self) -> ConnectionState {
        self.poller.connection.borrow().clone()
    }

    /// A reactive watch on the connection health, mirroring the change-feed's
    /// shape, for the later TUI wiring that renders health as it changes.
    pub fn connection_feed(&self) -> ConnectionFeed {
        ConnectionFeed::new(self.poller.connection.clone())
    }

    fn session_url(&self, id: SessionId, tail: &[&str]) -> Url {
        let sid = id.as_uuid().to_string();
        let mut segments = Vec::with_capacity(2 + tail.len());
        segments.push("sessions");
        segments.push(&sid);
        segments.extend_from_slice(tail);
        self.inner.endpoint(&segments)
    }

    fn project_url(&self, id: ProjectId, tail: &[&str]) -> Url {
        let pid = id.as_uuid().to_string();
        let mut segments = Vec::with_capacity(2 + tail.len());
        segments.push("projects");
        segments.push(&pid);
        segments.extend_from_slice(tail);
        self.inner.endpoint(&segments)
    }
}

#[async_trait]
impl CommanderBackend for RemoteBackend {
    // -- Identity / capabilities --

    fn descriptor(&self) -> BackendDescriptor {
        BackendDescriptor {
            name: self.inner.name.clone(),
            kind: BackendKind::Remote,
        }
    }

    fn capabilities(&self) -> BackendCapabilities {
        // Every capability here is an operator-local affordance the server host
        // can't satisfy: opening the operator's editor, a `tmux display-popup`
        // switcher on the server, a dedicated commander tmux session, and the
        // in-session Ctrl+\ shell-toggle (which flips panes on the *local* tmux
        // server). Remote shell *attach* arrives in Phase F, but that's a
        // separate `AttachKind`, not this in-session toggle — so all four stay
        // honestly off.
        BackendCapabilities {
            open_editor: false,
            switcher_popup: false,
            commander_session: false,
            shell_toggle: false,
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn change_feed(&self) -> BackendChangeFeed {
        BackendChangeFeed::new(self.poller.generation.clone())
    }

    fn connection_watch(&self) -> Option<tokio::sync::watch::Receiver<ConnectionState>> {
        // Expose the poller's connection watch so the TUI renders this server's
        // health live (Connecting → Connected → Degraded) in its header.
        Some(self.poller.connection.clone())
    }

    // `startup_reconcile`, `reconcile_sections`, `reconcile_one_section`,
    // `record_feature`, `flush_telemetry`, `restart_session_fresh`, and
    // `apply_pr_results` all keep the trait defaults: the server reconciles and
    // records telemetry itself, and applying PR results is a local-only loop.

    /// Ask the server to re-check PR metadata (it runs the PR-status loop).
    async fn request_pr_refresh(&self) -> BResult<()> {
        self.inner
            .post_empty_ok(self.inner.endpoint(&["pr-refresh"]))
            .await
    }

    // -- Queries --

    async fn workspace_snapshot(&self) -> BResult<WorkspaceSnapshot> {
        self.inner
            .get_json(self.inner.endpoint(&["workspace"]))
            .await
    }

    async fn agent_states(&self, fresh: bool) -> BResult<AgentStatesSnapshot> {
        let mut url = self.inner.endpoint(&["agent-states"]);
        url.query_pairs_mut()
            .append_pair("fresh", if fresh { "true" } else { "false" });
        self.inner.get_json(url).await
    }

    async fn session_detail(
        &self,
        query: &str,
        lines: Option<usize>,
    ) -> BResult<Option<SessionDetail>> {
        let mut url = self.inner.endpoint(&["sessions", query, "detail"]);
        if let Some(lines) = lines {
            url.query_pairs_mut()
                .append_pair("lines", &lines.to_string());
        }
        self.inner.get_json_opt(url).await
    }

    async fn preview(&self, target: PreviewTarget) -> BResult<PreviewData> {
        let url = match target {
            PreviewTarget::Session { id, lines } => {
                let mut url = self.session_url(id, &["preview"]);
                if let Some(lines) = lines {
                    url.query_pairs_mut()
                        .append_pair("lines", &lines.to_string());
                }
                url
            }
            PreviewTarget::Project(id) => self.project_url(id, &["preview"]),
        };
        self.inner.get_json(url).await
    }

    async fn branch_diff(&self, id: SessionId) -> BResult<String> {
        self.inner
            .get_text(self.session_url(id, &["branch-diff"]))
            .await
    }

    async fn list_branches(&self, project: ProjectId, fetch: bool) -> BResult<Vec<BranchInfo>> {
        let mut url = self.project_url(project, &["branches"]);
        url.query_pairs_mut()
            .append_pair("fetch", if fetch { "true" } else { "false" });
        self.inner.get_json(url).await
    }

    async fn create_options(&self) -> BResult<CreateOptions> {
        self.inner
            .get_json(self.inner.endpoint(&["create-options"]))
            .await
    }

    async fn pending_comment_sessions(&self) -> BResult<Vec<SessionId>> {
        self.inner
            .get_json(self.inner.endpoint(&["comments", "pending"]))
            .await
    }

    // -- Session mutations --

    async fn create_session(&self, opts: CreateSessionOpts) -> BResult<SessionId> {
        let env: IdEnvelope<SessionId> = self
            .inner
            .post_json(self.inner.endpoint(&["sessions"]), &opts)
            .await?;
        Ok(env.id)
    }

    async fn kill_session(&self, id: SessionId) -> BResult<()> {
        self.inner
            .post_empty_ok(self.session_url(id, &["kill"]))
            .await
    }

    async fn restart_session(&self, id: SessionId) -> BResult<()> {
        self.inner
            .post_empty_ok(self.session_url(id, &["restart"]))
            .await
    }

    async fn delete_session(&self, id: SessionId) -> BResult<()> {
        self.inner.delete_ok(self.session_url(id, &[])).await
    }

    async fn rename_session(&self, id: SessionId, title: String) -> BResult<()> {
        let body = serde_json::json!({ "op": "rename", "title": title });
        self.inner
            .patch_json_ok(self.session_url(id, &[]), &body)
            .await
    }

    async fn set_section(&self, id: SessionId, section: Option<String>) -> BResult<()> {
        let body = serde_json::json!({ "op": "set_section", "section": section });
        self.inner
            .patch_json_ok(self.session_url(id, &[]), &body)
            .await
    }

    async fn mark_read(&self, id: SessionId) -> BResult<()> {
        self.inner
            .post_empty_ok(self.session_url(id, &["read"]))
            .await
    }

    async fn mark_unread(&self, _ids: Vec<SessionId>) -> BResult<()> {
        // The server exposes `POST /sessions/{id}/read` (mark read) but no
        // mark-unread route yet, so there's nothing honest to call. Surface a
        // toast rather than silently succeeding. (Phase G: add the route.)
        Err(BackendError::Unavailable {
            reason: "the remote server does not support marking sessions unread yet".to_string(),
        })
    }

    // -- Projects --

    async fn add_project(&self, path: PathBuf) -> BResult<ProjectId> {
        let body = serde_json::json!({ "path": path });
        let env: IdEnvelope<ProjectId> = self
            .inner
            .post_json(self.inner.endpoint(&["projects"]), &body)
            .await?;
        Ok(env.id)
    }

    async fn remove_project(&self, id: ProjectId) -> BResult<()> {
        self.inner.delete_ok(self.project_url(id, &[])).await
    }

    async fn scan_directory(&self, dir: PathBuf) -> BResult<ScanResult> {
        let mut url = self.inner.endpoint(&["projects", "scan"]);
        url.query_pairs_mut()
            .append_pair("dir", &dir.to_string_lossy());
        let body: ScanBody = self.inner.get_json(url).await?;
        Ok(ScanResult {
            added: body.added,
            skipped: body.skipped,
        })
    }

    // -- Cascade / push-stack --

    async fn cascade_merge(&self, id: SessionId) -> BResult<OperationStatus> {
        self.inner
            .post_empty_json(self.session_url(id, &["cascade"]))
            .await
    }

    async fn cascade_resume(&self) -> BResult<OperationStatus> {
        self.inner
            .post_empty_json(self.inner.endpoint(&["cascade", "resume"]))
            .await
    }

    async fn cascade_abandon(&self) -> BResult<()> {
        self.inner
            .post_empty_ok(self.inner.endpoint(&["cascade", "abandon"]))
            .await
    }

    async fn push_stack(&self, id: SessionId) -> BResult<OperationStatus> {
        self.inner
            .post_empty_json(self.session_url(id, &["push-stack"]))
            .await
    }

    // -- Review / comments --

    async fn list_comments(&self, id: SessionId) -> BResult<Vec<Comment>> {
        self.inner
            .get_json(self.session_url(id, &["comments"]))
            .await
    }

    async fn open_review(&self, id: SessionId) -> BResult<ReviewSnapshot> {
        self.inner.get_json(self.session_url(id, &["review"])).await
    }

    async fn refresh_review_if_changed(
        &self,
        id: SessionId,
        prev_hash: u64,
    ) -> BResult<Option<ReviewSnapshot>> {
        let mut url = self.session_url(id, &["review", "refresh"]);
        url.query_pairs_mut()
            .append_pair("prev_hash", &prev_hash.to_string());
        self.inner.get_json_if_present(url).await
    }

    async fn create_comment(&self, id: SessionId, draft: NewComment) -> BResult<Uuid> {
        let env: IdEnvelope<Uuid> = self
            .inner
            .post_json(self.session_url(id, &["comments"]), &draft)
            .await?;
        Ok(env.id)
    }

    async fn delete_comment(&self, id: SessionId, comment_id: Uuid) -> BResult<()> {
        let cid = comment_id.to_string();
        self.inner
            .delete_ok(self.session_url(id, &["comments", &cid]))
            .await
    }

    async fn apply_comments(&self, id: SessionId) -> BResult<ApplyOutcome> {
        self.inner
            .post_empty_json(self.session_url(id, &["comments", "apply"]))
            .await
    }

    async fn toggle_file_reviewed(&self, id: SessionId, display_path: String) -> BResult<bool> {
        let body = ToggleReviewed { display_path };
        let out: ReviewedBody = self
            .inner
            .post_json(self.session_url(id, &["files", "reviewed"]), &body)
            .await?;
        Ok(out.reviewed)
    }

    async fn fetch_diff_blob(
        &self,
        id: SessionId,
        side: DiffSide,
        path: String,
    ) -> BResult<Vec<u8>> {
        let mut url = self.session_url(id, &["blob"]);
        url.query_pairs_mut()
            .append_pair("side", diff_side_param(side))
            .append_pair("path", &path);
        self.inner.get_bytes(url).await
    }

    // -- Attach --

    async fn attach(
        &self,
        id: SessionId,
        cols: u16,
        rows: u16,
        kind: AttachKind,
    ) -> BResult<Box<dyn AttachConnection>> {
        // Map core's transport-neutral `AttachKind` onto the wire enum. The
        // server resolves the agent pane vs. the on-demand shell pane from this,
        // mirroring `LocalBackend::attach`.
        let pane = match kind {
            AttachKind::Agent => WsAttachKind::Agent,
            AttachKind::Shell => WsAttachKind::Shell,
        };
        crate::attach::connect(
            &self.inner.ws_attach_url(),
            self.inner.token(),
            id.as_uuid().to_string(),
            cols,
            rows,
            pane,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::Arc;

    use claude_commander_core::api::CommanderService;
    use claude_commander_core::backend::{AttachEnd, AttachStreams};
    use claude_commander_core::config::storage::AppState as CoreState;
    use claude_commander_core::config::{Config, ConfigStore, StateStore};
    use claude_commander_core::session::{Project, WorktreeSession};
    use claude_commander_core::telemetry::FrontendInfo;
    use claude_commander_core::tmux::TmuxExecutor;
    use claude_commander_server::{AppState, AuthConfig};
    use claude_commander_test_support::{
        create_test_repo, spawn_server, test_state, tmux_available,
    };
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::poller::PollConfig;

    /// A distinctive token that must never surface in an error or log line.
    const SECRET: &str = "TOKEN_DO_NOT_LEAK_9f3a1c";

    fn fast_config() -> PollConfig {
        PollConfig {
            interval: Duration::from_millis(40),
            ..PollConfig::default()
        }
    }

    /// A poll cadence long enough that the background loop stays out of the way
    /// while a test drives requests directly.
    fn idle_config() -> PollConfig {
        PollConfig {
            interval: Duration::from_secs(3600),
            ..PollConfig::default()
        }
    }

    fn spec(addr: SocketAddr, token: Option<&str>) -> RemoteServerSpec {
        RemoteServerSpec {
            name: "test-remote".to_string(),
            base_url: format!("http://{addr}"),
            token: token.map(SecretString::new),
        }
    }

    /// A hermetic server `AppState` with the given auth policy. Mirrors
    /// `test-support`'s safety knobs (telemetry off, tmux isolated) but lets us
    /// choose the auth policy so we can exercise the client's bearer header.
    fn state_with_auth(data_dir: &TempDir, worktrees_dir: &TempDir, auth: AuthConfig) -> AppState {
        let tmux_tmpdir = data_dir.path().join("tmux");
        std::fs::create_dir_all(&tmux_tmpdir).unwrap();
        let mut config = Config {
            worktrees_dir: Some(worktrees_dir.path().to_path_buf()),
            tmux_tmpdir: Some(tmux_tmpdir),
            ..Config::default()
        };
        config.telemetry.enabled = false;
        let config_store = Arc::new(ConfigStore::with_path(
            config,
            data_dir.path().join("config.toml"),
        ));
        let store = Arc::new(StateStore::with_path(
            CoreState::default(),
            data_dir.path().join("state.json"),
        ));
        let service = CommanderService::new(
            config_store,
            store,
            FrontendInfo::new("claude-commander-remote-test", "0.0.0"),
        );
        AppState::new(service, auth)
    }

    /// Boot a Disabled-auth server, returning its address plus a service clone
    /// (sharing the same store) so the test can seed/inspect state directly.
    async fn serve_disabled() -> (SocketAddr, CommanderService, TempDir, TempDir) {
        let data_dir = TempDir::new().unwrap();
        let worktrees_dir = TempDir::new().unwrap();
        let state = test_state(&data_dir, &worktrees_dir);
        let service = state.service.clone();
        let addr = spawn_server(state).await;
        (addr, service, data_dir, worktrees_dir)
    }

    /// A never-listening loopback address: bind to grab a free port, then drop
    /// the listener so a connect attempt is refused.
    async fn unused_addr() -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        addr
    }

    #[test]
    fn capabilities_are_all_local_only_off() {
        // Construction needs a runtime for the poll task; use a tiny one.
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let addr = unused_addr().await;
            let backend = RemoteBackend::with_config(spec(addr, None), idle_config()).unwrap();
            let caps = backend.capabilities();
            assert!(!caps.open_editor);
            assert!(!caps.switcher_popup);
            assert!(!caps.commander_session);
            assert!(!caps.shell_toggle);
            let d = backend.descriptor();
            assert_eq!(d.name, "test-remote");
            assert_eq!(d.kind, BackendKind::Remote);
        });
    }

    #[tokio::test]
    async fn workspace_snapshot_round_trips_seeded_state() {
        let (addr, service, _d, _w) = serve_disabled().await;
        let project = Project::new("repo", PathBuf::from("/tmp/repo"), "main");
        let pid = project.id;
        let session = WorktreeSession::new(
            pid,
            "task",
            "branch-task",
            PathBuf::from("/tmp/wt"),
            "claude",
        );
        let sid = session.id;
        service
            .store()
            .mutate(move |state| {
                state.add_project(project);
                state.add_session(session);
            })
            .await
            .unwrap();

        let backend = RemoteBackend::with_config(spec(addr, None), idle_config()).unwrap();
        let snap = backend.workspace_snapshot().await.unwrap();
        assert_eq!(snap.projects.len(), 1);
        assert_eq!(snap.projects[0].id, pid);
        assert_eq!(snap.sessions.len(), 1);
        assert_eq!(snap.sessions[0].session_id, sid);
    }

    #[tokio::test]
    async fn rename_is_visible_in_the_next_snapshot() {
        let (addr, service, _d, _w) = serve_disabled().await;
        let project = Project::new("repo", PathBuf::from("/tmp/repo"), "main");
        let pid = project.id;
        let session = WorktreeSession::new(
            pid,
            "task",
            "branch-task",
            PathBuf::from("/tmp/wt"),
            "claude",
        );
        let sid = session.id;
        service
            .store()
            .mutate(move |state| {
                state.add_project(project);
                state.add_session(session);
            })
            .await
            .unwrap();

        let backend = RemoteBackend::with_config(spec(addr, None), idle_config()).unwrap();
        backend
            .rename_session(sid, "renamed-over-http".to_string())
            .await
            .unwrap();

        let snap = backend.workspace_snapshot().await.unwrap();
        let s = snap
            .sessions
            .iter()
            .find(|s| s.session_id == sid)
            .expect("session present");
        assert_eq!(s.title, "renamed-over-http");
    }

    #[tokio::test]
    async fn wrong_token_is_auth_error() {
        let data_dir = TempDir::new().unwrap();
        let worktrees_dir = TempDir::new().unwrap();
        let state = state_with_auth(
            &data_dir,
            &worktrees_dir,
            AuthConfig::Token("the-real-token".to_string()),
        );
        let addr = spawn_server(state).await;

        let backend =
            RemoteBackend::with_config(spec(addr, Some("the-wrong-token")), idle_config()).unwrap();
        let err = backend.workspace_snapshot().await.unwrap_err();
        assert!(matches!(err, BackendError::Auth), "got {err:?}");
    }

    #[tokio::test]
    async fn correct_token_authorizes() {
        // Proves the client actually sends a usable `Authorization: Bearer …`.
        let data_dir = TempDir::new().unwrap();
        let worktrees_dir = TempDir::new().unwrap();
        let state = state_with_auth(
            &data_dir,
            &worktrees_dir,
            AuthConfig::Token("the-real-token".to_string()),
        );
        let addr = spawn_server(state).await;

        let backend =
            RemoteBackend::with_config(spec(addr, Some("the-real-token")), idle_config()).unwrap();
        let snap = backend.workspace_snapshot().await.unwrap();
        assert!(snap.projects.is_empty());
    }

    #[tokio::test]
    async fn connection_refused_is_unavailable() {
        let addr = unused_addr().await;
        let backend = RemoteBackend::with_config(spec(addr, None), idle_config()).unwrap();
        let err = backend.workspace_snapshot().await.unwrap_err();
        assert!(
            matches!(err, BackendError::Unavailable { .. }),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn unknown_session_rename_is_not_found() {
        let (addr, _service, _d, _w) = serve_disabled().await;
        let backend = RemoteBackend::with_config(spec(addr, None), idle_config()).unwrap();
        let err = backend
            .rename_session(SessionId::new(), "x".to_string())
            .await
            .unwrap_err();
        assert!(matches!(err, BackendError::NotFound), "got {err:?}");
    }

    #[tokio::test]
    async fn poller_bumps_change_feed_on_state_change() {
        let (addr, service, _d, _w) = serve_disabled().await;
        let backend = RemoteBackend::with_config(spec(addr, None), fast_config()).unwrap();
        let mut feed = backend.change_feed();

        // First successful poll (Connecting → Connected) bumps the feed so the
        // TUI fetches the initial snapshot.
        assert!(
            tokio::time::timeout(Duration::from_secs(3), feed.changed())
                .await
                .expect("first bump should arrive"),
            "sender should be alive"
        );

        // Mutating server-side state changes the snapshot hash → next poll bumps.
        service
            .store()
            .mutate(|state| {
                state.add_project(Project::new("repo", PathBuf::from("/tmp/repo"), "main"))
            })
            .await
            .unwrap();

        assert!(
            tokio::time::timeout(Duration::from_secs(3), feed.changed())
                .await
                .expect("a state change should bump the feed"),
            "sender should still be alive"
        );
    }

    #[tokio::test]
    async fn connection_state_reflects_reachability() {
        let (addr, _service, _d, _w) = serve_disabled().await;
        let backend = RemoteBackend::with_config(spec(addr, None), fast_config()).unwrap();
        let mut feed = backend.connection_feed();
        // Starts Connecting, then the first poll moves it to Connected.
        loop {
            if backend.connection_state() == ConnectionState::Connected {
                break;
            }
            tokio::time::timeout(Duration::from_secs(3), feed.changed())
                .await
                .expect("connection state should progress");
        }

        // A backend pointed at nothing goes Degraded.
        let dead = unused_addr().await;
        let bad = RemoteBackend::with_config(spec(dead, None), fast_config()).unwrap();
        let mut bad_feed = bad.connection_feed();
        loop {
            if matches!(bad.connection_state(), ConnectionState::Degraded { .. }) {
                break;
            }
            tokio::time::timeout(Duration::from_secs(3), bad_feed.changed())
                .await
                .expect("connection state should degrade");
        }
    }

    /// Force errors on several paths and assert the bearer token never appears
    /// in the error's `Display` or `Debug` (nor in the spec's `Debug`).
    #[tokio::test]
    async fn token_never_appears_in_errors() {
        let assert_clean = |err: &BackendError| {
            let display = format!("{err}");
            let debug = format!("{err:?}");
            assert!(
                !display.contains(SECRET),
                "token leaked in Display: {display}"
            );
            assert!(!debug.contains(SECRET), "token leaked in Debug: {debug}");
        };

        // Connection refused → Unavailable.
        let dead = unused_addr().await;
        let refused = RemoteBackend::with_config(spec(dead, Some(SECRET)), idle_config()).unwrap();
        assert_clean(&refused.workspace_snapshot().await.unwrap_err());

        // 401 from a token server we hold the wrong secret for → Auth.
        let data_dir = TempDir::new().unwrap();
        let worktrees_dir = TempDir::new().unwrap();
        let state = state_with_auth(
            &data_dir,
            &worktrees_dir,
            AuthConfig::Token("a-different-real-token".to_string()),
        );
        let addr = spawn_server(state).await;
        let authed = RemoteBackend::with_config(spec(addr, Some(SECRET)), idle_config()).unwrap();
        let auth_err = authed.workspace_snapshot().await.unwrap_err();
        assert!(matches!(auth_err, BackendError::Auth));
        assert_clean(&auth_err);

        // 404 → NotFound (with the token still attached to the request).
        let (ok_addr, _service, _d, _w) = serve_disabled().await;
        let ok = RemoteBackend::with_config(spec(ok_addr, Some(SECRET)), idle_config()).unwrap();
        let nf = ok
            .rename_session(SessionId::new(), "x".to_string())
            .await
            .unwrap_err();
        assert!(matches!(nf, BackendError::NotFound));
        assert_clean(&nf);

        // And the spec's own Debug is safe.
        let dbg = format!("{:?}", spec(ok_addr, Some(SECRET)));
        assert!(!dbg.contains(SECRET), "token leaked in spec Debug: {dbg}");

        // -- Attach failure paths carry the token only in the (never-logged)
        //    `auth` frame, so their errors must be clean too. --

        // Connect refused (WS transport never opens) → Unavailable.
        let dead_ws = unused_addr().await;
        let refused_ws =
            RemoteBackend::with_config(spec(dead_ws, Some(SECRET)), idle_config()).unwrap();
        // `Box<dyn AttachConnection>` isn't `Debug`, so match rather than `expect_err`.
        let refused_err = match refused_ws
            .attach(SessionId::new(), 80, 24, AttachKind::Agent)
            .await
        {
            Err(e) => e,
            Ok(_) => panic!("attach to a dead server must fail"),
        };
        assert!(matches!(refused_err, BackendError::Unavailable { .. }));
        assert_clean(&refused_err);

        // Auth rejection at the handshake → Auth (no session/tmux needed).
        let auth_data = TempDir::new().unwrap();
        let auth_wt = TempDir::new().unwrap();
        let auth_state = state_with_auth(
            &auth_data,
            &auth_wt,
            AuthConfig::Token("a-different-real-token".to_string()),
        );
        let auth_addr = spawn_server(auth_state).await;
        let bad_auth =
            RemoteBackend::with_config(spec(auth_addr, Some(SECRET)), idle_config()).unwrap();
        let attach_auth_err = match bad_auth
            .attach(SessionId::new(), 80, 24, AttachKind::Agent)
            .await
        {
            Err(e) => e,
            Ok(_) => panic!("attach with a bad token must fail"),
        };
        assert!(matches!(attach_auth_err, BackendError::Auth));
        assert_clean(&attach_auth_err);
    }

    #[tokio::test]
    async fn attach_connect_refused_is_unavailable() {
        let addr = unused_addr().await;
        let backend = RemoteBackend::with_config(spec(addr, None), idle_config()).unwrap();
        // `Box<dyn AttachConnection>` isn't `Debug`, so match rather than unwrap.
        match backend
            .attach(SessionId::new(), 80, 24, AttachKind::Agent)
            .await
        {
            Err(BackendError::Unavailable { .. }) => {}
            Err(other) => panic!("expected Unavailable, got {other:?}"),
            Ok(_) => panic!("attach to a dead server must not succeed"),
        }
    }

    /// A WS attach with the wrong token is rejected at the `auth` handshake frame
    /// — before any session/tmux resolution — so this is hermetic without tmux.
    #[tokio::test]
    async fn attach_wrong_token_is_auth_error() {
        let data_dir = TempDir::new().unwrap();
        let worktrees_dir = TempDir::new().unwrap();
        let state = state_with_auth(
            &data_dir,
            &worktrees_dir,
            AuthConfig::Token("the-real-token".to_string()),
        );
        let addr = spawn_server(state).await;

        let backend =
            RemoteBackend::with_config(spec(addr, Some("the-wrong-token")), idle_config()).unwrap();
        match backend
            .attach(SessionId::new(), 80, 24, AttachKind::Agent)
            .await
        {
            Err(BackendError::Auth) => {}
            Err(other) => panic!("expected Auth, got {other:?}"),
            Ok(_) => panic!("attach with a bad token must not succeed"),
        }
    }

    /// tmux-gated end-to-end: register a project and create a session over HTTP,
    /// then see it in the workspace snapshot. Self-skips without tmux (never
    /// `#[ignore]`), matching the server's integration-test convention.
    #[tokio::test]
    async fn create_session_round_trip_tmux() {
        if !tmux_available().await {
            eprintln!("Skipping test: tmux not available");
            return;
        }
        let (repo_temp_dir, repo_path) = create_test_repo().await;
        let data_dir = TempDir::new().unwrap();
        let worktrees_dir = TempDir::new().unwrap();
        let state = test_state(&data_dir, &worktrees_dir);
        let service = state.service.clone();
        let addr = spawn_server(state).await;

        let backend = RemoteBackend::with_config(spec(addr, None), idle_config()).unwrap();
        backend.add_project(repo_path.clone()).await.unwrap();
        let sid = backend
            .create_session(CreateSessionOpts {
                project_path: repo_path.clone(),
                title: "remote-e2e".to_string(),
                program: Some("bash".to_string()),
                initial_prompt: None,
                effort: None,
                mode: None,
                base_branch: None,
                section: None,
                stack_parent: None,
            })
            .await
            .unwrap();

        let snap = backend.workspace_snapshot().await.unwrap();
        assert!(
            snap.sessions.iter().any(|s| s.session_id == sid),
            "created session should appear in the snapshot"
        );

        // Clean up the tmux session so its throwaway server tears down.
        let _ = service.kill_session(&sid).await;
        drop(repo_temp_dir);
        drop(data_dir);
        drop(worktrees_dir);
    }

    /// Attaching to a session that doesn't exist yields the server's
    /// "no such session" error *before* `ready`, classified to `NotFound`. No
    /// tmux is spawned (the server resolves from its store), so this is hermetic.
    #[tokio::test]
    async fn attach_unknown_session_is_not_found() {
        let (addr, _service, _d, _w) = serve_disabled().await;
        let backend = RemoteBackend::with_config(spec(addr, None), idle_config()).unwrap();
        match backend
            .attach(SessionId::new(), 80, 24, AttachKind::Agent)
            .await
        {
            Err(BackendError::NotFound) => {}
            Err(other) => panic!("expected NotFound, got {other:?}"),
            Ok(_) => panic!("attach to an unknown session must not succeed"),
        }
    }

    /// Read from `reader` until its accumulated output contains `needle`, or the
    /// timeout elapses. Returns whether the needle was seen.
    async fn read_until_contains(
        reader: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
        needle: &str,
        timeout: Duration,
    ) -> bool {
        let fut = async {
            let mut acc = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => return false,
                    Ok(n) => {
                        acc.extend_from_slice(&buf[..n]);
                        if String::from_utf8_lossy(&acc).contains(needle) {
                            return true;
                        }
                    }
                    Err(_) => return false,
                }
            }
        };
        tokio::time::timeout(timeout, fut).await.unwrap_or(false)
    }

    /// tmux-gated end-to-end agent attach: create a session over HTTP, attach via
    /// the WebSocket, type a command and observe its echo in the PTY output,
    /// resize, then detach and assert the tmux session **survives** (detach ≠
    /// kill). Self-skips without tmux (never `#[ignore]`).
    #[tokio::test]
    async fn attach_agent_round_trip_tmux() {
        if !tmux_available().await {
            eprintln!("Skipping test: tmux not available");
            return;
        }
        let (repo_temp_dir, repo_path) = create_test_repo().await;
        let data_dir = TempDir::new().unwrap();
        let worktrees_dir = TempDir::new().unwrap();
        let state = test_state(&data_dir, &worktrees_dir);
        let service = state.service.clone();
        let addr = spawn_server(state).await;

        let backend = RemoteBackend::with_config(spec(addr, None), idle_config()).unwrap();
        backend.add_project(repo_path.clone()).await.unwrap();
        let sid = backend
            .create_session(CreateSessionOpts {
                project_path: repo_path.clone(),
                title: "remote-attach-agent".to_string(),
                program: Some("bash".to_string()),
                initial_prompt: None,
                effort: None,
                mode: None,
                base_branch: None,
                section: None,
                stack_parent: None,
            })
            .await
            .unwrap();

        let tmux_name = service
            .resolve_tmux_session(&sid.to_string())
            .await
            .unwrap()
            .expect("session should resolve to a tmux name");

        let conn = backend
            .attach(sid, 80, 24, AttachKind::Agent)
            .await
            .expect("remote agent attach should reach ready");
        let AttachStreams {
            mut reader,
            mut writer,
            resizer,
            mut terminator,
        } = conn.split();

        // Type a command; bash echoes the marker back through the PTY.
        writer.write_all(b"echo cc_remote_marker\n").await.unwrap();
        writer.flush().await.unwrap();
        assert!(
            read_until_contains(&mut *reader, "cc_remote_marker", Duration::from_secs(5)).await,
            "attached PTY output should echo the typed marker"
        );

        // A resize is fire-and-forget; just prove it doesn't disrupt the stream.
        resizer.resize(100, 30);

        // Detach: leaves the tmux session running.
        terminator.detach().await;
        assert_eq!(terminator.wait().await, AttachEnd::Detached);

        // The tmux session must survive the detach. Probe on the same isolated
        // socket dir the harness created it on.
        let tmux = TmuxExecutor::new().with_tmux_tmpdir(service.read_config().tmux_tmpdir);
        let mut exists = false;
        for _ in 0..50 {
            if tmux.session_exists(&tmux_name).await.unwrap_or(false) {
                exists = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(exists, "tmux session must survive a remote WS detach");

        service.kill_session(&sid).await.unwrap();
        drop(repo_temp_dir);
        drop(data_dir);
        drop(worktrees_dir);
    }

    /// tmux-gated shell-pane attach: the `kind: shell` attach frame drives the
    /// server to create the on-demand `-sh` pane and bring it to `ready`. After a
    /// detach the shell tmux session survives. Exercises the additive protocol
    /// `kind` field end-to-end.
    #[tokio::test]
    async fn attach_shell_round_trip_tmux() {
        if !tmux_available().await {
            eprintln!("Skipping test: tmux not available");
            return;
        }
        let (repo_temp_dir, repo_path) = create_test_repo().await;
        let data_dir = TempDir::new().unwrap();
        let worktrees_dir = TempDir::new().unwrap();
        let state = test_state(&data_dir, &worktrees_dir);
        let service = state.service.clone();
        let addr = spawn_server(state).await;

        let backend = RemoteBackend::with_config(spec(addr, None), idle_config()).unwrap();
        backend.add_project(repo_path.clone()).await.unwrap();
        let sid = backend
            .create_session(CreateSessionOpts {
                project_path: repo_path.clone(),
                title: "remote-attach-shell".to_string(),
                program: Some("bash".to_string()),
                initial_prompt: None,
                effort: None,
                mode: None,
                base_branch: None,
                section: None,
                stack_parent: None,
            })
            .await
            .unwrap();

        let conn = backend
            .attach(sid, 80, 24, AttachKind::Shell)
            .await
            .expect("remote shell attach should reach ready");
        let AttachStreams {
            reader,
            writer,
            resizer,
            mut terminator,
        } = conn.split();

        // The shell pane's tmux session is the agent name + `-sh`.
        let shell_name = service
            .resolve_shell_tmux_session(&sid.to_string())
            .await
            .unwrap()
            .expect("shell session should resolve/create");
        assert!(shell_name.ends_with("-sh"), "got {shell_name}");

        // Detach and confirm the shell session survives.
        drop(reader);
        drop(writer);
        drop(resizer);
        terminator.detach().await;
        assert_eq!(terminator.wait().await, AttachEnd::Detached);

        let tmux = TmuxExecutor::new().with_tmux_tmpdir(service.read_config().tmux_tmpdir);
        let mut exists = false;
        for _ in 0..50 {
            if tmux.session_exists(&shell_name).await.unwrap_or(false) {
                exists = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(exists, "shell tmux session must survive a remote WS detach");

        service.kill_session(&sid).await.unwrap();
        drop(repo_temp_dir);
        drop(data_dir);
        drop(worktrees_dir);
    }
}
