//! HTTP + WebSocket routes for the web UI.
//!
//! Handlers are thin: they parse/validate input, call a [`CommanderService`]
//! method (the same one the CLI/TUI use), and serialize the result. All the
//! real logic lives in the service, so the web layer adds no untested behaviour
//! of its own.

// The internal control-flow helpers here use `Result<T, Response>` so a failed
// parse / join short-circuits straight to the HTTP response that represents it.
// The `Response` "error" is the genuine return payload, not an error type to be
// boxed — these results are request-scoped and never stored, so boxing them
// would add allocation and indirection for no benefit.
#![allow(clippy::result_large_err)]

use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::api::CreateSessionOpts;
use crate::session::{ProjectId, SessionId};

use super::{WebState, assets};

/// Build the application router (without the auth layer — that is wrapped on by
/// the caller so it also covers the fallback/asset routes).
pub(crate) fn router(state: WebState) -> Router {
    Router::new()
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route(
            "/api/sessions/{id}",
            get(session_detail).delete(delete_session),
        )
        .route("/api/sessions/{id}/restart", post(restart_session))
        .route("/api/sessions/{id}/kill", post(kill_session))
        .route("/api/sessions/{id}/scrollback", get(session_scrollback))
        .route("/ws/sessions/{id}", get(terminal_ws))
        // Projects / repos
        .route("/api/projects", get(list_projects).post(add_project))
        .route("/api/projects/scan", post(scan_directory))
        .route("/api/projects/{id}", axum::routing::delete(remove_project))
        // Settings
        .route("/api/config", get(get_config).put(update_config))
        // Static options for the new-session form (effort levels, modes, sections)
        .route("/api/meta", get(get_meta))
        .fallback(assets::serve_asset)
        .with_state(state)
}

/// Uniform JSON error body.
#[derive(Serialize)]
struct ApiError {
    error: String,
}

/// Map a domain error to an HTTP response. Not-found-ish session errors become
/// 404; everything else is a 500 with the message.
fn api_err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(ApiError { error: msg.into() })).into_response()
}

/// Control message a browser sends on the terminal WebSocket (text frame).
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ControlMessage {
    Resize { cols: u16, rows: u16 },
}

/// Parse a text frame as a resize control message, returning `(cols, rows)`.
/// Returns `None` for any frame that isn't a well-formed resize message, so the
/// caller can distinguish control traffic from stray text.
fn parse_resize(text: &str) -> Option<(u16, u16)> {
    match serde_json::from_str::<ControlMessage>(text) {
        Ok(ControlMessage::Resize { cols, rows }) if cols > 0 && rows > 0 => Some((cols, rows)),
        _ => None,
    }
}

/// Parse a path id into a [`SessionId`], or return a 400.
fn parse_id(id: &str) -> Result<SessionId, Response> {
    Uuid::parse_str(id)
        .map(SessionId::from_uuid)
        .map_err(|_| api_err(StatusCode::BAD_REQUEST, format!("invalid session id: {id}")))
}

/// Run a `!Send` service future to completion off the axum handler task.
///
/// Several lifecycle methods (`create`/`kill`/`delete`/`restart`) build a
/// transient gix `GitBackend`/`WorktreeManager` and hold it across an `.await`.
/// gix uses `Rc` internally, so those *futures* are `!Send` even though
/// [`CommanderService`] itself is `Send`. axum's [`Handler`] bound requires
/// `Send` futures, so we hop the work onto a dedicated current-thread runtime
/// inside a `spawn_blocking` thread: the closure we hand to `spawn_blocking` is
/// `Send` (it only moves the `Send` service + args), and the `!Send` future
/// lives entirely on that thread's runtime, never crossing a task boundary.
///
/// `f` is a constructor (not a future) so the `!Send` future is created *inside*
/// the worker thread, keeping the outer closure `Send`.
async fn run_local<T, F, Fut>(f: F) -> Result<T, Response>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = T>,
{
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread runtime for web handler");
        rt.block_on(f())
    })
    .await
    .map_err(|e| {
        api_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("worker join: {e}"),
        )
    })
}

// -- Queries --

