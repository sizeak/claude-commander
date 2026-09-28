//! Running the HTTP server inside the TUI process.
//!
//! Wanting the server on the same machine as the TUI is the common case, so
//! `[server] auto_start` (or `--serve`) brings it up with the TUI and takes it
//! down when the TUI exits. This binary is the only crate that depends on both
//! `claude-commander-tui` and `claude-commander-server`, which is why the glue
//! lives here: the terminal frontend must not compile axum, and the server must
//! not compile ratatui.
//!
//! The decisions are pure functions ([`should_serve`], [`plan`]) so they are
//! testable without a socket; [`start`] is the only part that touches the
//! network.

use std::path::Path;

use claude_commander_core::Config;
use claude_commander_core::api::CommanderService;
use claude_commander_core::config::ServerConfig;
use claude_commander_server::auth::AuthConfig;
use claude_commander_server::embed::{self, EmbeddedServer, TokenDecision};
use claude_commander_tui::EmbeddedServerStatus;
use tracing::{info, warn};

/// Whether this run should serve.
///
/// `[server] auto_start` is the persistent answer; `--serve` and `--no-serve`
/// override it for one run. Both flags at once is rejected by clap
/// (`conflicts_with`), so `no_serve` winning here is only a belt-and-braces
/// tie-break rather than a real precedence rule.
pub fn should_serve(auto_start: bool, serve: bool, no_serve: bool) -> bool {
    if no_serve {
        return false;
    }
    serve || auto_start
}

/// Everything needed to start serving, resolved but not yet acted on.
pub struct ServePlan {
    /// The effective `[server]` settings, with `CC_SERVER_*` applied.
    pub cfg: ServerConfig,
    /// The authentication policy to serve under.
    pub auth: AuthConfig,
    /// The bearer token, for showing to the operator.
    pub token: Option<String>,
    /// `Some` when the token was freshly generated and so must be written back
    /// to `config.toml` before the config store is built. A generated token that
    /// is *not* persisted would change on every launch, which would break every
    /// client the operator had already paired.
    pub persist_token: Option<String>,
}

/// Settle this run's token against already-resolved settings.
///
/// Pure: it decides what the token should be and whether it needs persisting,
/// but writes nothing. The caller owns the write, and must do it before
/// constructing the `ConfigStore` so the store's mtime cache does not then see
/// its own file as an external edit.
pub fn plan(cfg: ServerConfig) -> ServePlan {
    let (token, persist_token) = match embed::token_decision(&cfg) {
        TokenDecision::Existing(t) => (t, None),
        TokenDecision::Generated(t) => (t.clone(), Some(t)),
    };
    ServePlan {
        auth: AuthConfig::Token(token.clone()),
        token: Some(token),
        persist_token,
        cfg,
    }
}

/// Decide whether to serve this run and, if so, settle the token — persisting a
/// freshly generated one to `config_path` and mirroring it into `config` so the
/// `ConfigStore` built from it (and the settings tab reading it) agrees with the
/// file.
///
/// **Must be called before the `ConfigStore` is constructed.** The store caches
/// the config file's mtime to tell its own writes from a hand edit, so writing
/// the token behind its back afterwards would read as an external edit and
/// trigger a spurious reload.
///
/// Neither a malformed `[server]` config nor an unwritable config file is fatal:
/// the first means this run does not serve, the second means the token is only
/// good for this run. Both are logged rather than aborting a TUI the operator
/// launched to do something else.
pub fn prepare(
    config: &mut Config,
    config_path: &Path,
    serve: bool,
    no_serve: bool,
) -> Option<ServePlan> {
    // `--no-serve` needs no config at all, and short-circuiting here keeps the
    // "never generate a token we were told not to use" guarantee obvious.
    if no_serve {
        return None;
    }

    // Resolve BEFORE testing `auto_start`: the env layer can set it, and reading
    // it off the raw file would have made `CC_SERVER_AUTO_START` the one
    // `CC_SERVER_*` variable that did nothing. So this runs on every launch —
    // cheap (figment over one small struct), and the only way it says anything
    // is a genuinely malformed `CC_SERVER_*`, which is worth a log line whether
    // or not this particular run was going to serve.
    let cfg = match claude_commander_server::config::resolve(config.server.clone()) {
        Ok(cfg) => cfg,
        Err(e) => {
            warn!("[server] config could not be resolved, so not serving: {e}");
            return None;
        }
    };

    // Decide *after* the gate, so a run that will not serve never generates a
    // token it would only throw away.
    if !should_serve(cfg.auto_start, serve, no_serve) {
        return None;
    }
    let resolved = plan(cfg);

    if let Some(token) = &resolved.persist_token {
        if let Err(e) = claude_commander_core::config::persist_server_token(config_path, token) {
            warn!("generated server token could not be saved to {config_path:?}: {e}");
            warn!("the server is reachable with it this run, but it will change on the next");
        }
        config.server.token = Some(token.clone());
    }

    Some(resolved)
}

