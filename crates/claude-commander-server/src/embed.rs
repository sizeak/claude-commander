//! Starting the server from inside another process.
//!
//! The TUI runs the HTTP server in-process rather than shelling out to the
//! `claude-commander-server` binary, and shares its own [`CommanderService`]
//! with it. That sharing is the whole point: one set of background loops, one
//! `state.json` writer and one telemetry stream, with teardown falling out of
//! process exit instead of needing orphan cleanup.
//!
//! The standalone binary uses this module too, so there is exactly one
//! implementation of "bind a listener and serve the router" and one token
//! policy, rather than a second copy in `main.rs` that drifts.

use std::net::SocketAddr;

use claude_commander_core::api::CommanderService;
use claude_commander_core::config::ServerConfig;
use tokio::task::JoinHandle;

use crate::auth::AuthConfig;
use crate::config::check_no_auth_bind;
use crate::router::build_router;
use crate::state::AppState;

/// Where the bearer token for this run came from.
///
/// The distinction matters to the caller, not to the server: a `Generated` token
/// is one the operator has never seen, so a frontend has to either log it (the
/// standalone binary) or persist it so a paired client keeps working across
/// restarts (the TUI). Returning the decision instead of acting on it keeps this
/// function pure and testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenDecision {
    /// A token was configured; use it and never log it.
    Existing(String),
    /// No token was configured, so this one was freshly generated.
    Generated(String),
}

impl TokenDecision {
    /// The token itself, however it was obtained.
    pub fn token(&self) -> &str {
        match self {
            Self::Existing(t) | Self::Generated(t) => t,
        }
    }

    /// Consume the decision, yielding the token.
    pub fn into_token(self) -> String {
        match self {
            Self::Existing(t) | Self::Generated(t) => t,
        }
    }
}

/// Decide this run's bearer token: the configured one, or a fresh random one.
///
/// Secure-by-default — a fresh install gets authentication without the operator
/// opting in. Generating (rather than disabling auth) is deliberate: an
/// unauthenticated port is reachable by anything else running on the machine,
/// which is not the same trust boundary as the operator's own shell.
pub fn token_decision(cfg: &ServerConfig) -> TokenDecision {
    match &cfg.token {
        Some(token) if !token.is_empty() => TokenDecision::Existing(token.clone()),
        // An empty string is a hand-edited `token = ""`, which is an absent
        // token rather than a zero-length secret that would authenticate anyone.
        _ => TokenDecision::Generated(generate_token()),
    }
}

/// Generate a random bearer token: two v4 UUIDs (256 bits of OS-RNG entropy)
/// rendered as hex without separators.
fn generate_token() -> String {
    let a = uuid::Uuid::new_v4().simple().to_string();
    let b = uuid::Uuid::new_v4().simple().to_string();
    format!("{a}{b}")
}

/// Why the server could not be started.
///
/// Self-contained with a hand-written `Display`, and deliberately carries no
/// `#[from]` into `claude_commander_core::Error`: a crate that can be embedded
/// owes its host nothing, and every crossing should be spelled out at the call
/// site (the `protocol::paste::ImageRejection` precedent).
#[derive(Debug)]
pub enum StartError {
    /// `--allow-no-auth` was requested on a routable bind address.
    UnauthenticatedBind(String),
    /// The listener could not be bound — most often the port is already taken,
    /// which usually means a server is already running.
    Bind {
        addr: SocketAddr,
        source: std::io::Error,
    },
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnauthenticatedBind(msg) => write!(f, "{msg}"),
            Self::Bind { addr, source } => {
                write!(f, "could not bind {addr}: {source}")
            }
        }
    }
}

impl std::error::Error for StartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::UnauthenticatedBind(_) => None,
            Self::Bind { source, .. } => Some(source),
        }
    }
}

/// A running server, owned by whoever started it.
///
/// Dropping it aborts the serving task, so an embedder gets teardown for free
/// and cannot leak a listener by forgetting to stop it. The standalone binary
/// instead [`join`](Self::join)s, since serving *is* its job.
pub struct EmbeddedServer {
    addr: SocketAddr,
    /// `Option` only so [`join`](Self::join) can take the handle out and await
    /// it: a `Drop` type cannot be destructured, and awaiting a handle `Drop`
    /// would then abort is not much of an await.
    task: Option<JoinHandle<()>>,
}

