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

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;

use claude_commander_core::Config;
use claude_commander_core::api::CommanderService;
use claude_commander_core::config::{ServerConfig, WebUiConfig};
use claude_commander_server::auth::AuthConfig;
use claude_commander_server::embed::{self, EmbeddedServer, TokenDecision};
use claude_commander_tui::EmbeddedServerStatus;
use claude_commander_web::{AuthMode, EmbeddedWeb};
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
    /// The `[web_ui]` settings when the web UI should be served beside the API
    /// this run, else `None`.
    pub web: Option<WebUiConfig>,
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
        web: None,
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
    // The web UI is only a proxy in front of the API, so asking for it asks
    // for the API too — on the API's own `[server]` bind, which stays loopback
    // unless the operator widened it.
    let web = config.web_ui.auto_start;
    if !should_serve(cfg.auto_start || web, serve, no_serve) {
        return None;
    }
    let mut resolved = plan(cfg);
    resolved.web = web.then(|| config.web_ui.clone());

    if let Some(token) = &resolved.persist_token {
        if let Err(e) = claude_commander_core::config::persist_server_token(config_path, token) {
            warn!("generated server token could not be saved to {config_path:?}: {e}");
            warn!("the server is reachable with it this run, but it will change on the next");
        }
        config.server.token = Some(token.clone());
    }

    Some(resolved)
}

/// What [`start`] brought up. Hold it for the process's lifetime: dropping it
/// stops both listeners.
#[derive(Default)]
#[allow(
    dead_code,
    reason = "held only for Drop, which stops the listeners; read by the tests"
)]
pub struct Serving {
    /// The API server, when it bound.
    pub server: Option<EmbeddedServer>,
    /// The web UI, when it was asked for and started.
    pub web: Option<EmbeddedWeb>,
}

/// The TUI-facing outcome of [`start`].
pub struct ServeStatus {
    /// How the API server fared.
    pub server: EmbeddedServerStatus,
    /// How the web UI fared, or `None` when it was not asked for.
    pub web: Option<EmbeddedServerStatus>,
}

/// Bind and serve, sharing the TUI's own service, then — if `[web_ui]` asked
/// for it — the web UI in front of that server.
///
/// Neither failure is fatal: the overwhelmingly likely cause is a port already
/// taken because a server is already running, and killing the TUI over that
/// would be absurd.
pub async fn start(service: CommanderService, plan: ServePlan) -> (Serving, ServeStatus) {
    let ServePlan {
        cfg,
        auth,
        token,
        web,
        ..
    } = plan;
    let server = match embed::start(service, &cfg, auth).await {
        Ok(server) => server,
        Err(e) => {
            let reason = e.to_string();
            warn!("embedded server not started: {reason}");
            // Carry the server's reason: this toast is raised second, so it
            // replaces the server's, and "needs the server" alone would hide why.
            let web = web.map(|_| EmbeddedServerStatus::Failed {
                reason: format!("the embedded server did not start ({reason})"),
            });
            let status = ServeStatus {
                server: EmbeddedServerStatus::Failed { reason },
                web,
            };
            return (Serving::default(), status);
        }
    };
    let url = server.url();
    info!("serving the commander API on {url}");

    let (web, web_status) = match (web, &token) {
        (Some(web_cfg), Some(token)) => {
            match start_web(&web_cfg, upstream_url(server.addr()), token).await {
                Ok(web) => {
                    let url = web.url();
                    info!("serving the web UI on {url}");
                    (
                        Some(web),
                        Some(EmbeddedServerStatus::Listening { url, token: None }),
                    )
                }
                Err(reason) => {
                    warn!("web UI not started: {reason}");
                    (None, Some(EmbeddedServerStatus::Failed { reason }))
                }
            }
        }
        _ => (None, None),
    };

    let serving = Serving {
        server: Some(server),
        web,
    };
    let status = ServeStatus {
        server: EmbeddedServerStatus::Listening { url, token },
        web: web_status,
    };
    (serving, status)
}

/// The browser auth for the embedded web UI: Basic auth with the configured
/// credentials, the API token injected upstream so it never reaches the
/// browser. A missing password refuses to start — there is no unauthenticated
/// embedded web UI, for the same reason there is no unauthenticated embedded
/// server.
pub fn web_auth(web: &WebUiConfig, token: &str) -> Result<AuthMode, String> {
    let password = web
        .password()
        .ok_or("no password is set (Settings \u{203a} Server \u{203a} Web UI Password)")?;
    AuthMode::bff(web.username.clone(), password, token).map_err(|e| e.to_string())
}

/// The URL the web UI proxies to: the API server as bound, dialled over
/// loopback. An unspecified bind (`0.0.0.0` / `::`) is not dialable, and
/// rendering it as `localhost` would leave the choice of `127.0.0.1` vs `::1`
/// to the resolver — which, against an IPv4-only listener, can pick the wrong
/// one.
pub fn upstream_url(addr: SocketAddr) -> String {
    let ip = match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    format!("http://{}", SocketAddr::new(ip, addr.port()))
}

