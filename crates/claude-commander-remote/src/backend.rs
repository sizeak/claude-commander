//! [`RemoteBackend`]: a [`CommanderBackend`] implemented as a thin adapter over
//! `claude-commander-client`'s transport.
//!
//! All the HTTP/WebSocket machinery — per-route calls, the change-feed poller,
//! the connection state machine, the attach pump — lives in the client crate,
//! which knows nothing of core. This adapter's job is purely to satisfy core's
//! [`CommanderBackend`] trait: each method delegates to the matching
//! [`RemoteClient`] method, maps the client's [`ClientError`](claude_commander_client::ClientError)
//! onto a [`BackendError`] via [`into_backend_error`], and rebuilds the handful
//! of core-only return types (`ScanResult`) that have no wire DTO. The attach
//! seam is bridged in [`crate::attach`].

use std::any::Any;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use claude_commander_client::{
    ConnectionFeed, PollConfig, Poller, RemoteClient, RemoteServerSpec, spawn_poller,
};
use claude_commander_core::api::{
    AgentStatesSnapshot, BranchInfo, CreateOptions, CreateSessionOpts, DiffSide, NewComment,
    OperationStatus, PreviewData, PreviewTarget, ProgramInfo, ReviewSnapshot, SessionDetail,
    WorkspaceSnapshot,
};
use claude_commander_core::backend::{
    AttachConnection, AttachKind, BResult, BackendCapabilities, BackendChangeFeed,
    BackendDescriptor, BackendKind, CommanderBackend, ConnectionState,
};
use claude_commander_core::comment::{ApplyOutcome, Comment};
use claude_commander_core::session::{ProjectId, ScanResult, SessionId};
use claude_commander_protocol::ws::AttachKind as WsAttachKind;
use uuid::Uuid;

use crate::attach::RemoteAttachConnection;
use crate::error::into_backend_error;

/// A [`CommanderBackend`] that drives a remote `claude-commander-server` over
/// HTTP + WebSocket. Construct with [`RemoteBackend::new`]; the change-feed and
/// connection health are served by a background [`Poller`] spawned at
/// construction and aborted when the backend is dropped.
///
/// The poll task holds the `Arc<RemoteClient>` — never the `RemoteBackend`
/// itself — so there's no reference cycle.
pub struct RemoteBackend {
    client: Arc<RemoteClient>,
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
    /// a fast interval; a frontend wires this from config).
    pub fn with_config(spec: RemoteServerSpec, config: PollConfig) -> BResult<Self> {
        let client = Arc::new(RemoteClient::new(spec).map_err(into_backend_error)?);
        let poller = spawn_poller(Arc::clone(&client), config);
        Ok(Self { client, poller })
    }

    /// The current connection health (cheap; no `.await`). The TUI reaches this
    /// via [`CommanderBackend::as_any`] downcast to render the server header.
    pub fn connection_state(&self) -> ConnectionState {
        self.poller.connection_state()
    }

    /// A reactive watch on the connection health, mirroring the change-feed's
    /// shape, for the TUI wiring that renders health as it changes.
    pub fn connection_feed(&self) -> ConnectionFeed {
        self.poller.connection_feed()
    }
}

#[async_trait]
impl CommanderBackend for RemoteBackend {
    // -- Identity / capabilities --

    fn descriptor(&self) -> BackendDescriptor {
        BackendDescriptor {
            name: self.client.name().to_string(),
            kind: BackendKind::Remote,
        }
    }

