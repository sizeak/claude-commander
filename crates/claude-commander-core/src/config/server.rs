//! The `[server]` table: how this machine exposes its sessions over HTTP.
//!
//! This lives in core rather than in `claude-commander-server` for two reasons,
//! both of which are about *who writes the file* rather than who serves the
//! requests:
//!
//! 1. [`ConfigStore::mutate`](super::ConfigStore::mutate) persists by
//!    re-serialising the whole [`Config`](super::Config), so a table core does
//!    not model is **deleted** on the next write. While `[server]` lived only in
//!    the server crate, any settings-modal edit (or a `PATCH /config`) silently
//!    wiped the operator's bind address and bearer token.
//! 2. The settings UI that edits these values lives in `claude-commander-tui`,
//!    which must not depend on the server crate (that would pull axum/tower into
//!    the terminal frontend).
//!
//! Nothing about the *server* moved: the router, auth middleware, handlers and
//! the `check_no_auth_bind` policy all stay in `claude-commander-server`, and the
//! dependency direction is still server → core. What core owns is the persisted
//! data and its defaults. The server crate layers `CC_SERVER_*` environment
//! overrides and its own CLI flags on top (see `ServerConfig::resolve` there).

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The default loopback bind address.
fn default_bind() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

/// The default listen port.
fn default_port() -> u16 {
    7878
}

/// Server settings, persisted as the `[server]` table of `config.toml`:
///
/// ```toml
/// [server]
/// auto_start = true
/// bind = "127.0.0.1"
/// port = 7878
/// token = "..."
/// ```
///
/// `Debug` is hand-written to redact `token`, matching
/// [`RemoteServerConfig`](super::RemoteServerConfig). This is prophylactic
/// rather than a fix for a known leak — no `{:?}` of the enclosing `Config` was
/// found in core when this was written — and that is the point: redacting at the
/// value means the next `{:?}` anyone adds, anywhere, is safe without their
/// having to know.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Start the HTTP server in-process when the TUI launches, and take it down
    /// when the TUI exits. Off by default — a listening socket is opt-in.
    ///
    /// Overridable per-run by the `claude-commander` binary's `--serve` /
    /// `--no-serve` flags. Read once at startup, so a change needs a restart.
    pub auto_start: bool,
    /// Interface to bind. Defaults to `127.0.0.1` (loopback only), so the
    /// default configuration is not reachable from the network.
    pub bind: IpAddr,
    /// Port to listen on. Defaults to `7878`.
    pub port: u16,
    /// Pre-shared bearer token. `None` means "no token configured": the
    /// standalone server then generates a one-time token and logs it, while the
    /// TUI's embedded server generates one and persists it back here, so a
    /// client only has to be paired once.
    pub token: Option<String>,
    /// TLS certificate path (PEM). Only used when the server crate's `tls`
    /// feature is built.
    pub tls_cert_path: Option<PathBuf>,
    /// TLS private-key path (PEM). Only used when the server crate's `tls`
    /// feature is built.
    pub tls_key_path: Option<PathBuf>,
    /// CORS allowlist of permitted origins. Empty means same-origin/deny
    /// (browsers can't call `/api` cross-origin unless listed here).
    pub cors_allowed_origins: Vec<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            auto_start: false,
            bind: default_bind(),
            port: default_port(),
            token: None,
            tls_cert_path: None,
            tls_key_path: None,
            cors_allowed_origins: Vec::new(),
        }
    }
}

impl std::fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the token — redact its presence, not its value.
        f.debug_struct("ServerConfig")
            .field("auto_start", &self.auto_start)
            .field("bind", &self.bind)
            .field("port", &self.port)
            .field(
                "token",
                &self.token.as_ref().map(|_| "<redacted>").unwrap_or("None"),
            )
            .field("tls_cert_path", &self.tls_cert_path)
            .field("tls_key_path", &self.tls_key_path)
            .field("cors_allowed_origins", &self.cors_allowed_origins)
            .finish()
    }
}

