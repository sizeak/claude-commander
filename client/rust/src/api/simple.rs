//! HTTP client API exposed to Flutter via flutter_rust_bridge.
//!
//! Each `pub fn` here becomes an async-callable function on the Dart side (frb
//! runs the Rust body on a worker thread, so blocking reqwest calls don't block
//! the UI isolate). Responses are validated against the shared
//! `claude-commander-protocol` types, so any drift from the server fails here in
//! Rust rather than silently in the UI.

use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use claude_commander_protocol::api::{CreateSessionOpts, SessionDetail, SessionInfo};
use reqwest::blocking::{Client, Response};
use reqwest::StatusCode;

#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    // Default utilities (logging, panic backtraces) for the bridge.
    flutter_rust_bridge::setup_default_user_utils();
}

/// Trim a trailing slash so `{base}/path` joins cleanly.
fn base(base_url: &str) -> &str {
    base_url.trim_end_matches('/')
}

/// Build `{base}/api/sessions/{query}/{leaf}` with `query` percent-encoded as a
/// single path segment. The loose session query may be a branch or title
/// prefix, so a raw '/', '?', or '#' in it must not restructure the URL (which
/// would 404 and read as "session gone").
fn session_url(base_url: &str, query: &str, leaf: &str) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(base(base_url)).context("invalid base URL")?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("base URL cannot be used as a base"))?
        .extend(["api", "sessions", query, leaf]);
    Ok(url)
}

/// A process-wide blocking client, so repeated calls (e.g. the 2s detail poll)
/// reuse connections instead of building a fresh pool each time. `Client` is
/// `Arc`-backed, so the clone is cheap and shares the pool.
fn client() -> Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(Client::new).clone()
}

/// Map a response to a `Result`: a 401 becomes a friendly auth error, any other
/// non-2xx surfaces via `error_for_status`, and a 2xx passes through. `what`
/// labels the failing call in the error message.
fn ok_or_status(resp: Response, what: &str) -> Result<Response> {
    if resp.status() == StatusCode::UNAUTHORIZED {
        anyhow::bail!("authentication failed (check your token)");
    }
    resp.error_for_status()
        .with_context(|| format!("{what}: server returned an error status"))
}

/// Liveness probe: `GET {base_url}/health` (no auth). Returns true on a 2xx.
pub fn health(base_url: String) -> Result<bool> {
    let resp = reqwest::blocking::Client::new()
        .get(format!("{}/health", base(&base_url)))
        .send()
        .context("health request failed")?;
    Ok(resp.status().is_success())
}

/// Authenticated tmux probe: `GET {base_url}/api/health/tmux`. 200 → true,
/// 503 → false. Doubles as an auth check — a 401 surfaces as an error to Dart.
pub fn health_tmux(base_url: String, token: String) -> Result<bool> {
    let resp = reqwest::blocking::Client::new()
        .get(format!("{}/api/health/tmux", base(&base_url)))
        .bearer_auth(token)
        .send()
        .context("tmux health request failed")?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        anyhow::bail!("authentication failed (check your token)");
    }
    Ok(resp.status().is_success())
}

/// `GET {base_url}/api/sessions?include_stopped=` → the session list, decoded
/// into the shared `SessionInfo` wire type (frb mirrors it to a Dart class).
pub fn list_sessions(
    base_url: String,
    token: String,
    include_stopped: bool,
) -> Result<Vec<SessionInfo>> {
    let resp = client()
        .get(format!(
            "{}/api/sessions?include_stopped={}",
            base(&base_url),
            include_stopped
        ))
        .bearer_auth(token)
        .send()
        .context("list_sessions request failed")?;
    let sessions = ok_or_status(resp, "list_sessions")?
        .json::<Vec<SessionInfo>>()
        .context("response did not match the SessionInfo contract")?;
    Ok(sessions)
}

/// `GET {base_url}/api/sessions/{query}/detail?lines=` → a session's live
/// detail (agent state, diff summary, pane snapshot). `query` is matched
/// loosely server-side (a full id, branch, or title prefix). A 404 (no match)
/// returns `None` rather than an error, so a deleted session reads as "gone".
pub fn get_session_detail(
    base_url: String,
    token: String,
    query: String,
    lines: Option<u32>,
) -> Result<Option<SessionDetail>> {
    let mut url = session_url(&base_url, &query, "detail")?;
    if let Some(n) = lines {
        url.query_pairs_mut().append_pair("lines", &n.to_string());
    }
    let resp = client()
        .get(url)
        .bearer_auth(token)
        .send()
        .context("get_session_detail request failed")?;
    if resp.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let detail = ok_or_status(resp, "get_session_detail")?
        .json::<SessionDetail>()
        .context("response did not match the SessionDetail contract")?;
    Ok(Some(detail))
}