async fn list_sessions(State(state): State<WebState>) -> Response {
    match state.service.list_sessions(true).await {
        Ok(sessions) => Json(sessions).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn session_detail(State(state): State<WebState>, Path(id): Path<String>) -> Response {
    // get_session_detail resolves by query string; the full UUID is unambiguous.
    match state.service.get_session_detail(&id, Some(200)).await {
        Ok(Some(detail)) => Json(detail).into_response(),
        Ok(None) => api_err(StatusCode::NOT_FOUND, "session not found"),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

// -- Mutations --

/// Request body for creating a session. Mirrors the fields of
/// [`CreateSessionOpts`] but is `Deserialize` and accepts a string path.
#[derive(Deserialize)]
struct CreateRequest {
    project_path: String,
    title: String,
    #[serde(default)]
    program: Option<String>,
    #[serde(default)]
    initial_prompt: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    base_branch: Option<String>,
    #[serde(default)]
    section: Option<String>,
}

#[derive(Serialize)]
struct CreateResponse {
    id: String,
}

async fn create_session(State(state): State<WebState>, Json(req): Json<CreateRequest>) -> Response {
    let opts = CreateSessionOpts {
        project_path: req.project_path.into(),
        title: req.title,
        program: req.program,
        initial_prompt: req.initial_prompt,
        effort: req.effort,
        mode: req.mode,
        base_branch: req.base_branch,
        section: req.section,
    };
    let service = state.service.clone();
    let result = run_local(move || async move { service.create_session(opts).await }).await;
    match result {
        Ok(Ok(id)) => (
            StatusCode::CREATED,
            Json(CreateResponse {
                id: id.as_uuid().to_string(),
            }),
        )
            .into_response(),
        Ok(Err(e)) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
        Err(resp) => resp,
    }
}

async fn restart_session(State(state): State<WebState>, Path(id): Path<String>) -> Response {
    let sid = match parse_id(&id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let service = state.service.clone();
    let result = run_local(move || async move { service.restart_session(&sid).await }).await;
    no_content_or_error(result)
}

async fn kill_session(State(state): State<WebState>, Path(id): Path<String>) -> Response {
    let sid = match parse_id(&id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let service = state.service.clone();
    let result = run_local(move || async move { service.kill_session(&sid).await }).await;
    no_content_or_error(result)
}

async fn delete_session(State(state): State<WebState>, Path(id): Path<String>) -> Response {
    let sid = match parse_id(&id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let service = state.service.clone();
    let result = run_local(move || async move { service.delete_session(&sid).await }).await;
    no_content_or_error(result)
}

/// Collapse a `run_local` result whose inner value is `Result<(), Error>` into a
/// `204 No Content` on success or a `500` with the message on failure.
fn no_content_or_error(result: Result<crate::error::Result<()>, Response>) -> Response {
    match result {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Err(resp) => resp,
    }
}

// -- Projects / repos --

async fn list_projects(State(state): State<WebState>) -> Response {
    match state.service.list_projects().await {
        Ok(projects) => Json(projects).into_response(),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct PathRequest {
    path: String,
}

async fn add_project(State(state): State<WebState>, Json(req): Json<PathRequest>) -> Response {
    let service = state.service.clone();
    let path = std::path::PathBuf::from(req.path);
    // add_project discovers the git repo (gix) → `!Send` future, run off-task.
    let result = run_local(move || async move { service.add_project(path).await }).await;
    match result {
        Ok(Ok(id)) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "id": id.as_uuid().to_string() })),
        )
            .into_response(),
        Ok(Err(e)) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
        Err(resp) => resp,
    }
}

#[derive(Serialize)]
struct ScanResponse {
    added: usize,
    skipped: usize,
}

async fn scan_directory(State(state): State<WebState>, Json(req): Json<PathRequest>) -> Response {
    let service = state.service.clone();
    let dir = std::path::PathBuf::from(req.path);
    let result = run_local(move || async move { service.scan_directory(&dir).await }).await;
    match result {
        Ok(Ok(scan)) => Json(ScanResponse {
            added: scan.added,
            skipped: scan.skipped,
        })
        .into_response(),
        Ok(Err(e)) => api_err(StatusCode::BAD_REQUEST, e.to_string()),
        Err(resp) => resp,
    }
}

async fn remove_project(State(state): State<WebState>, Path(id): Path<String>) -> Response {
    let pid = match Uuid::parse_str(&id).map(ProjectId::from_uuid) {
        Ok(p) => p,
        Err(_) => return api_err(StatusCode::BAD_REQUEST, format!("invalid project id: {id}")),
    };
    let service = state.service.clone();
    let result = run_local(move || async move { service.remove_project(&pid).await }).await;
    no_content_or_error(result)
}

// -- Settings --

/// Keys whose values are secrets and must never be sent to the browser. Replaced
/// with a redaction marker in GET /api/config so the UI can show "(set)"/"(unset)"
/// without leaking the value.
const REDACTED: &str = "********";

async fn get_config(State(state): State<WebState>) -> Response {
    let config = state.service.read_config();
    let mut value = match serde_json::to_value(&config) {
        Ok(v) => v,
        Err(e) => return api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    redact_secrets(&mut value);
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "config": value,
            "restart_required": state.service.restart_required(),
        })),
    )
        .into_response()
}

/// Replace secret config values with a marker in-place. Operates on the JSON
/// representation so it stays correct as long as the field names match `Config`.
fn redact_secrets(value: &mut serde_json::Value) {
    if let Some(obj) = value.as_object_mut() {
        if obj.get("web_ui_password").map(|v| !v.is_null()) == Some(true) {
            obj.insert("web_ui_password".into(), REDACTED.into());
        }
        if let Some(tel) = obj.get_mut("telemetry").and_then(|v| v.as_object_mut())
            && tel.get("token").map(|v| !v.is_null()) == Some(true)
        {
            tel.insert("token".into(), REDACTED.into());
        }
        if let Some(stt) = obj.get_mut("stt").and_then(|v| v.as_object_mut())
            && stt.get("api_key").map(|v| !v.is_null()) == Some(true)
        {
            stt.insert("api_key".into(), REDACTED.into());
        }
    }
}

/// Editable scalar settings the web UI exposes. Every field is optional so the
/// browser sends only what changed; unspecified fields keep their current value.
/// Deliberately a curated subset of `Config` (not the whole struct) so the web
/// form can't clobber nested/secret config it doesn't render.
#[derive(Deserialize)]
struct ConfigPatch {
    default_program: Option<String>,
    branch_prefix: Option<String>,
    worktrees_dir: Option<String>,
    editor: Option<String>,
    fetch_before_create: Option<bool>,
    resume_session: Option<bool>,
    web_ui_enabled: Option<bool>,
    web_ui_port: Option<u16>,
    /// "basic" or "mutual_tls".
    web_ui_auth: Option<String>,
    /// `null`/absent → unchanged; empty string → clear; the redaction marker →
    /// unchanged (UI echoed back the placeholder).
    web_ui_password: Option<String>,
    /// Cert paths: `null`/absent → unchanged; empty string → clear.
    web_ui_tls_cert: Option<String>,
    web_ui_tls_key: Option<String>,
    web_ui_tls_client_ca: Option<String>,
}

async fn update_config(State(state): State<WebState>, Json(patch): Json<ConfigPatch>) -> Response {
    let mut config = state.service.read_config();

    if let Some(v) = patch.default_program {
        config.default_program = v;
    }
    if let Some(v) = patch.branch_prefix {
        config.branch_prefix = v;
    }
    if let Some(v) = patch.worktrees_dir {
        config.worktrees_dir = if v.trim().is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(v))
        };
    }
    if let Some(v) = patch.editor {
        config.editor = if v.trim().is_empty() { None } else { Some(v) };
    }
    if let Some(v) = patch.fetch_before_create {
        config.fetch_before_create = v;
    }
    if let Some(v) = patch.resume_session {
        config.resume_session = v;
    }
    if let Some(v) = patch.web_ui_enabled {
        config.web_ui_enabled = v;
    }
    if let Some(v) = patch.web_ui_port {
        if v == 0 {
            return api_err(StatusCode::BAD_REQUEST, "web_ui_port must be 1–65535");
        }
        config.web_ui_port = v;
    }
    if let Some(v) = patch.web_ui_auth {
        config.web_ui_auth = match v.as_str() {
            "mutual_tls" => crate::config::WebUiAuth::MutualTls,
            "basic" => crate::config::WebUiAuth::Basic,
            other => {
                return api_err(
                    StatusCode::BAD_REQUEST,
                    format!("web_ui_auth must be \"basic\" or \"mutual_tls\", got {other:?}"),
                );
            }
        };
    }
    if let Some(v) = patch.web_ui_password {
        // The redaction marker means "leave as-is" (the UI echoed the placeholder
        // back). Empty means "clear it".
        if v != REDACTED {
            config.web_ui_password = if v.is_empty() { None } else { Some(v) };
        }
    }
    let opt_path = |v: String| {
        if v.trim().is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(v))
        }
    };
    if let Some(v) = patch.web_ui_tls_cert {
        config.web_ui_tls_cert = opt_path(v);
    }
    if let Some(v) = patch.web_ui_tls_key {
        config.web_ui_tls_key = opt_path(v);
    }
    if let Some(v) = patch.web_ui_tls_client_ca {
        config.web_ui_tls_client_ca = opt_path(v);
    }

    if let Err(e) = state.service.update_config(config) {
        return api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({ "restart_required": state.service.restart_required() })),
    )
        .into_response()
}