/// Write `token` into the `[server]` table of the `config.toml` at
/// `config_path`, leaving the rest of the file — including comments and
/// formatting — exactly as it was.
///
/// This is surgical rather than a whole-config re-serialise on purpose. The
/// generated token is persisted *automatically*, on a launch where the operator
/// did nothing but start the TUI, so it must not be the thing that silently
/// strips the comments out of a hand-written config file. (`ConfigStore::mutate`
/// does re-serialise wholesale, but only in response to a deliberate settings
/// edit.)
///
/// Goes through [`write_private_file`](super::write_private_file), so the file
/// stays `0o600` and the write is atomic — the same discipline every other
/// token-bearing write here uses.
pub fn persist_token(config_path: &std::path::Path, token: &str) -> crate::error::Result<()> {
    use crate::error::ConfigError;

    // A missing file is not an error: the token is simply the first thing in it.
    let existing = std::fs::read_to_string(config_path).unwrap_or_default();
    let mut doc = existing
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| ConfigError::SaveFailed(format!("config.toml is not valid TOML: {e}")))?;

    // `contains_table`/`Item::is_table` are true only for a `[server]` *header*
    // table, so testing that and replacing on false would delete a perfectly
    // valid inline `server = { bind = "0.0.0.0", port = 9999 }` — the exact data
    // loss this function exists to prevent (receipt: toml_edit-0.25.12
    // `item.rs:209` `is_table` → `as_table`, which is `None` for
    // `Value::InlineTable`; pinned by
    // `persist_token_preserves_an_inline_server_table` below). So insert only
    // when the key is genuinely absent, and edit whatever is there through
    // `as_table_like_mut`, which accepts both spellings (`item.rs:311-317`).
    let server = doc
        .as_table_mut()
        .entry("server")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    match server.as_table_like_mut() {
        Some(table) => {
            table.insert("token", toml_edit::value(token));
        }
        None => {
            // `server` is present but not a table at all (`server = 3`). Core's
            // loader would have rejected the file before we got here, so this is
            // unreachable in practice; replacing is the only sane repair.
            let mut table = toml_edit::Table::new();
            table.insert("token", toml_edit::value(token));
            *server = toml_edit::Item::Table(table);
        }
    }

    super::write_private_file(config_path, doc.to_string())
        .map_err(|e| ConfigError::SaveFailed(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_off_loopback_7878_no_token() {
        let cfg = ServerConfig::default();
        assert!(!cfg.auto_start, "a listening socket must be opt-in");
        assert_eq!(cfg.bind, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(cfg.port, 7878);
        assert!(cfg.token.is_none());
        assert!(cfg.cors_allowed_origins.is_empty());
    }

    #[test]
    fn debug_redacts_the_token() {
        let cfg = ServerConfig {
            token: Some("sekret".into()),
            ..Default::default()
        };
        let rendered = format!("{cfg:?}");
        assert!(
            !rendered.contains("sekret"),
            "token leaked through Debug: {rendered}"
        );
        assert!(rendered.contains("<redacted>"), "{rendered}");

        // Absence is reported as absence, not as a redacted value.
        let empty = format!("{:?}", ServerConfig::default());
        assert!(empty.contains("None"), "{empty}");
    }

    /// Persisting a generated token happens on a launch where the operator did
    /// nothing but start the TUI, so it must not strip their comments or reorder
    /// their file.
    #[test]
    fn persist_token_preserves_comments_and_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = "\
# My carefully commented config.
branch_prefix = \"wt/\"  # trailing comment

[[programs]]
label = \"Claude\"
command = \"claude\"

[server]
# Reachable from the LAN so my phone can connect.
bind = \"0.0.0.0\"
port = 7878
";
        std::fs::write(&path, original).unwrap();

        persist_token(&path, "generated-token").unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(
            after.contains("# My carefully commented config."),
            "{after}"
        );
        assert!(after.contains("# trailing comment"), "{after}");
        assert!(
            after.contains("# Reachable from the LAN so my phone can connect."),
            "{after}"
        );
        assert!(after.contains("token = \"generated-token\""), "{after}");
        // And the existing values are untouched.
        let cfg: super::super::Config = super::super::Config::load_from_path(&path).unwrap();
        assert_eq!(cfg.server.bind.to_string(), "0.0.0.0");
        assert_eq!(cfg.branch_prefix, "wt/");
        assert_eq!(cfg.server.token.as_deref(), Some("generated-token"));
    }

    /// A config file with no `[server]` table yet gains one.
    #[test]
    fn persist_token_creates_a_missing_server_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "branch_prefix = \"wt/\"\n").unwrap();

        persist_token(&path, "generated-token").unwrap();

        let cfg = super::super::Config::load_from_path(&path).unwrap();
        assert_eq!(cfg.server.token.as_deref(), Some("generated-token"));
        assert_eq!(cfg.branch_prefix, "wt/");
    }

    /// An inline `server = { … }` table must be edited in place, not replaced.
    ///
    /// Regression test: `contains_table` is false for the inline spelling, so a
    /// "create it if missing" check written against that predicate silently
    /// deleted the operator's bind address and port — while writing a `[server ]`
    /// header in their place.
    #[test]
    fn persist_token_preserves_an_inline_server_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "branch_prefix = \"wt/\"\nserver = { bind = \"0.0.0.0\", port = 9999 }\n",
        )
        .unwrap();

        persist_token(&path, "generated").unwrap();

        let after = super::super::Config::load_from_path(&path).unwrap();
        assert_eq!(after.server.port, 9999, "port must survive");
        assert_eq!(
            after.server.bind.to_string(),
            "0.0.0.0",
            "bind must survive"
        );
        assert_eq!(after.server.token.as_deref(), Some("generated"));
        assert_eq!(after.branch_prefix, "wt/");
    }

    #[test]
    fn persist_token_rewrites_an_existing_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[server]\ntoken = \"old\"\n").unwrap();

        persist_token(&path, "new").unwrap();

        let cfg = super::super::Config::load_from_path(&path).unwrap();
        assert_eq!(cfg.server.token.as_deref(), Some("new"));
    }

    /// The file carries a bearer token, so it must not be group/world-readable.
    #[cfg(unix)]
    #[test]
    fn persist_token_keeps_the_file_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        persist_token(&path, "generated-token").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "config.toml must stay owner-only, got {mode:o}"
        );
    }
}
