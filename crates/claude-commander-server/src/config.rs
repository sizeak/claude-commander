//! Server configuration.
//!
//! `ServerConfig` lives in the server crate, **not** in core — the core library
//! stays completely server-agnostic (the dependency direction is server → core,
//! never the reverse). The server reuses only core's *path* helper
//! ([`Config::config_file_path`]) to locate the shared `config.toml`, then loads
//! its own `[server]` table with its own figment stack:
//!
//! `Serialized::defaults(ServerConfig::default())` → the `[server]` table of the
//! TOML → environment (`CC_SERVER_*`) → CLI flags.
//!
//! Core's `Config` uses `#[serde(default)]` (not `deny_unknown_fields`), so a
//! `[server]` section in `config.toml` is silently ignored by the TUI's config
//! extraction — verified by a test below.

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use claude_commander_core::Config;
use figment::{
    Figment,
    providers::{Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};

/// The default loopback bind address.
fn default_bind() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

/// The default listen port.
fn default_port() -> u16 {
    7878
}

/// Server configuration, loaded from the `[server]` table of `config.toml`,
/// layered with environment variables and CLI flags.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Interface to bind. Defaults to `127.0.0.1` (loopback only).
    pub bind: IpAddr,
    /// Port to listen on. Defaults to `7878`.
    pub port: u16,
    /// Pre-shared bearer token. `None` means "no token configured" — the
    /// server then auto-generates one on first run (see token resolution in
    /// `main.rs`) unless `--allow-no-auth` is set.
    pub token: Option<String>,
    /// TLS certificate path (PEM). Only used when the `tls` feature is built.
    pub tls_cert_path: Option<PathBuf>,
    /// TLS private-key path (PEM). Only used when the `tls` feature is built.
    pub tls_key_path: Option<PathBuf>,
    /// CORS allowlist of permitted origins. Empty means same-origin/deny
    /// (browsers can't call `/api` cross-origin unless listed here).
    pub cors_allowed_origins: Vec<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            port: default_port(),
            token: None,
            tls_cert_path: None,
            tls_key_path: None,
            cors_allowed_origins: Vec::new(),
        }
    }
}

impl ServerConfig {
    /// Load the `[server]` table from the shared `config.toml`, layered over the
    /// defaults and any `CC_SERVER_*` environment overrides. Missing file or
    /// missing `[server]` table both fall back to defaults.
    pub fn load() -> Result<Self, Box<figment::Error>> {
        let path = Config::config_file_path().unwrap_or_default();
        Self::load_from(&path)
    }

    /// Load from an explicit config-file path (used by tests).
    pub fn load_from(config_path: &std::path::Path) -> Result<Self, Box<figment::Error>> {
        // The shared `config.toml` has core's `Config` keys at the top level
        // (mostly scalars) alongside a `[server]` table. We only want the
        // `[server]` sub-table, with `ServerConfig::default()` filling the rest
        // and `CC_SERVER_*` env vars overriding. A `Wrapper` whose only field is
        // `server` lets serde drop every other top-level key (it isn't
        // `deny_unknown_fields`), so the core keys are ignored cleanly.
        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            #[serde(default)]
            server: ServerConfig,
        }

        let wrapper: Wrapper = Figment::from(Serialized::defaults(Wrapper {
            server: ServerConfig::default(),
        }))
        .merge(Toml::file(config_path))
        // Env overrides are namespaced under `server.` so they land in the
        // wrapped struct (e.g. `CC_SERVER_TOKEN` → `server.token`).
        .merge(
            figment::providers::Env::prefixed("CC_SERVER_").map(|k| format!("server.{k}").into()),
        )
        .extract()
        .map_err(Box::new)?;

        Ok(wrapper.server)
    }
}

/// Reject the dangerous `--allow-no-auth` on a non-loopback bind.
///
/// Disabling authentication is only ever safe on a loopback interface; on any
/// routable address it would expose an unauthenticated session-control API to
/// the network. Returns `Err` with an operator-facing message when
/// `allow_no_auth` is set against a non-loopback `bind`; `Ok` otherwise
/// (loopback + no-auth, or any bind with a token).
pub fn check_no_auth_bind(bind: IpAddr, allow_no_auth: bool) -> Result<(), String> {
    if allow_no_auth && !bind.is_loopback() {
        Err(format!(
            "--allow-no-auth refuses to run on non-loopback bind {bind}: an unauthenticated \
             API would be exposed to the network. Bind to a loopback address (127.0.0.1 / ::1) \
             or configure a token instead."
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::Ipv6Addr;

    #[test]
    fn no_auth_on_loopback_is_ok() {
        assert!(check_no_auth_bind(IpAddr::V4(Ipv4Addr::LOCALHOST), true).is_ok());
        assert!(check_no_auth_bind(IpAddr::V6(Ipv6Addr::LOCALHOST), true).is_ok());
    }

    #[test]
    fn no_auth_on_non_loopback_is_rejected() {
        let err = check_no_auth_bind(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), true)
            .expect_err("0.0.0.0 + --allow-no-auth must be rejected");
        assert!(
            err.contains("loopback"),
            "message should explain why: {err}"
        );
    }

    #[test]
    fn token_on_non_loopback_is_ok() {
        // No `--allow-no-auth` → a token is in force, so any bind is allowed.
        assert!(check_no_auth_bind(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), false).is_ok());
    }

    #[test]
    fn defaults_are_loopback_7878_no_token() {
        let cfg = ServerConfig::default();
        assert_eq!(cfg.bind, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(cfg.port, 7878);
        assert!(cfg.token.is_none());
        assert!(cfg.cors_allowed_origins.is_empty());
    }

    #[test]
    fn missing_file_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.toml");
        let cfg = ServerConfig::load_from(&path).unwrap();
        assert_eq!(cfg.port, 7878);
        assert!(cfg.token.is_none());
    }

    #[test]
    fn server_table_overrides_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "[[programs]]\nlabel = \"Claude\"\ncommand = \"claude\"\n\n[server]\nport = 9999\ntoken = \"sekret\"\n"
        )
        .unwrap();

        let cfg = ServerConfig::load_from(&path).unwrap();
        assert_eq!(cfg.port, 9999);
        assert_eq!(cfg.token.as_deref(), Some("sekret"));
        // Unspecified fields keep their defaults.
        assert_eq!(cfg.bind, IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    /// The same `config.toml` carrying a `[server]` table must still parse as
    /// core's `Config` — i.e. core ignores the unknown table rather than
    /// rejecting it. This is the contract that lets one file serve both.
    #[test]
    fn core_config_ignores_unknown_server_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "[[programs]]\nlabel = \"Claude\"\ncommand = \"claude\"\n\n[server]\nport = 9999\ntoken = \"sekret\"\n"
        )
        .unwrap();

        // Extract core's Config from the same TOML; the `[server]` table must
        // not cause an error (core does not use `deny_unknown_fields`).
        let core: Config = Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file(&path))
            .extract()
            .expect("core Config must ignore the unknown [server] table");
        assert_eq!(core.default_session_program(), "claude");
    }
}
