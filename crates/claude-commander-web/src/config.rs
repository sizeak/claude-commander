//! Resolved runtime configuration + shared handler state.
//!
//! The web binary is a thin *client* of `claude-commander-server`: it serves the
//! browser SPA and reverse-proxies `/api` + `/ws/attach` upstream. Its auth mode
//! is chosen by whether a commander token was supplied at launch:
//!
//! - **[`AuthMode::Bff`]** (token provided): the browser logs in with HTTP Basic
//!   auth; we inject `Authorization: Bearer <token>` on every upstream call and
//!   the in-band `auth` frame on the WebSocket. The token never reaches the
//!   browser.
//! - **[`AuthMode::PassThrough`]** (no token): the browser supplies the commander
//!   token itself (connect screen); we forward it upstream verbatim.

use std::sync::Arc;

/// How the web tier authenticates the browser and the upstream server.
#[derive(Debug, Clone)]
pub enum AuthMode {
    /// Backend-for-frontend: gate the browser with Basic auth, inject the bearer
    /// token upstream. The token stays server-side.
    Bff {
        username: String,
        password: String,
        token: String,
    },
    /// Transparent proxy: the browser carries the commander token itself (bearer
    /// header on `/api`, in-band `auth` frame on the WS), and we forward it.
    PassThrough,
}

impl AuthMode {
    /// The mode label surfaced to the SPA via `/webui/config` so the frontend
    /// knows whether to show a password login (`bff`) or a token connect screen
    /// (`direct`).
    pub fn label(&self) -> &'static str {
        match self {
            AuthMode::Bff { .. } => "bff",
            AuthMode::PassThrough => "direct",
        }
    }
}

/// Shared state handed to every handler. Cheap to clone (all `Arc`/`Client`).
#[derive(Clone)]
pub struct AppState {
    /// Reusable upstream HTTP client for the `/api` reverse proxy.
    pub http: reqwest::Client,
    /// Base URL of `claude-commander-server`, e.g. `http://127.0.0.1:7878`,
    /// with no trailing slash.
    pub commander_url: Arc<str>,
    /// Resolved auth policy.
    pub auth: Arc<AuthMode>,
}

impl AppState {
    /// Build the upstream URL for an `/api` path (`path` includes the leading
    /// `/api/...`) plus an optional raw query string.
    pub fn upstream_api_url(&self, path_and_query: &str) -> String {
        format!("{}{}", self.commander_url, path_and_query)
    }

    /// Build the upstream WebSocket URL for `/ws/attach`, translating the
    /// `http(s)` base to `ws(s)`.
    pub fn upstream_ws_url(&self) -> String {
        let base = self.commander_url.as_ref();
        let ws = if let Some(rest) = base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = base.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            // Assume already a ws(s) scheme or bare host; pass through.
            base.to_string()
        };
        format!("{ws}/ws/attach")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(url: &str) -> AppState {
        AppState {
            http: reqwest::Client::new(),
            commander_url: Arc::from(url),
            auth: Arc::new(AuthMode::PassThrough),
        }
    }

    #[test]
    fn ws_url_translates_scheme() {
        assert_eq!(
            state("http://127.0.0.1:7878").upstream_ws_url(),
            "ws://127.0.0.1:7878/ws/attach"
        );
        assert_eq!(
            state("https://host.ts.net").upstream_ws_url(),
            "wss://host.ts.net/ws/attach"
        );
    }

    #[test]
    fn api_url_joins_path_and_query() {
        assert_eq!(
            state("http://127.0.0.1:7878").upstream_api_url("/api/sessions?all=true"),
            "http://127.0.0.1:7878/api/sessions?all=true"
        );
    }

    #[test]
    fn mode_label() {
        assert_eq!(AuthMode::PassThrough.label(), "direct");
        assert_eq!(
            AuthMode::Bff {
                username: "admin".into(),
                password: "pw".into(),
                token: "t".into()
            }
            .label(),
            "bff"
        );
    }
}
