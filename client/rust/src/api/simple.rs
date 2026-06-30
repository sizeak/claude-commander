//! HTTP client API exposed to Flutter via flutter_rust_bridge.
//!
//! Each `pub fn` here becomes an async-callable function on the Dart side (frb
//! runs the Rust body on a worker thread, so blocking reqwest calls don't block
//! the UI isolate). Responses are validated against the shared
//! `claude-commander-protocol` types, so any drift from the server fails here in
//! Rust rather than silently in the UI.

use anyhow::{Context, Result};
use claude_commander_protocol::api::SessionInfo;

#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    // Default utilities (logging, panic backtraces) for the bridge.
    flutter_rust_bridge::setup_default_user_utils();
}

/// Trim a trailing slash so `{base}/path` joins cleanly.
fn base(base_url: &str) -> &str {
    base_url.trim_end_matches('/')
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
    let sessions = reqwest::blocking::Client::new()
        .get(format!(
            "{}/api/sessions?include_stopped={}",
            base(&base_url),
            include_stopped
        ))
        .bearer_auth(token)
        .send()
        .context("list_sessions request failed")?
        .error_for_status()
        .context("server returned an error status")?
        .json::<Vec<SessionInfo>>()
        .context("response did not match the SessionInfo contract")?;
    Ok(sessions)
}