/// `GET {base_url}/api/sessions/{query}/pane?lines=` → the raw captured pane
/// text. Lighter than `get_session_detail` for polling a preview. `None` on a
/// 404.
pub fn get_pane(
    base_url: String,
    token: String,
    query: String,
    lines: Option<u32>,
) -> Result<Option<String>> {
    let mut url = session_url(&base_url, &query, "pane")?;
    if let Some(n) = lines {
        url.query_pairs_mut().append_pair("lines", &n.to_string());
    }
    let resp = client()
        .get(url)
        .bearer_auth(token)
        .send()
        .context("get_pane request failed")?;
    if resp.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let text = ok_or_status(resp, "get_pane")?
        .text()
        .context("could not read pane response body")?;
    Ok(Some(text))
}

/// `POST {base_url}/api/sessions` → create a session, returning the new id.
///
/// `project_path` is a path on the *server's* filesystem (the repo to branch
/// from). The optional fields map straight onto [`CreateSessionOpts`]; absent
/// ones let the server apply its defaults.
#[allow(clippy::too_many_arguments)]
pub fn create_session(
    base_url: String,
    token: String,
    project_path: String,
    title: String,
    program: Option<String>,
    initial_prompt: Option<String>,
    effort: Option<String>,
    mode: Option<String>,
    base_branch: Option<String>,
) -> Result<String> {
    let opts = CreateSessionOpts {
        project_path: PathBuf::from(project_path),
        title,
        program,
        initial_prompt,
        effort,
        mode,
        base_branch,
        section: None,
    };
    let resp = client()
        .post(format!("{}/api/sessions", base(&base_url)))
        .bearer_auth(token)
        .json(&opts)
        .send()
        .context("create_session request failed")?;
    let body: serde_json::Value = ok_or_status(resp, "create_session")?
        .json()
        .context("could not read create_session response body")?;
    parse_created_id(&body)
}

/// Pull the new session id out of `POST /sessions`'s `{ "id": ... }` body.
fn parse_created_id(body: &serde_json::Value) -> Result<String> {
    body.get("id")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .context("create_session response was missing the new session id")
}

/// `POST {base_url}/api/sessions/{id}/kill` — stop a running session (204).
pub fn kill_session(base_url: String, token: String, id: String) -> Result<()> {
    let resp = client()
        .post(format!("{}/api/sessions/{}/kill", base(&base_url), id))
        .bearer_auth(token)
        .send()
        .context("kill_session request failed")?;
    ok_or_status(resp, "kill_session")?;
    Ok(())
}

/// `POST {base_url}/api/sessions/{id}/restart` — restart a session (204).
pub fn restart_session(base_url: String, token: String, id: String) -> Result<()> {
    let resp = client()
        .post(format!("{}/api/sessions/{}/restart", base(&base_url), id))
        .bearer_auth(token)
        .send()
        .context("restart_session request failed")?;
    ok_or_status(resp, "restart_session")?;
    Ok(())
}

/// `DELETE {base_url}/api/sessions/{id}` — delete a session and its worktree
/// (204).
pub fn delete_session(base_url: String, token: String, id: String) -> Result<()> {
    let resp = client()
        .delete(format!("{}/api/sessions/{}", base(&base_url), id))
        .bearer_auth(token)
        .send()
        .context("delete_session request failed")?;
    ok_or_status(resp, "delete_session")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_trims_trailing_slash() {
        assert_eq!(base("http://host:1234/"), "http://host:1234");
        assert_eq!(base("http://host:1234"), "http://host:1234");
    }

    #[test]
    fn parse_created_id_extracts_id() {
        let body = serde_json::json!({ "id": "abc-123" });
        assert_eq!(parse_created_id(&body).unwrap(), "abc-123");
    }

    #[test]
    fn parse_created_id_missing_field_errors() {
        // A success body without an `id` (or with a non-string id) is a contract
        // violation, not a silent empty string.
        assert!(parse_created_id(&serde_json::json!({})).is_err());
        assert!(parse_created_id(&serde_json::json!({ "id": 42 })).is_err());
    }

    /// The loose `query` (a branch or title prefix, per the server's resolve)
    /// must travel as ONE percent-encoded path segment: a raw '/' would add a
    /// bogus segment, and '?'/'#' would truncate the path into a query string
    /// or fragment — turning a live session into a phantom 404.
    #[test]
    fn session_url_encodes_query_as_single_segment() {
        let url = session_url("http://host:7878/", "feature/login", "detail").unwrap();
        assert_eq!(
            url.as_str(),
            "http://host:7878/api/sessions/feature%2Flogin/detail"
        );

        let url = session_url("http://host:7878", "fix? #123", "pane").unwrap();
        let s = url.as_str();
        assert!(
            !s.contains('?') && !s.contains('#'),
            "reserved chars must be encoded, got {s}"
        );
        assert!(s.ends_with("/pane"), "leaf must survive: {s}");
    }
}