impl EmbeddedServer {
    /// The address actually bound. Worth reading rather than assuming: a
    /// configured port of 0 resolves to a real one here, which is how the tests
    /// avoid fighting over a fixed port.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A base URL a client can be pointed at. See [`server_url`].
    pub fn url(&self) -> String {
        server_url(self.addr)
    }

    /// Serve until the task ends (i.e. forever, absent an error).
    pub async fn join(mut self) {
        // Taking the handle disarms `Drop`, so awaiting here cannot race the
        // abort that dropping this value would otherwise perform.
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for EmbeddedServer {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

impl std::fmt::Debug for EmbeddedServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddedServer")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

/// Render a bound address as a base URL for a client.
///
/// An unspecified bind (`0.0.0.0` / `::`) is not a dialable host, so it renders
/// as `localhost` — correct from this machine, and a client on another one has
/// to be given this host's own address either way (which this process cannot
/// discover without enumerating interfaces).
pub fn server_url(addr: SocketAddr) -> String {
    if addr.ip().is_unspecified() {
        return format!("http://localhost:{}", addr.port());
    }
    // `SocketAddr`'s Display already brackets an IPv6 host for URL use.
    format!("http://{addr}")
}

/// Bind the configured address and serve the API on a background task.
///
/// The `service` is shared with the caller, so this deliberately does **not**
/// start background loops or the hibernation loop — a frontend that already runs
/// them (the TUI) would end up with two of each. The standalone binary starts
/// them itself before calling this.
pub async fn start(
    service: CommanderService,
    cfg: &ServerConfig,
    auth: AuthConfig,
) -> Result<EmbeddedServer, StartError> {
    check_no_auth_bind(cfg.bind, matches!(auth, AuthConfig::Disabled))
        .map_err(StartError::UnauthenticatedBind)?;

    let requested = SocketAddr::new(cfg.bind, cfg.port);
    let listener = tokio::net::TcpListener::bind(requested)
        .await
        .map_err(|source| StartError::Bind {
            addr: requested,
            source,
        })?;
    // Not `requested`: a configured port of 0 has just been resolved to a real
    // one, and that is what a client has to be told.
    let addr = listener.local_addr().unwrap_or(requested);

    let state = AppState::new(service, auth).with_cors(cfg.cors_allowed_origins.clone());
    let app = build_router(state);

    let task = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("server stopped: {e}");
        }
    });

