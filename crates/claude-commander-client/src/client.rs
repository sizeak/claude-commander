//! [`RemoteClient`]: the pure HTTP + WebSocket transport for one
//! `claude-commander-server`.
//!
//! This is the shared, cloneable core the background [`Poller`](crate::Poller)
//! holds (via an `Arc`) and the thin `claude-commander-remote` adapter delegates
//! every backend method to. Each per-route method maps to one route on the
//! server's `/api` surface and returns a **wire DTO** from
//! `claude-commander-protocol` (or a [`ClientError`]) — never a core type — so
//! the crate stays independent of `claude-commander-core` and can cross-compile
//! to mobile targets.

use std::path::PathBuf;
use std::time::Duration;

use claude_commander_protocol::api::{
    AgentStatesSnapshot, BranchInfo, CreateOptions, CreateSessionOpts, DiffSide, NewComment,
    OperationStatus, PreviewData, ProgramInfo, ReviewSnapshot, SessionDetail, SetProgramsRequest,
    SlackNotifyRequest, ToggleReviewed, WorkspaceSnapshot,
};
use claude_commander_protocol::comment::{ApplyOutcome, Comment};
use claude_commander_protocol::session::{ProjectId, SessionId};
use claude_commander_protocol::ws::AttachKind;
use reqwest::{Client, RequestBuilder, Response, StatusCode, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;
use xxhash_rust::xxh3::Xxh3;

use crate::attach::AttachConnection;
use crate::error::{self, ClientError, ClientResult};
use crate::spec::{RemoteServerSpec, SecretString};

/// How long to wait for a TCP connection before treating the server as
/// unreachable. Kept short so the poller's backoff engages promptly.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Overall per-request ceiling (a slow branch-diff still fits comfortably).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Tighter per-request bound for interactive review reads/writes (list/create/
/// delete comment, toggle-reviewed): these are quick store/file operations, so
/// a user who fires one and waits shouldn't sit through the 30s
/// [`REQUEST_TIMEOUT`] when a server has wedged — a few seconds surfaces the
/// failure while still tolerating a slow link.
const REVIEW_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Per-request bound for `apply_comments`. Longer than [`REVIEW_WRITE_TIMEOUT`]
/// because the server does more work here — it recomposes a fresh review diff,
/// re-anchors every comment, then detects the agent's state and sends keys into
/// tmux — but still well under [`REQUEST_TIMEOUT`].
const APPLY_COMMENTS_TIMEOUT: Duration = Duration::from_secs(15);

/// The transport client for one remote `claude-commander-server`: the HTTP
/// client, the resolved base URL, and the (redacted) bearer token. Cloneable via
/// `Arc`; the poll task holds an `Arc` of this — never of the adapter backend —
/// so there's no cycle.
pub struct RemoteClient {
    name: String,
    client: Client,
    base: Url,
    token: Option<SecretString>,
}

impl RemoteClient {
    /// Build a client for the server described by `spec`. Fails only on a
    /// malformed `base_url` or an un-buildable HTTP client — the first
    /// *reachability* result surfaces later through the poller, not here.
    pub fn new(spec: RemoteServerSpec) -> ClientResult<Self> {
        let base = Url::parse(&spec.base_url)
            .map_err(|e| ClientError::InvalidRequest(format!("invalid server url: {e}")))?;
        if base.cannot_be_a_base() || base.host().is_none() {
            return Err(ClientError::InvalidRequest(
                "server url must include a host (e.g. http://host:port)".to_string(),
            ));
        }
        // The WS attach URL is derived by rewriting the scheme (http→ws,
        // https→wss); any other scheme would yield an unusable endpoint that
        // `ws_attach_url` passes through unchanged. Reject it here.
        if !matches!(base.scheme(), "http" | "https") {
            return Err(ClientError::InvalidRequest(format!(
                "server url must use http or https (got '{}')",
                base.scheme()
            )));
        }
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| ClientError::Unavailable {
                reason: format!("could not build http client: {e}"),
            })?;
        Ok(Self {
            name: spec.name,
            client,
            base,
            token: spec.token,
        })
    }

    /// The server's human-readable label (used in logging and the descriptor).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The `/ws/attach` WebSocket URL for this server (scheme mapped, path
    /// prefix preserved).
    fn ws_attach_url(&self) -> String {
        crate::attach::ws_attach_url(self.base.as_str())
    }

    /// The raw bearer token, if configured. Crate-internal and only handed to
    /// the request builder / the attach handshake's `auth` frame — never logged.
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

    fn session_url(&self, id: SessionId, tail: &[&str]) -> Url {
        let sid = id.as_uuid().to_string();
        let mut segments = Vec::with_capacity(2 + tail.len());
        segments.push("sessions");
        segments.push(&sid);
        segments.extend_from_slice(tail);
        self.endpoint(&segments)
    }

    fn project_url(&self, id: ProjectId, tail: &[&str]) -> Url {
        let pid = id.as_uuid().to_string();
        let mut segments = Vec::with_capacity(2 + tail.len());
        segments.push("projects");
        segments.push(&pid);
        segments.extend_from_slice(tail);
        self.endpoint(&segments)
    }

    // -- Verb helpers --

    /// Attach the bearer header (when set) and send, mapping a transport failure
    /// (no response) to [`ClientError::Unavailable`]/`Protocol`.
    async fn send(&self, request: RequestBuilder) -> ClientResult<Response> {
        let request = match &self.token {
            Some(token) => request.bearer_auth(token.expose()),
            None => request,
        };
        request.send().await.map_err(|err| {
            tracing::debug!(server = %self.name, error = %err, "remote request failed in transport");
            error::transport_error(err)
        })
    }

    /// Turn a non-success status into the matching [`ClientError`], reading the
    /// server's error body for the reason; pass a success response through.
    async fn check(&self, response: Response) -> ClientResult<Response> {
        let status = response.status();
        if status.is_success() {
            Ok(response)
        } else {
            Err(error::status_error(
                status,
                error::error_message(response).await,
            ))
        }
    }

    async fn get_json<T: DeserializeOwned>(&self, url: Url) -> ClientResult<T> {
        self.get_json_within(url, REQUEST_TIMEOUT).await
    }

    /// [`Self::get_json`] with a per-request `timeout` overriding the client-wide
    /// [`REQUEST_TIMEOUT`] (used by interactive review reads — see
    /// [`REVIEW_WRITE_TIMEOUT`]).
    async fn get_json_within<T: DeserializeOwned>(
        &self,
        url: Url,
        timeout: Duration,
    ) -> ClientResult<T> {
        let response = self.send(self.client.get(url).timeout(timeout)).await?;
        let response = self.check(response).await?;
        decode_json(response).await
    }

    /// GET where a `404` means "no such thing" rather than an error (the detail
    /// route resolves a free-form query and 404s when nothing matches).
    async fn get_json_opt<T: DeserializeOwned>(&self, url: Url) -> ClientResult<Option<T>> {
        let response = self.send(self.client.get(url)).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = self.check(response).await?;
        Ok(Some(decode_json(response).await?))
    }

    /// GET where a `204 No Content` means "unchanged" (the review-refresh route).
    async fn get_json_if_present<T: DeserializeOwned>(&self, url: Url) -> ClientResult<Option<T>> {
        let response = self.send(self.client.get(url)).await?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        let response = self.check(response).await?;
        Ok(Some(decode_json(response).await?))
    }

    async fn get_text(&self, url: Url) -> ClientResult<String> {
        let response = self.send(self.client.get(url)).await?;
        let response = self.check(response).await?;
        response.text().await.map_err(error::body_error)
    }

    async fn get_bytes(&self, url: Url) -> ClientResult<Vec<u8>> {
        let response = self.send(self.client.get(url)).await?;
        let response = self.check(response).await?;
        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(error::body_error)
    }

    /// POST with no body, decoding the JSON response (cascade routes → 202 +
    /// `OperationStatus`).
    async fn post_empty_json<T: DeserializeOwned>(&self, url: Url) -> ClientResult<T> {
        self.post_empty_json_within(url, REQUEST_TIMEOUT).await
    }

    /// [`Self::post_empty_json`] with a per-request `timeout` overriding the
    /// client-wide [`REQUEST_TIMEOUT`] (used by `apply_comments` — see
    /// [`APPLY_COMMENTS_TIMEOUT`]).
    async fn post_empty_json_within<T: DeserializeOwned>(
        &self,
        url: Url,
        timeout: Duration,
    ) -> ClientResult<T> {
        let response = self.send(self.client.post(url).timeout(timeout)).await?;
        let response = self.check(response).await?;
        decode_json(response).await
    }

    /// POST a JSON body, decoding the JSON response (create routes → 201 + id).
    async fn post_json<T: DeserializeOwned, B: Serialize>(
        &self,
        url: Url,
        body: &B,
    ) -> ClientResult<T> {
        self.post_json_within(url, body, REQUEST_TIMEOUT).await
    }

    /// [`Self::post_json`] with a per-request `timeout` overriding the
    /// client-wide [`REQUEST_TIMEOUT`] (used by interactive review writes — see
    /// [`REVIEW_WRITE_TIMEOUT`]).
    async fn post_json_within<T: DeserializeOwned, B: Serialize>(
        &self,
        url: Url,
        body: &B,
        timeout: Duration,
    ) -> ClientResult<T> {
        let response = self
            .send(self.client.post(url).json(body).timeout(timeout))
            .await?;
        let response = self.check(response).await?;
        decode_json(response).await
    }

    /// POST with no body, discarding the (204/empty) response.
    async fn post_empty_ok(&self, url: Url) -> ClientResult<()> {
        let response = self.send(self.client.post(url)).await?;
        self.check(response).await?;
        Ok(())
    }

    /// POST a JSON body, discarding the (204) response.
    async fn post_json_ok<B: Serialize>(&self, url: Url, body: &B) -> ClientResult<()> {
        let response = self.send(self.client.post(url).json(body)).await?;
        self.check(response).await?;
        Ok(())
    }

    /// PATCH a JSON body, discarding the (204) response.
    async fn patch_json_ok<B: Serialize>(&self, url: Url, body: &B) -> ClientResult<()> {
        let response = self.send(self.client.patch(url).json(body)).await?;
        self.check(response).await?;
        Ok(())
    }

    /// POST a raw byte body with an explicit `Content-Type`, discarding the
    /// response. Used to upload a pasted image (PNG bytes) to the server.
    async fn post_bytes_ok(
        &self,
        url: Url,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> ClientResult<()> {
        let response = self
            .send(
                self.client
                    .post(url)
                    .header(reqwest::header::CONTENT_TYPE, content_type)
                    .body(bytes),
            )
            .await?;
        self.check(response).await?;
        Ok(())
    }

    /// PUT a JSON body, discarding the (204) response.
    async fn put_json_ok<B: Serialize>(&self, url: Url, body: &B) -> ClientResult<()> {
        let response = self.send(self.client.put(url).json(body)).await?;
        self.check(response).await?;
        Ok(())
    }

    /// DELETE, discarding the (204) response.
    async fn delete_ok(&self, url: Url) -> ClientResult<()> {
        self.delete_ok_within(url, REQUEST_TIMEOUT).await
    }

    /// [`Self::delete_ok`] with a per-request `timeout` overriding the
    /// client-wide [`REQUEST_TIMEOUT`] (used by interactive review writes — see
    /// [`REVIEW_WRITE_TIMEOUT`]).
    async fn delete_ok_within(&self, url: Url, timeout: Duration) -> ClientResult<()> {
        let response = self.send(self.client.delete(url).timeout(timeout)).await?;
        self.check(response).await?;
        Ok(())
    }

    /// Fetch the workspace + agent-state snapshots and content-hash them
    /// together. The poll loop compares this against the previous hash to decide
    /// whether observable state moved. Any HTTP/transport failure propagates so
    /// the poller can go degraded.
    pub async fn poll_hashes(&self) -> ClientResult<u64> {
        let workspace = self.get_bytes(self.endpoint(&["workspace"])).await?;
        let agent_states = self.get_bytes(self.endpoint(&["agent-states"])).await?;
        let mut hasher = Xxh3::new();
        hasher.update(&workspace);
        hasher.update(b"\x00");
        hasher.update(&agent_states);
        Ok(hasher.digest())
    }

    // -- Per-route methods --

    /// Liveness probe: `GET /health` (outside the `/api` bearer surface, so no
    /// auth needed). Returns whether the server answered a 2xx. A transport
    /// failure (unreachable) surfaces as [`ClientError::Unavailable`]. Used by
    /// the mobile connect screen before a poller exists.
    pub async fn health(&self) -> ClientResult<bool> {
        let mut url = self.base.clone();
        {
            let mut path = url
                .path_segments_mut()
                .expect("a validated http(s) base URL is always a base");
            path.pop_if_empty();
            path.push("health");
        }
        let response = self.send(self.client.get(url)).await?;
        Ok(response.status().is_success())
    }

    /// Authenticated tmux probe: `GET /api/health/tmux`. `200` → true, `503` →
    /// false; a `401`/`403` surfaces as [`ClientError::Auth`] so the connect
    /// screen can flag a bad token. Doubles as an auth check.
    pub async fn health_tmux(&self) -> ClientResult<bool> {
        let response = self
            .send(self.client.get(self.endpoint(&["health", "tmux"])))
            .await?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(ClientError::Auth);
        }
        Ok(status.is_success())
    }

    /// Ask the server to re-check PR metadata (it runs the PR-status loop).
    pub async fn request_pr_refresh(&self) -> ClientResult<()> {
        self.post_empty_ok(self.endpoint(&["pr-refresh"])).await
    }

    pub async fn workspace_snapshot(&self) -> ClientResult<WorkspaceSnapshot> {
        self.get_json(self.endpoint(&["workspace"])).await
    }

    pub async fn agent_states(&self, fresh: bool) -> ClientResult<AgentStatesSnapshot> {
        let mut url = self.endpoint(&["agent-states"]);
        url.query_pairs_mut()
            .append_pair("fresh", if fresh { "true" } else { "false" });
        self.get_json(url).await
    }

    pub async fn session_detail(
        &self,
        query: &str,
        lines: Option<usize>,
    ) -> ClientResult<Option<SessionDetail>> {
        let mut url = self.endpoint(&["sessions", query, "detail"]);
        if let Some(lines) = lines {
            url.query_pairs_mut()
                .append_pair("lines", &lines.to_string());
        }
        self.get_json_opt(url).await
    }

    /// Preview payload for a session (`GET /api/sessions/{id}/preview?lines=`).
    pub async fn session_preview(
        &self,
        id: SessionId,
        lines: Option<usize>,
    ) -> ClientResult<PreviewData> {
        let mut url = self.session_url(id, &["preview"]);
        if let Some(lines) = lines {
            url.query_pairs_mut()
                .append_pair("lines", &lines.to_string());
        }
        self.get_json(url).await
    }

    /// Preview payload for a project (`GET /api/projects/{id}/preview`).
    pub async fn project_preview(&self, id: ProjectId) -> ClientResult<PreviewData> {
        self.get_json(self.project_url(id, &["preview"])).await
    }

    pub async fn branch_diff(&self, id: SessionId) -> ClientResult<String> {
        self.get_text(self.session_url(id, &["branch-diff"])).await
    }

    pub async fn list_branches(
        &self,
        project: ProjectId,
        fetch: bool,
    ) -> ClientResult<Vec<BranchInfo>> {
        let mut url = self.project_url(project, &["branches"]);
        url.query_pairs_mut()
            .append_pair("fetch", if fetch { "true" } else { "false" });
        self.get_json(url).await
    }

    pub async fn create_options(&self) -> ClientResult<CreateOptions> {
        self.get_json(self.endpoint(&["create-options"])).await
    }

    pub async fn set_programs(&self, programs: Vec<ProgramInfo>) -> ClientResult<()> {
        self.put_json_ok(
            self.endpoint(&["config", "programs"]),
            &SetProgramsRequest { programs },
        )
        .await
    }

    pub async fn pending_comment_sessions(&self) -> ClientResult<Vec<SessionId>> {
        self.get_json(self.endpoint(&["comments", "pending"])).await
    }

    // -- Session mutations --

    pub async fn create_session(&self, opts: CreateSessionOpts) -> ClientResult<SessionId> {
        let env: IdEnvelope<SessionId> =
            self.post_json(self.endpoint(&["sessions"]), &opts).await?;
        Ok(env.id)
    }

    pub async fn kill_session(&self, id: SessionId) -> ClientResult<()> {
        self.post_empty_ok(self.session_url(id, &["kill"])).await
    }

    pub async fn restart_session(&self, id: SessionId) -> ClientResult<()> {
        self.post_empty_ok(self.session_url(id, &["restart"])).await
    }

    pub async fn delete_session(&self, id: SessionId) -> ClientResult<()> {
        self.delete_ok(self.session_url(id, &[])).await
    }

    pub async fn rename_session(&self, id: SessionId, title: String) -> ClientResult<()> {
        let body = serde_json::json!({ "op": "rename", "title": title });
        self.patch_json_ok(self.session_url(id, &[]), &body).await
    }

    pub async fn set_section(&self, id: SessionId, section: Option<String>) -> ClientResult<()> {
        let body = serde_json::json!({ "op": "set_section", "section": section });
        self.patch_json_ok(self.session_url(id, &[]), &body).await
    }

    /// Change a session's launch program (PATCH `change_program` op).
    pub async fn change_program(&self, id: SessionId, program: String) -> ClientResult<()> {
        let body = serde_json::json!({ "op": "change_program", "program": program });
        self.patch_json_ok(self.session_url(id, &[]), &body).await
    }

    /// Upload a pasted image (PNG bytes) to a session (`POST /paste-image`).
    pub async fn paste_image(&self, id: SessionId, png: Vec<u8>) -> ClientResult<()> {
        self.post_bytes_ok(self.session_url(id, &["paste-image"]), png, "image/png")
            .await
    }

    pub async fn mark_read(&self, id: SessionId) -> ClientResult<()> {
        self.post_empty_ok(self.session_url(id, &["read"])).await
    }

    pub async fn toggle_keep_alive(&self, id: SessionId) -> ClientResult<bool> {
        self.post_empty_json(self.session_url(id, &["keep-alive"]))
            .await
    }

    pub async fn mark_unread(&self, ids: Vec<SessionId>) -> ClientResult<()> {
        // Batch counterpart to `mark_read`: `POST /api/sessions/unread` with
        // `{ "ids": [...] }`. Unknown ids are silently skipped server-side,
        // matching the local backend.
        let ids: Vec<String> = ids.iter().map(|id| id.as_uuid().to_string()).collect();
        let body = serde_json::json!({ "ids": ids });
        self.post_json_ok(self.endpoint(&["sessions", "unread"]), &body)
            .await
    }

    // -- Projects --

    pub async fn add_project(&self, path: PathBuf) -> ClientResult<ProjectId> {
        let body = serde_json::json!({ "path": path });
        let env: IdEnvelope<ProjectId> =
            self.post_json(self.endpoint(&["projects"]), &body).await?;
        Ok(env.id)
    }

    pub async fn remove_project(&self, id: ProjectId) -> ClientResult<()> {
        self.delete_ok(self.project_url(id, &[])).await
    }

    pub async fn scan_directory(&self, dir: PathBuf) -> ClientResult<ScanResponse> {
        let url = self.endpoint(&["projects", "scan"]);
        self.post_json(url, &ScanRequest { path: dir }).await
    }

    // -- Cascade / push-stack --

    pub async fn cascade_merge(&self, id: SessionId) -> ClientResult<OperationStatus> {
        self.post_empty_json(self.session_url(id, &["cascade"]))
            .await
    }

    pub async fn cascade_resume(&self) -> ClientResult<OperationStatus> {
        self.post_empty_json(self.endpoint(&["cascade", "resume"]))
            .await
    }

    pub async fn cascade_abandon(&self) -> ClientResult<()> {
        self.post_empty_ok(self.endpoint(&["cascade", "abandon"]))
            .await
    }

    pub async fn push_stack(&self, id: SessionId) -> ClientResult<OperationStatus> {
        self.post_empty_json(self.session_url(id, &["push-stack"]))
            .await
    }

    // -- Review / comments --

    pub async fn list_comments(&self, id: SessionId) -> ClientResult<Vec<Comment>> {
        self.get_json_within(self.session_url(id, &["comments"]), REVIEW_WRITE_TIMEOUT)
            .await
    }

    pub async fn open_review(&self, id: SessionId) -> ClientResult<ReviewSnapshot> {
        self.get_json(self.session_url(id, &["review"])).await
    }

    pub async fn refresh_review_if_changed(
        &self,
        id: SessionId,
        prev_hash: u64,
    ) -> ClientResult<Option<ReviewSnapshot>> {
        let mut url = self.session_url(id, &["review", "refresh"]);
        url.query_pairs_mut()
            .append_pair("prev_hash", &prev_hash.to_string());
        self.get_json_if_present(url).await
    }

    pub async fn create_comment(&self, id: SessionId, draft: NewComment) -> ClientResult<Uuid> {
        let env: IdEnvelope<Uuid> = self
            .post_json_within(
                self.session_url(id, &["comments"]),
                &draft,
                REVIEW_WRITE_TIMEOUT,
            )
            .await?;
        Ok(env.id)
    }

    pub async fn delete_comment(&self, id: SessionId, comment_id: Uuid) -> ClientResult<()> {
        let cid = comment_id.to_string();
        self.delete_ok_within(
            self.session_url(id, &["comments", &cid]),
            REVIEW_WRITE_TIMEOUT,
        )
        .await
    }

    pub async fn apply_comments(&self, id: SessionId) -> ClientResult<ApplyOutcome> {
        self.post_empty_json_within(
            self.session_url(id, &["comments", "apply"]),
            APPLY_COMMENTS_TIMEOUT,
        )
        .await
    }

    pub async fn toggle_file_reviewed(
        &self,
        id: SessionId,
        display_path: String,
    ) -> ClientResult<bool> {
        let body = ToggleReviewed { display_path };
        let out: ReviewedBody = self
            .post_json_within(
                self.session_url(id, &["files", "reviewed"]),
                &body,
                REVIEW_WRITE_TIMEOUT,
            )
            .await?;
        Ok(out.reviewed)
    }

    pub async fn fetch_diff_blob(
        &self,
        id: SessionId,
        side: DiffSide,
        path: String,
    ) -> ClientResult<Vec<u8>> {
        let mut url = self.session_url(id, &["blob"]);
        url.query_pairs_mut()
            .append_pair("side", diff_side_param(side))
            .append_pair("path", &path);
        self.get_bytes(url).await
    }

    /// Relay a worker's message to Slack via the server (`POST /api/slack/notify`).
    /// The server owns the Slack client and resolves the session's thread/DM;
    /// this is the one-shot call the CLI `slack notify` path makes.
    pub async fn slack_notify(&self, req: &SlackNotifyRequest) -> ClientResult<()> {
        self.post_json_ok(self.endpoint(&["slack", "notify"]), req)
            .await
    }

    // -- Attach --

    /// Open a live attach to a session's `kind` pane over the `/ws/attach`
    /// WebSocket, sized `cols`×`rows`. Returns a client-local [`AttachConnection`]
    /// the caller (the remote adapter) wraps into core's attach seam.
    pub async fn attach(
        &self,
        id: SessionId,
        cols: u16,
        rows: u16,
        kind: AttachKind,
    ) -> ClientResult<AttachConnection> {
        crate::attach::connect(
            &self.ws_attach_url(),
            self.token(),
            id.as_uuid().to_string(),
            cols,
            rows,
            kind,
        )
        .await
    }
}