async fn start_web(
    web: &WebUiConfig,
    upstream: String,
    token: &str,
) -> Result<EmbeddedWeb, String> {
    let auth = web_auth(web, token)?;
    claude_commander_web::start(web.bind, web.port, &upstream, auth)
        .await
        .map_err(|e| e.to_string())
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

    /// Turning on only the web UI still serves: it is a proxy in front of the
    /// API, so asking for it has to bring the API up too.
    #[test]
    fn web_ui_auto_start_alone_serves_the_api_too() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[web_ui]\nauto_start = true\n").unwrap();
        let mut config = Config {
            web_ui: WebUiConfig {
                auto_start: true,
                ..Default::default()
            },
            ..Default::default()
        };

        let plan = prepare(&mut config, &path, false, false).expect("web_ui means serve");
        assert!(plan.web.is_some(), "the web UI must be part of the plan");
        assert!(
            !plan.cfg.auto_start,
            "the API's own auto_start is left as configured"
        );
    }

    #[test]
    fn the_web_ui_is_not_planned_unless_asked_for() {
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
        let plan = prepare(&mut config, &path, false, false).unwrap();
        assert!(plan.web.is_none());
    }

    /// `--no-serve` turns the web UI off with everything else.
    #[test]
    fn no_serve_also_suppresses_the_web_ui() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = Config {
            web_ui: WebUiConfig {
                auto_start: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(prepare(&mut config, &path, false, true).is_none());
    }

    #[test]
    fn the_web_ui_refuses_to_start_without_a_password() {
        for password in [None, Some(String::new()), Some("  ".to_string())] {
            let web = WebUiConfig {
                password: password.clone(),
                ..Default::default()
            };
            let err = web_auth(&web, "tok").expect_err("no password must refuse");
            assert!(err.contains("password"), "{password:?}: {err}");
        }
    }

    #[test]
    fn the_web_ui_injects_the_api_token_behind_basic_auth() {
        let web = WebUiConfig {
            username: "me".into(),
            password: Some("pw".into()),
            ..Default::default()
        };
        match web_auth(&web, "tok").unwrap() {
            AuthMode::Bff {
                username,
                password,
                token,
            } => {
                assert_eq!(username, "me");
                assert_eq!(password, "pw");
                assert_eq!(token, "tok");
            }
            AuthMode::PassThrough => panic!("must be BFF"),
        }
    }

    #[test]
    fn upstream_dials_loopback_for_an_unspecified_bind() {
        let url = |a: &str| upstream_url(a.parse().unwrap());
        assert_eq!(url("0.0.0.0:7878"), "http://127.0.0.1:7878");
        assert_eq!(url("[::]:7878"), "http://[::1]:7878");
        assert_eq!(url("127.0.0.1:7878"), "http://127.0.0.1:7878");
        assert_eq!(url("100.64.0.7:7878"), "http://100.64.0.7:7878");
    }

    /// End to end over real sockets: the API and the web UI both come up, and
    /// the web UI proxies an authenticated browser request through to the API
    /// with the token injected.
    #[tokio::test]
    async fn start_serves_the_web_ui_in_front_of_the_api() {
        let (data, worktrees) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let service = claude_commander_test_support::test_state(&data, &worktrees).service;

        let mut plan = plan(ServerConfig {
            port: 0,
            ..Default::default()
        });
        plan.web = Some(WebUiConfig {
            port: 0,
            password: Some("pw".into()),
            ..Default::default()
        });

        let (serving, status) = start(service, plan).await;
        assert!(matches!(
            status.server,
            EmbeddedServerStatus::Listening { .. }
        ));
        let web_url = match status.web {
            Some(EmbeddedServerStatus::Listening { url, token }) => {
                assert!(token.is_none(), "the web status must not carry the token");
                url
            }
            other => panic!("web UI did not start: {other:?}"),
        };
        assert!(serving.web.is_some());

        let client = reqwest::Client::new();
        let anon = client
            .get(format!("{web_url}/api/config"))
            .send()
            .await
            .unwrap();
        assert_eq!(anon.status(), 401);
        let proxied = client
            .get(format!("{web_url}/api/config"))
            .basic_auth("admin", Some("pw"))
            .send()
            .await
            .unwrap();
        assert!(
            proxied.status().is_success(),
            "the proxied API call must be authenticated upstream: {}",
            proxied.status()
        );
    }

    /// If the API cannot bind, the web UI is not attempted, and its failure
    /// says why — its toast replaces the server's, so it must carry the reason.
    #[tokio::test]
    async fn a_failed_server_fails_the_web_ui_with_the_servers_reason() {
        let (data, worktrees) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let service = claude_commander_test_support::test_state(&data, &worktrees).service;
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();

        let mut plan = plan(ServerConfig {
            port: held.local_addr().unwrap().port(),
            ..Default::default()
        });
        plan.web = Some(WebUiConfig {
            port: 0,
            password: Some("pw".into()),
            ..Default::default()
        });

        let (serving, status) = start(service, plan).await;
        assert!(serving.server.is_none() && serving.web.is_none());
        match status.web {
            Some(EmbeddedServerStatus::Failed { reason }) => {
                assert!(reason.contains("could not bind"), "{reason}")
            }
            other => panic!("expected a failed web status, got {other:?}"),
        }
    }

    /// With the web UI requested but no password, the API still serves and the
    /// web UI reports why it did not.
    #[tokio::test]
    async fn a_web_ui_without_a_password_fails_alone() {
        let (data, worktrees) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let service = claude_commander_test_support::test_state(&data, &worktrees).service;

        let mut plan = plan(ServerConfig {
            port: 0,
            ..Default::default()
        });
        plan.web = Some(WebUiConfig {
            port: 0,
            ..Default::default()
        });

        let (serving, status) = start(service, plan).await;
        assert!(serving.server.is_some(), "the API must still serve");
        assert!(serving.web.is_none());
        match status.web {
            Some(EmbeddedServerStatus::Failed { reason }) => {
                assert!(reason.contains("password"), "{reason}")
            }
            other => panic!("expected a failed web status, got {other:?}"),
        }
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