    Ok(EmbeddedServer {
        addr,
        task: Some(task),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn a_configured_token_is_used_as_is() {
        let cfg = ServerConfig {
            token: Some("configured".into()),
            ..Default::default()
        };
        assert_eq!(
            token_decision(&cfg),
            TokenDecision::Existing("configured".into())
        );
    }

    #[test]
    fn a_missing_token_is_generated() {
        let cfg = ServerConfig::default();
        let decision = token_decision(&cfg);
        assert!(
            matches!(decision, TokenDecision::Generated(_)),
            "{decision:?}"
        );
        // Two v4 UUIDs, hex, no separators.
        assert_eq!(decision.token().len(), 64);
        assert!(decision.token().chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// A hand-edited `token = ""` must not authenticate every caller.
    #[test]
    fn an_empty_token_counts_as_missing() {
        let cfg = ServerConfig {
            token: Some(String::new()),
            ..Default::default()
        };
        assert!(matches!(token_decision(&cfg), TokenDecision::Generated(_)));
    }

    #[test]
    fn generated_tokens_do_not_repeat() {
        let cfg = ServerConfig::default();
        assert_ne!(
            token_decision(&cfg).into_token(),
            token_decision(&cfg).into_token()
        );
    }

    #[test]
    fn url_renders_a_dialable_host() {
        let port = 7878;
        assert_eq!(
            server_url(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)),
            "http://127.0.0.1:7878"
        );
        assert_eq!(
            server_url(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
                port
            )),
            "http://192.168.1.10:7878"
        );
        // An unspecified bind is not a host a client can dial.
        assert_eq!(
            server_url(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)),
            "http://localhost:7878"
        );
        assert_eq!(
            server_url(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port)),
            "http://localhost:7878"
        );
        // IPv6 keeps the brackets a URL needs.
        assert_eq!(
            server_url(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port)),
            "http://[::1]:7878"
        );
    }

    /// The no-auth guard runs before the listener is bound, so an
    /// unauthenticated routable bind never reaches the network even for an
    /// instant.
    #[tokio::test]
    async fn start_refuses_to_serve_unauthenticated_off_loopback() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::handlers::test_support::test_state(&dir);
        let cfg = ServerConfig {
            bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: 0,
            ..Default::default()
        };

        let err = start(state.service.clone(), &cfg, AuthConfig::Disabled)
            .await
            .expect_err("0.0.0.0 with auth disabled must be refused");
        assert!(matches!(err, StartError::UnauthenticatedBind(_)), "{err:?}");
    }

    /// Port 0 must be resolved to the port actually bound — a client cannot be
    /// pointed at port 0.
    #[tokio::test]
    async fn start_reports_the_port_it_actually_bound() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::handlers::test_support::test_state(&dir);
        let cfg = ServerConfig {
            port: 0,
            ..Default::default()
        };

        let server = start(state.service.clone(), &cfg, AuthConfig::Disabled)
            .await
            .expect("loopback bind on an ephemeral port");
        assert_ne!(server.addr().port(), 0);
        assert_eq!(
            server.url(),
            format!("http://127.0.0.1:{}", server.addr().port())
        );
    }

    /// End to end over a real socket: the embedded server enforces its bearer
    /// token. This is the property the TUI's `--serve` rests on — the port is
    /// reachable by anything else on the machine, so an unauthenticated request
    /// must be refused.
    #[tokio::test]
    async fn a_served_request_needs_the_token() {
        let dir = tempfile::tempdir().unwrap();
        // The auth under test is the one handed to `start`; the fixture's own
        // `AppState` is discarded, since `start` builds its own.
        let state = crate::handlers::test_support::test_state(&dir);
        let cfg = ServerConfig {
            port: 0,
            ..Default::default()
        };
        let server = start(
            state.service.clone(),
            &cfg,
            AuthConfig::Token("sekret".into()),
        )
        .await
        .expect("loopback bind");
        let url = format!("{}/api/config", server.url());

        let client = reqwest::Client::new();
        let anon = client.get(&url).send().await.expect("request sent");
        assert_eq!(
            anon.status(),
            401,
            "an unauthenticated request must be refused"
        );

        let authed = client
            .get(&url)
            .bearer_auth("sekret")
            .send()
            .await
            .expect("request sent");
        assert_eq!(authed.status(), 200);
    }

    /// Dropping the handle must stop the server and free the port.
    ///
    /// Not *instantly*, though: `JoinHandle::abort` requests cancellation and the
    /// listener is only closed once the runtime drops the task, so this polls
    /// rather than asserting an immediate rebind. In production the teardown path
    /// is process exit, where the OS closes the socket regardless — the guarantee
    /// under test here is that the listener is released at all rather than leaked
    /// for the life of the runtime.
    #[tokio::test]
    async fn dropping_the_handle_releases_the_port() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::handlers::test_support::test_state(&dir);
        let cfg = ServerConfig {
            port: 0,
            ..Default::default()
        };

        let first = start(state.service.clone(), &cfg, AuthConfig::Disabled)
            .await
            .expect("first bind");
        let port = first.addr().port();
        drop(first);

        // Re-binding the same port is the observable proof the listener is gone.
        let mut last = None;
        for _ in 0..100 {
            tokio::task::yield_now().await;
            match start(
                state.service.clone(),
                &ServerConfig {
                    port,
                    ..Default::default()
                },
                AuthConfig::Disabled,
            )
            .await
            {
                Ok(_) => return,
                Err(e) => last = Some(e),
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("port {port} was never released: {last:?}");
    }

    /// A taken port must surface as `Bind`, not a panic: the TUI treats it as
    /// "a server is already running" and carries on without one.
    #[tokio::test]
    async fn start_reports_a_taken_port() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::handlers::test_support::test_state(&dir);

        let first = start(
            state.service.clone(),
            &ServerConfig {
                port: 0,
                ..Default::default()
            },
            AuthConfig::Disabled,
        )
        .await
        .expect("first bind");

        let err = start(
            state.service.clone(),
            &ServerConfig {
                port: first.addr().port(),
                ..Default::default()
            },
            AuthConfig::Disabled,
        )
        .await
        .expect_err("the port is already held by `first`");
        assert!(matches!(err, StartError::Bind { .. }), "{err:?}");
        assert!(err.to_string().contains("could not bind"), "{err}");
    }
}