/// Bind and serve, sharing the TUI's own service.
///
/// Returns the guard to hold for the process's lifetime (dropping it stops the
/// server) alongside the status to show in the TUI. A failure is **not** fatal:
/// the overwhelmingly likely cause is that the port is already taken because a
/// server is already running, and killing the TUI over that would be absurd.
pub async fn start(
    service: CommanderService,
    plan: ServePlan,
) -> (Option<EmbeddedServer>, EmbeddedServerStatus) {
    let ServePlan {
        cfg, auth, token, ..
    } = plan;
    match embed::start(service, &cfg, auth).await {
        Ok(server) => {
            let url = server.url();
            info!("serving the commander API on {url}");
            (Some(server), EmbeddedServerStatus::Listening { url, token })
        }
        Err(e) => {
            let reason = e.to_string();
            warn!("embedded server not started: {reason}");
            (None, EmbeddedServerStatus::Failed { reason })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `prepare` goes through `claude_commander_server::config::resolve`, which
    // reads `CC_SERVER_*` from the real process environment. These tests
    // therefore assume none are set — true in CI and under `verify.sh` — and is
    // why none of them try to *exercise* the env layer, which would make its
    // neighbours order-dependent, the process environment being global.

    #[test]
    fn auto_start_serves_without_a_flag() {
        assert!(should_serve(true, false, false));
        assert!(!should_serve(false, false, false));
    }

    #[test]
    fn the_flags_override_the_config() {
        // --serve turns it on for a run where config says no.
        assert!(should_serve(false, true, false));
        // --no-serve turns it off for a run where config says yes.
        assert!(!should_serve(true, false, true));
    }

    #[test]
    fn a_configured_token_is_not_rewritten() {
        let base = ServerConfig {
            token: Some("configured".into()),
            ..Default::default()
        };
        let plan = plan(base);
        assert_eq!(plan.token.as_deref(), Some("configured"));
        assert!(
            plan.persist_token.is_none(),
            "an existing token must not be written back"
        );
    }

    /// A generated token has to be persisted, or every restart invalidates the
    /// token each paired client is holding.
    #[test]
    fn a_generated_token_is_flagged_for_persistence() {
        let plan = plan(ServerConfig::default());
        let token = plan.token.clone().expect("a token is always resolved");
        assert_eq!(plan.persist_token.as_deref(), Some(token.as_str()));
        assert_eq!(token.len(), 64);
    }

    /// The embedded server never runs unauthenticated: there is no `--serve`
    /// equivalent of the standalone binary's `--allow-no-auth`, because the TUI
    /// has no way to make that choice deliberate.
    #[test]
    fn the_embedded_server_always_requires_a_token() {
        let plan = plan(ServerConfig::default());
        assert!(matches!(plan.auth, AuthConfig::Token(_)));
    }

    /// A config with a `[server]` table but no token: `prepare` generates one,
    /// writes it to the file, and mirrors it into the in-memory config so the
    /// `ConfigStore` built next agrees with what is on disk.
    #[test]
    fn prepare_persists_a_generated_token_and_mirrors_it_into_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[server]\nauto_start = true\n").unwrap();
        let mut config = Config {
            server: ServerConfig {
                auto_start: true,
                ..Default::default()
            },
            ..Default::default()
        };

        let plan = prepare(&mut config, &path, false, false).expect("auto_start means serve");
        let token = plan.token.clone().unwrap();

        assert_eq!(
            config.server.token.as_deref(),
            Some(token.as_str()),
            "the in-memory config must match the file, or the settings tab lies"
        );
        let on_disk = Config::load_from_path(&path).unwrap();
        assert_eq!(on_disk.server.token.as_deref(), Some(token.as_str()));
        assert!(on_disk.server.auto_start, "the existing table survives");
    }

    /// A token already in the file is left exactly as it is — `prepare` must not
    /// rewrite config.toml on every launch.
    #[test]
    fn prepare_does_not_touch_the_file_when_a_token_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = "[server]\nauto_start = true\ntoken = \"configured\"\n";
        std::fs::write(&path, original).unwrap();
        let mut config = Config {
            server: ServerConfig {
                auto_start: true,
                token: Some("configured".into()),
                ..Default::default()
            },
            ..Default::default()
        };

        let plan = prepare(&mut config, &path, false, false).expect("auto_start means serve");
        assert_eq!(plan.token.as_deref(), Some("configured"));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original,
            "config.toml must be byte-identical when nothing needed writing"
        );
    }

    /// Not serving must not generate or persist anything: a user who never turns
    /// the server on should never find a bearer token in their config file.
    #[test]
    fn prepare_writes_nothing_when_not_serving() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "branch_prefix = \"wt/\"\n").unwrap();
        let mut config = Config::default();

        assert!(prepare(&mut config, &path, false, false).is_none());
        assert!(config.server.token.is_none());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "branch_prefix = \"wt/\"\n"
        );
    }

    /// `--no-serve` wins over `auto_start`, and must not leave a token behind
    /// either.
    #[test]
    fn prepare_respects_no_serve() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[server]\nauto_start = true\n").unwrap();
        let mut config = Config {
            server: ServerConfig {
                auto_start: true,
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(prepare(&mut config, &path, false, true).is_none());
        assert!(config.server.token.is_none());
        assert!(!std::fs::read_to_string(&path).unwrap().contains("token"));
    }

    /// An unwritable config file must not stop the server: the token is simply
    /// good for this run only.
    #[test]
    fn prepare_still_serves_when_the_token_cannot_be_saved() {
        let dir = tempfile::tempdir().unwrap();
        // A path whose parent is a file, so the write cannot succeed.
        let blocker = dir.path().join("not-a-dir");
        std::fs::write(&blocker, "").unwrap();
        let path = blocker.join("config.toml");
        let mut config = Config {
            server: ServerConfig {
                auto_start: true,
                ..Default::default()
            },
            ..Default::default()
        };

        let plan = prepare(&mut config, &path, false, false)
            .expect("an unwritable config must not cancel the server");
        assert!(plan.token.is_some());
        assert!(config.server.token.is_some());
    }
}