    fn capabilities(&self) -> BackendCapabilities {
        // Every capability here is an operator-local affordance the server host
        // can't satisfy: opening the operator's editor, a `tmux display-popup`
        // switcher on the server, a dedicated commander tmux session, and
        // *project* shells (a local tmux affordance keyed on a local project id).
        // Note `shell_toggle` gates only the project-shell path — the in-session
        // Ctrl+\ toggle to a session's own shell pane works fine over WS
        // (e2e-tested) via a separate `AttachKind::Shell`, independent of this
        // flag. The name reads broader than it acts; left unchanged this round.
        BackendCapabilities {
            open_editor: false,
            switcher_popup: false,
            commander_session: false,
            shell_toggle: false,
            // The agent runs on the server host: the operator's clipboard image
            // can't be read there, so the client captures it locally and uploads
            // via `paste_image`.
            client_side_image_paste: true,
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn change_feed(&self) -> BackendChangeFeed {
        BackendChangeFeed::new(self.poller.generation_watch())
    }

    fn connection_watch(&self) -> Option<tokio::sync::watch::Receiver<ConnectionState>> {
        // Expose the poller's connection watch so the TUI renders this server's
        // health live (Connecting → Connected → Degraded) in its header.
        Some(self.poller.connection_watch())
    }

    // `startup_reconcile`, `reconcile_sections`, `reconcile_one_section`,
    // `record_feature`, `flush_telemetry`, `restart_session_fresh`, and
    // `apply_pr_results` all keep the trait defaults: the server reconciles and
    // records telemetry itself, and applying PR results is a local-only loop.

    /// Ask the server to re-check PR metadata (it runs the PR-status loop).
    async fn request_pr_refresh(&self) -> BResult<()> {
        self.client
            .request_pr_refresh()
            .await
            .map_err(into_backend_error)
    }

    // -- Queries --

    async fn workspace_snapshot(&self) -> BResult<WorkspaceSnapshot> {
        self.client
            .workspace_snapshot()
            .await
            .map_err(into_backend_error)
    }

    async fn agent_states(&self, fresh: bool) -> BResult<AgentStatesSnapshot> {
        self.client
            .agent_states(fresh)
            .await
            .map_err(into_backend_error)
    }

    async fn session_detail(
        &self,
        query: &str,
        lines: Option<usize>,
    ) -> BResult<Option<SessionDetail>> {
        self.client
            .session_detail(query, lines)
            .await
            .map_err(into_backend_error)
    }

    async fn preview(&self, target: PreviewTarget) -> BResult<PreviewData> {
        let result = match target {
            PreviewTarget::Session { id, lines } => self.client.session_preview(id, lines).await,
            PreviewTarget::Project(id) => self.client.project_preview(id).await,
        };
        result.map_err(into_backend_error)
    }

    async fn branch_diff(&self, id: SessionId) -> BResult<String> {
        self.client
            .branch_diff(id)
            .await
            .map_err(into_backend_error)
    }

    async fn list_branches(&self, project: ProjectId, fetch: bool) -> BResult<Vec<BranchInfo>> {
        self.client
            .list_branches(project, fetch)
            .await
            .map_err(into_backend_error)
    }

    async fn create_options(&self) -> BResult<CreateOptions> {
        self.client
            .create_options()
            .await
            .map_err(into_backend_error)
    }

    async fn set_programs(&self, programs: Vec<ProgramInfo>) -> BResult<()> {
        self.client
            .set_programs(programs)
            .await
            .map_err(into_backend_error)
    }

    async fn pending_comment_sessions(&self) -> BResult<Vec<SessionId>> {
        self.client
            .pending_comment_sessions()
            .await
            .map_err(into_backend_error)
    }

    // -- Session mutations --

    async fn create_session(&self, opts: CreateSessionOpts) -> BResult<SessionId> {
        self.client
            .create_session(opts)
            .await
            .map_err(into_backend_error)
    }

    async fn kill_session(&self, id: SessionId) -> BResult<()> {
        self.client
            .kill_session(id)
            .await
            .map_err(into_backend_error)
    }

    async fn restart_session(&self, id: SessionId) -> BResult<()> {
        self.client
            .restart_session(id)
            .await
            .map_err(into_backend_error)
    }

    async fn delete_session(&self, id: SessionId) -> BResult<()> {
        self.client
            .delete_session(id)
            .await
            .map_err(into_backend_error)
    }

    async fn rename_session(&self, id: SessionId, title: String) -> BResult<()> {
        self.client
            .rename_session(id, title)
            .await
            .map_err(into_backend_error)
    }

    async fn set_section(&self, id: SessionId, section: Option<String>) -> BResult<()> {
        self.client
            .set_section(id, section)
            .await
            .map_err(into_backend_error)
    }
    async fn change_program(&self, id: SessionId, program: String) -> BResult<()> {
        self.client
            .change_program(id, program)
            .await
            .map_err(into_backend_error)
    }
    async fn paste_image(&self, id: SessionId, png: Vec<u8>) -> BResult<()> {
        self.client
            .paste_image(id, png)
            .await
            .map_err(into_backend_error)
    }

    async fn mark_read(&self, id: SessionId) -> BResult<()> {
        self.client.mark_read(id).await.map_err(into_backend_error)
    }

    async fn toggle_keep_alive(&self, id: SessionId) -> BResult<bool> {
        self.client
            .toggle_keep_alive(id)
            .await
            .map_err(into_backend_error)
    }

    async fn mark_unread(&self, ids: Vec<SessionId>) -> BResult<()> {
        self.client
            .mark_unread(ids)
            .await
            .map_err(into_backend_error)
    }

    // -- Projects --

    async fn add_project(&self, path: PathBuf) -> BResult<ProjectId> {
        self.client
            .add_project(path)
            .await
            .map_err(into_backend_error)
    }

    async fn remove_project(&self, id: ProjectId) -> BResult<()> {
        self.client
            .remove_project(id)
            .await
            .map_err(into_backend_error)
    }

    async fn scan_directory(&self, dir: PathBuf) -> BResult<ScanResult> {
        // The wire response mirrors `ScanResult`'s fields (which isn't
        // `Deserialize`); rebuild the core type from it.
        let body = self
            .client
            .scan_directory(dir)
            .await
            .map_err(into_backend_error)?;
        Ok(ScanResult {
            added: body.added,
            skipped: body.skipped,
        })
    }

    // -- Cascade / push-stack --

    async fn cascade_merge(&self, id: SessionId) -> BResult<OperationStatus> {
        self.client
            .cascade_merge(id)
            .await
            .map_err(into_backend_error)
    }

    async fn cascade_resume(&self) -> BResult<OperationStatus> {
        self.client
            .cascade_resume()
            .await
            .map_err(into_backend_error)
    }

    async fn cascade_abandon(&self) -> BResult<()> {
        self.client
            .cascade_abandon()
            .await
            .map_err(into_backend_error)
    }

    async fn push_stack(&self, id: SessionId) -> BResult<OperationStatus> {
        self.client.push_stack(id).await.map_err(into_backend_error)
    }

    // -- Review / comments --

    async fn list_comments(&self, id: SessionId) -> BResult<Vec<Comment>> {
        self.client
            .list_comments(id)
            .await
            .map_err(into_backend_error)
    }

    async fn open_review(&self, id: SessionId) -> BResult<ReviewSnapshot> {
        self.client
            .open_review(id)
            .await
            .map_err(into_backend_error)
    }

    async fn refresh_review_if_changed(
        &self,
        id: SessionId,
        prev_hash: u64,
    ) -> BResult<Option<ReviewSnapshot>> {
        self.client
            .refresh_review_if_changed(id, prev_hash)
            .await
            .map_err(into_backend_error)
    }

    async fn create_comment(&self, id: SessionId, draft: NewComment) -> BResult<Uuid> {
        self.client
            .create_comment(id, draft)
            .await
            .map_err(into_backend_error)
    }

    async fn delete_comment(&self, id: SessionId, comment_id: Uuid) -> BResult<()> {
        self.client
            .delete_comment(id, comment_id)
            .await
            .map_err(into_backend_error)
    }

    async fn apply_comments(&self, id: SessionId) -> BResult<ApplyOutcome> {
        self.client
            .apply_comments(id)
            .await
            .map_err(into_backend_error)
    }

    async fn toggle_file_reviewed(&self, id: SessionId, display_path: String) -> BResult<bool> {
        self.client
            .toggle_file_reviewed(id, display_path)
            .await
            .map_err(into_backend_error)
    }

    async fn fetch_diff_blob(
        &self,
        id: SessionId,
        side: DiffSide,
        path: String,
    ) -> BResult<Vec<u8>> {
        self.client
            .fetch_diff_blob(id, side, path)
            .await
            .map_err(into_backend_error)
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
        let conn = self
            .client
            .attach(id, cols, rows, pane)
            .await
            .map_err(into_backend_error)?;
        Ok(Box::new(RemoteAttachConnection(conn)))
    }
}

/// One-shot Slack notify for the CLI `slack notify` path: POST to the server
/// described by `info` (its runtime `server-info.json`) without spinning up a
/// [`RemoteBackend`]'s background poller. Maps any failure to a human string —
/// the server's error message when it sent one, else the transport error — so
/// the CLI can print it and exit non-zero.
pub async fn slack_notify(
    info: &claude_commander_core::ServerInfo,
    req: &claude_commander_protocol::api::SlackNotifyRequest,
) -> Result<(), String> {
    use claude_commander_client::SecretString;
    let spec = RemoteServerSpec {
        name: "local".to_string(),
        base_url: info.url.clone(),
        token: info.token.clone().map(SecretString::new),
    };
    let client = RemoteClient::new(spec).map_err(|e| e.to_string())?;
    client.slack_notify(req).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;

    use claude_commander_client::SecretString;
    use claude_commander_core::api::CommanderService;
    use claude_commander_core::backend::{AttachEnd, AttachStreams, BackendError};
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
    fn with_config_rejects_non_http_scheme() {
        // Scheme validation happens before any tokio work, so no runtime needed.
        // The client's InvalidRequest must map through to a BackendError.
        let spec = RemoteServerSpec {
            name: "box".to_string(),
            base_url: "ftp://box".to_string(),
            token: None,
        };
        match RemoteBackend::with_config(spec, idle_config()) {
            Err(BackendError::InvalidRequest(m)) => assert!(
                m.contains("http or https"),
                "non-http(s) scheme must be rejected at construction: {m}"
            ),
            Ok(_) => panic!("non-http(s) scheme must be rejected"),
            Err(other) => panic!("expected InvalidRequest, got {other:?}"),
        }
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
            // Image paste is the exception: a remote agent can't read the
            // operator's clipboard, so the client must capture + upload it.
            assert!(caps.client_side_image_paste);
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
    async fn set_programs_round_trips_to_server_config() {
        use claude_commander_core::api::ProgramInfo;

        let (addr, service, _d, _w) = serve_disabled().await;
        // A fresh server has no configured programs.
        assert!(service.read_config().programs.is_empty());

        let backend = RemoteBackend::with_config(spec(addr, None), idle_config()).unwrap();
        backend
            .set_programs(vec![
                ProgramInfo {
                    label: "Claude (Opus)".to_string(),
                    command: "claude --model opus".to_string(),
                },
                ProgramInfo {
                    label: "Shell".to_string(),
                    command: "bash".to_string(),
                },
            ])
            .await
            .unwrap();

        // The PUT reached the server and rewrote its config; create_options
        // (what a client fetches to build the picker) now reflects it.
        let opts = service.create_options();
        assert_eq!(opts.programs.len(), 2);
        assert_eq!(opts.programs[0].command, "claude --model opus");
        assert_eq!(opts.default_program, "claude --model opus");

        // An empty list round-trips too.
        backend.set_programs(vec![]).await.unwrap();
        assert!(service.read_config().programs.is_empty());
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
    async fn mark_unread_flags_sessions_over_http() {
        // Wire check for `POST /api/sessions/unread`: a seeded, read session is
        // flagged unread over HTTP and the change is visible in the next snapshot.
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
                let mut session = session;
                session.unread = false;
                state.add_session(session);
            })
            .await
            .unwrap();

        let backend = RemoteBackend::with_config(spec(addr, None), idle_config()).unwrap();
        // Precondition: not unread.
        let before = backend.workspace_snapshot().await.unwrap();
        assert!(
            !before
                .sessions
                .iter()
                .find(|s| s.session_id == sid)
                .unwrap()
                .unread
        );

        backend.mark_unread(vec![sid]).await.unwrap();

        let after = backend.workspace_snapshot().await.unwrap();
        assert!(
            after
                .sessions
                .iter()
                .find(|s| s.session_id == sid)
                .unwrap()
                .unread,
            "session should be flagged unread after mark_unread over HTTP"
        );
    }

    #[tokio::test]
    async fn mark_unread_unknown_ids_is_ok() {
        // Unknown ids are silently skipped server-side (a 204), matching local.
        let (addr, _service, _d, _w) = serve_disabled().await;
        let backend = RemoteBackend::with_config(spec(addr, None), idle_config()).unwrap();
        backend.mark_unread(vec![SessionId::new()]).await.unwrap();
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
    async fn unknown_session_change_program_is_not_found() {
        let (addr, _service, _d, _w) = serve_disabled().await;
        let backend = RemoteBackend::with_config(spec(addr, None), idle_config()).unwrap();
        let err = backend
            .change_program(SessionId::new(), "codex".to_string())
            .await
            .unwrap_err();
        assert!(matches!(err, BackendError::NotFound), "got {err:?}");
    }

    #[tokio::test]
    async fn paste_image_valid_png_unknown_session_is_not_found() {
        const TINY_PNG: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let (addr, _service, _d, _w) = serve_disabled().await;
        let backend = RemoteBackend::with_config(spec(addr, None), idle_config()).unwrap();
        let err = backend
            .paste_image(SessionId::new(), TINY_PNG.to_vec())
            .await
            .unwrap_err();
        assert!(matches!(err, BackendError::NotFound), "got {err:?}");
    }

    #[tokio::test]
    async fn paste_image_non_image_is_invalid_request() {
        let (addr, _service, _d, _w) = serve_disabled().await;
        let backend = RemoteBackend::with_config(spec(addr, None), idle_config()).unwrap();
        let err = backend
            .paste_image(SessionId::new(), b"not an image".to_vec())
            .await
            .unwrap_err();
        assert!(
            matches!(err, BackendError::InvalidRequest(_)),
            "got {err:?}"
        );
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
                model: None,
                base_branch: None,
                section: None,
                stack_parent: None,
                slack_origin: None,
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
                model: None,
                base_branch: None,
                section: None,
                stack_parent: None,
                slack_origin: None,
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
            read_until_contains(&mut *reader, "cc_remote_marker", Duration::from_secs(20)).await,
            "attached PTY output should echo the typed marker"
        );

        // A resize is fire-and-forget; just prove it doesn't disrupt the stream.
        resizer.resize(100, 30);

        // Detach: leaves the tmux session running.
        terminator.detach().await;
        // Reason kept loose for the same CI-runner pty flakiness documented in
        // the shell round-trip below; session survival is the strict contract.
        let end = terminator.wait().await;
        assert!(
            matches!(end, AttachEnd::Detached | AttachEnd::SessionEnded),
            "expected a clean end, got {end:?}"
        );

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
                model: None,
                base_branch: None,
                section: None,
                stack_parent: None,
                slack_origin: None,
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

        // Detach (streams still held, like the interactive loop does) and
        // confirm the shell session survives — the load-bearing contract,
        // asserted strictly below via tmux. The END REASON is deliberately
        // loose: on loaded CI runners the server-side `tmux attach` child can
        // exit spontaneously (diagnostics captured on such runs show the
        // session and pane alive and healthy throughout), so the server may
        // report SessionEnded racing our detach. Either way the WS teardown is
        // clean and the session must survive.
        terminator.detach().await;
        let end = terminator.wait().await;
        if !matches!(end, AttachEnd::Detached | AttachEnd::SessionEnded) {
            let tmux = TmuxExecutor::new().with_tmux_tmpdir(service.read_config().tmux_tmpdir);
            let ls = tmux.execute(&["list-sessions"]).await;
            let dead = tmux.is_pane_dead(&shell_name).await;
            let pane = tmux
                .execute(&["capture-pane", "-p", "-t", &shell_name])
                .await;
            panic!(
                "expected a clean end, got {end:?}\n  tmux ls: {ls:?}\n  {shell_name} pane dead: {dead:?}\n  pane content: {pane:?}"
            );
        }
        drop(reader);
        drop(writer);
        drop(resizer);

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