async fn decode_json<T: DeserializeOwned>(response: Response) -> ClientResult<T> {
    response.json::<T>().await.map_err(error::body_error)
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

/// `POST /projects/scan` → `{ added, skipped }`. Mirrors the fields of core's
/// `ScanResult` (which isn't `Deserialize`); the remote adapter rebuilds the
/// core type from this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
pub struct ScanResponse {
    pub added: usize,
    pub skipped: usize,
}

/// Request body for `POST /projects/scan`: the directory to scan.
#[derive(serde::Serialize)]
struct ScanRequest {
    path: PathBuf,
}

fn diff_side_param(side: DiffSide) -> &'static str {
    match side {
        DiffSide::Old => "old",
        DiffSide::New => "new",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Interactive review reads/writes must be bounded tighter than the 30s
    /// [`REQUEST_TIMEOUT`] so a wedged server surfaces promptly, and the heavier
    /// `apply_comments` must sit between the two: strictly above the quick-write
    /// bound (it recomposes a diff + drives tmux) yet strictly below the overall
    /// ceiling. Asserted at the constant level rather than by wall-clock so it
    /// stays deterministic.
    #[test]
    fn review_timeouts_are_bounded_between_write_and_request_ceiling() {
        assert!(
            REVIEW_WRITE_TIMEOUT < APPLY_COMMENTS_TIMEOUT,
            "quick review writes must be tighter than the apply bound"
        );
        assert!(
            APPLY_COMMENTS_TIMEOUT < REQUEST_TIMEOUT,
            "apply_comments must stay under the overall request ceiling"
        );
        // The tightest interactive bound must still cover the TCP connect budget
        // (which reqwest counts inside the total request timeout), or a
        // healthy-but-slow-to-connect link could time out with no budget left
        // for the response.
        assert!(
            REVIEW_WRITE_TIMEOUT >= CONNECT_TIMEOUT,
            "review-write budget must at least cover the connect budget"
        );
    }

    #[test]
    fn new_rejects_non_http_scheme() {
        let spec = RemoteServerSpec {
            name: "box".to_string(),
            base_url: "ftp://box".to_string(),
            token: None,
        };
        match RemoteClient::new(spec) {
            Err(ClientError::InvalidRequest(m)) => assert!(
                m.contains("http or https"),
                "non-http(s) scheme must be rejected at construction: {m}"
            ),
            Ok(_) => panic!("non-http(s) scheme must be rejected"),
            Err(other) => panic!("expected InvalidRequest, got {other:?}"),
        }
    }

    #[test]
    fn new_rejects_url_without_host() {
        match RemoteClient::new(RemoteServerSpec {
            name: "box".to_string(),
            base_url: "not a url".to_string(),
            token: None,
        }) {
            Err(ClientError::InvalidRequest(_)) => {}
            Err(other) => panic!("expected InvalidRequest, got {other:?}"),
            Ok(_) => panic!("a hostless url must be rejected"),
        }
    }

    #[test]
    fn endpoint_builds_api_path_and_escapes_segments() {
        let client = RemoteClient::new(RemoteServerSpec {
            name: "box".to_string(),
            base_url: "http://host:8080".to_string(),
            token: None,
        })
        .unwrap();
        // A bare base must not produce `//api`, and free-form segments are
        // percent-encoded.
        assert_eq!(
            client.endpoint(&["sessions", "a b", "detail"]).as_str(),
            "http://host:8080/api/sessions/a%20b/detail"
        );
    }

    #[test]
    fn diff_side_param_wire_forms() {
        assert_eq!(diff_side_param(DiffSide::Old), "old");
        assert_eq!(diff_side_param(DiffSide::New), "new");
    }
}