// -- Meta (static options for the new-session form) --

async fn get_meta(State(state): State<WebState>) -> Response {
    let config = state.service.read_config();
    let sections: Vec<&str> = config.sections.iter().map(|s| s.name.as_str()).collect();
    Json(serde_json::json!({
        // Claude effort levels and permission modes the create form offers.
        "effort_levels": ["low", "medium", "high", "xhigh"],
        "permission_modes": ["default", "acceptEdits", "bypassPermissions", "plan"],
        "sections": sections,
        "default_program": config.default_program,
    }))
    .into_response()
}

// -- Terminal WebSocket --

/// Query string for the scrollback endpoint: `?lines=N`.
#[derive(Deserialize)]
struct ScrollbackQuery {
    #[serde(default)]
    lines: Option<u32>,
}

/// Default and maximum number of history lines the scrollback view fetches.
/// The default is generous enough to scroll back through a long Claude turn; the
/// cap bounds the response size (tmux's own history-limit is the real ceiling).
const SCROLLBACK_DEFAULT_LINES: u32 = 2000;
const SCROLLBACK_MAX_LINES: u32 = 10_000;

/// `GET /api/sessions/{id}/scrollback?lines=N` — one-shot capture of the pane
/// with history, as `text/plain` (ANSI preserved). The live terminal WebSocket
/// only ever sends one visible screenful; this backs the browser's "history"
/// mode, where the user pauses the live mirror and scrolls up through output that
/// has scrolled off. 404 if the session no longer exists.
async fn session_scrollback(
    State(state): State<WebState>,
    Path(id): Path<String>,
    Query(q): Query<ScrollbackQuery>,
) -> Response {
    let lines = q
        .lines
        .unwrap_or(SCROLLBACK_DEFAULT_LINES)
        .clamp(1, SCROLLBACK_MAX_LINES);
    match state.service.capture_scrollback(&id, lines).await {
        Ok(Some(content)) => (
            [(
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8",
            )],
            content,
        )
            .into_response(),
        Ok(None) => api_err(StatusCode::NOT_FOUND, "session not found"),
        Err(e) => api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn terminal_ws(
    State(state): State<WebState>,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| terminal_bridge(state, id, socket))
}

/// Bridge a browser xterm.js terminal to a session's tmux pane.
///
/// Outbound: poll the pane content on `stream_interval_ms` and push the full
/// screen (ANSI included) whenever it changes — xterm.js renders the escape
/// sequences directly. We poll rather than stream because the underlying
/// capture is a cached `tmux capture-pane`; the interval matches the cache TTL.
///
/// Inbound: every binary frame (and text frame, for convenience) is forwarded
/// verbatim to the pane via the raw-bytes path, so keystrokes, control chars,
/// and escape sequences pass through unchanged.
async fn terminal_bridge(state: WebState, id: String, mut socket: WebSocket) {
    let service = state.service.clone();
    let interval = Duration::from_millis(state.stream_interval_ms);

    let mut ticker = tokio::time::interval(interval);
    let mut last_hash: Option<u64> = None;

    // `capture_terminal` returns `None` both when the session is genuinely gone
    // and on a transient miss (pane briefly empty, capture cache race, session
    // mid-restart). Treating the first `None` as closure made every reconnect /
    // hiccup look permanent. Only declare the session closed after several
    // consecutive misses; any successful capture resets the counter.
    const MAX_MISSES: u32 = 10;
    let mut consecutive_misses: u32 = 0;

    loop {
        tokio::select! {
            // Outbound: pane → browser
            _ = ticker.tick() => {
                match service.capture_terminal(&id).await {
                    Ok(Some(content)) => {
                        consecutive_misses = 0;
                        let hash = xxhash_rust::xxh3::xxh3_64(content.as_bytes());
                        if last_hash != Some(hash) {
                            last_hash = Some(hash);
                            if socket.send(Message::Text(content.into())).await.is_err() {
                                break; // client gone
                            }
                        }
                    }
                    Ok(None) => {
                        consecutive_misses += 1;
                        if consecutive_misses >= MAX_MISSES {
                            // Session is really gone (killed/deleted). Tell the
                            // client and stop.
                            let _ = socket
                                .send(Message::Text(
                                    "\r\n[session closed]\r\n".to_string().into(),
                                ))
                                .await;
                            break;
                        }
                    }
                    Err(e) => {
                        debug!("web terminal capture failed for {id}: {e}");
                    }
                }
            }

            // Inbound: browser → pane. Convention: BINARY frames are raw
            // keystrokes (forwarded verbatim); TEXT frames are JSON control
            // messages (currently just `resize`). This keeps the two streams
            // unambiguous without framing overhead on the hot keystroke path.
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Binary(bytes))) => {
                        if let Err(e) = service.send_input(&id, &bytes).await {
                            warn!("web terminal input failed for {id}: {e}");
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        if let Some((cols, rows)) = parse_resize(&text) {
                            if let Err(e) = service.resize_session(&id, cols, rows).await {
                                debug!("web terminal resize failed for {id}: {e}");
                            }
                            // Force a repaint on next tick at the new size.
                            last_hash = None;
                        } else {
                            warn!("web terminal: ignoring unrecognized text frame for {id}");
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // ping/pong handled by axum
                    Some(Err(e)) => {
                        debug!("web terminal socket error for {id}: {e}");
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_id_accepts_full_uuid() {
        let uuid = Uuid::new_v4();
        let parsed = parse_id(&uuid.to_string());
        assert!(parsed.is_ok());
        assert_eq!(parsed.unwrap().as_uuid(), &uuid);
    }

    #[test]
    fn parse_id_rejects_garbage() {
        assert!(parse_id("not-a-uuid").is_err());
        assert!(parse_id("abc123").is_err());
    }

    #[test]
    fn parse_resize_accepts_valid_message() {
        assert_eq!(
            parse_resize(r#"{"type":"resize","cols":120,"rows":40}"#),
            Some((120, 40))
        );
    }

    #[test]
    fn parse_resize_rejects_zero_dimensions() {
        assert_eq!(
            parse_resize(r#"{"type":"resize","cols":0,"rows":40}"#),
            None
        );
        assert_eq!(
            parse_resize(r#"{"type":"resize","cols":120,"rows":0}"#),
            None
        );
    }

    #[test]
    fn parse_resize_rejects_non_control_text() {
        // Plain keystrokes / unknown JSON must not be mistaken for a resize.
        assert_eq!(parse_resize("ls -la\n"), None);
        assert_eq!(parse_resize(r#"{"type":"other"}"#), None);
        assert_eq!(parse_resize("{not json"), None);
    }

    #[test]
    fn redact_secrets_masks_set_values_only() {
        let mut v = serde_json::json!({
            "web_ui_password": "hunter2",
            "telemetry": { "enabled": true, "token": "abc" },
            "stt": { "api_key": "k" },
            "default_program": "claude",
        });
        redact_secrets(&mut v);
        assert_eq!(v["web_ui_password"], REDACTED);
        assert_eq!(v["telemetry"]["token"], REDACTED);
        assert_eq!(v["stt"]["api_key"], REDACTED);
        // Non-secret fields are untouched.
        assert_eq!(v["default_program"], "claude");
        assert_eq!(v["telemetry"]["enabled"], true);
    }

    #[test]
    fn redact_secrets_leaves_null_secrets_as_null() {
        // An unset secret must stay null (so the UI shows "unset"), not "********".
        let mut v = serde_json::json!({
            "web_ui_password": serde_json::Value::Null,
            "telemetry": { "token": serde_json::Value::Null },
        });
        redact_secrets(&mut v);
        assert!(v["web_ui_password"].is_null());
        assert!(v["telemetry"]["token"].is_null());
    }
}
