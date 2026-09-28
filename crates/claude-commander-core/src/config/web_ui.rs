//! The `[web_ui]` table: the browser UI the TUI can serve alongside its
//! embedded API server.
//!
//! Core models it for the same reason it models `[server]`
//! ([`ServerConfig`](super::ServerConfig)): `ConfigStore::mutate` re-serialises
//! the whole [`Config`](super::Config), so a table core does not know about is
//! deleted by the next settings edit. The web tier itself lives in
//! `claude-commander-web`, which never links core; the `claude-commander`
//! binary translates these settings into that crate's plain arguments.

use std::net::{IpAddr, Ipv4Addr};

use serde::{Deserialize, Serialize};

/// The default Basic-auth username.
fn default_username() -> String {
    "admin".to_string()
}

/// Web UI settings, persisted as the `[web_ui]` table of `config.toml`:
///
/// ```toml
/// [web_ui]
/// auto_start = true
/// bind = "0.0.0.0"
/// port = 8420
/// username = "admin"
/// password = "..."
/// ```
///
/// `Debug` is hand-written to redact `password`, for the reason given on
/// [`ServerConfig`](super::ServerConfig)'s: redacting at the value makes every
/// future `{:?}` safe without its author having to know.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebUiConfig {
    /// Serve the web UI in-process when the TUI launches. It proxies to the
    /// embedded API server, so turning this on also starts that server (on its
    /// own `[server]` bind, loopback by default). Off by default. Read once at
    /// startup, so a change needs a restart.
    pub auto_start: bool,
    /// Interface to bind. Defaults to `127.0.0.1`; set `0.0.0.0` to reach it
    /// from another machine (and put it behind TLS on an untrusted network).
    pub bind: IpAddr,
    /// Port to listen on. Defaults to `8420`.
    pub port: u16,
    /// Basic-auth username the browser logs in with. Defaults to `admin`.
    pub username: String,
    /// Basic-auth password. Required: with none set the web UI refuses to
    /// start rather than serving the API's bearer token to anyone who asks.
    pub password: Option<String>,
}

impl Default for WebUiConfig {
    fn default() -> Self {
        Self {
            auto_start: false,
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 8420,
            username: default_username(),
            password: None,
        }
    }
}

impl WebUiConfig {
    /// The configured password, treating a hand-edited `password = ""` (or
    /// whitespace) as absent — an empty password must never authenticate.
    pub fn password(&self) -> Option<&str> {
        self.password.as_deref().filter(|p| !p.trim().is_empty())
    }
}

impl std::fmt::Debug for WebUiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebUiConfig")
            .field("auto_start", &self.auto_start)
            .field("bind", &self.bind)
            .field("port", &self.port)
            .field("username", &self.username)
            .field(
                "password",
                &self
                    .password
                    .as_ref()
                    .map(|_| "<redacted>")
                    .unwrap_or("None"),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_off_and_loopback() {
        let c = WebUiConfig::default();
        assert!(!c.auto_start);
        assert!(c.bind.is_loopback());
        assert_eq!(c.port, 8420);
        assert_eq!(c.username, "admin");
        assert!(c.password.is_none());
    }

    #[test]
    fn debug_redacts_the_password() {
        let c = WebUiConfig {
            password: Some("hunter2".into()),
            ..Default::default()
        };
        let rendered = format!("{c:?}");
        assert!(!rendered.contains("hunter2"), "password leaked: {rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }

    #[test]
    fn a_blank_password_counts_as_unset() {
        for blank in ["", "   "] {
            let c = WebUiConfig {
                password: Some(blank.into()),
                ..Default::default()
            };
            assert_eq!(c.password(), None, "{blank:?} must not authenticate");
        }
        let set = WebUiConfig {
            password: Some("pw".into()),
            ..Default::default()
        };
        assert_eq!(set.password(), Some("pw"));
    }

    /// A partial table fills the rest from defaults, so a hand-written
    /// `[web_ui]` with only a password in it still binds somewhere sane.
    #[test]
    fn a_partial_table_takes_defaults() {
        let c: WebUiConfig = toml::from_str("password = \"pw\"\n").unwrap();
        assert_eq!(c.port, 8420);
        assert_eq!(c.username, "admin");
        assert_eq!(c.password(), Some("pw"));
    }
}
