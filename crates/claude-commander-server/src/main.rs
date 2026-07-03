//! `claude-commander-server` — exposes a local `CommanderService` over HTTP +
//! WebSocket so clients on other machines can drive Commander sessions.

use std::net::SocketAddr;

use clap::Parser;
use claude_commander_core::api::{BackgroundOpts, CommanderService};
use claude_commander_core::telemetry::FrontendInfo;
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use claude_commander_server::auth::AuthConfig;
use claude_commander_server::config::{ServerConfig, check_no_auth_bind};
use claude_commander_server::router::build_router;
use claude_commander_server::state::AppState;

/// Identify this binary to the telemetry layer (required by `CommanderService`).
fn frontend() -> FrontendInfo {
    FrontendInfo::new("claude-commander-server", env!("CARGO_PKG_VERSION"))
}

#[derive(Parser, Debug)]
#[command(name = "claude-commander-server", version, about)]
struct Cli {
    /// Interface to bind (overrides config). Defaults to 127.0.0.1.
    #[arg(long)]
    bind: Option<std::net::IpAddr>,

    /// Port to listen on (overrides config). Defaults to 7878.
    #[arg(long)]
    port: Option<u16>,

    /// Pre-shared bearer token (overrides config / auto-generation).
    #[arg(long)]
    token: Option<String>,

    /// Disable authentication entirely. Loopback-only dev convenience —
    /// never use on a non-loopback bind.
    #[arg(long)]
    allow_no_auth: bool,

    /// Enable TLS (requires the `tls` build feature and cert/key paths in config).
    #[arg(long)]
    tls: bool,

    /// Verbose (debug-level) logging.
    #[arg(long)]
    debug: bool,
}

fn setup_logging(debug: bool) {
    let filter = if debug {
        EnvFilter::new("debug")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("claude_commander_server=info,claude_commander_core=info,warn")
        })
    };
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .init();
}

/// Resolve the effective server config by layering CLI flags over the loaded
/// `[server]` config.
fn resolve_config(cli: &Cli) -> Result<ServerConfig, Box<dyn std::error::Error>> {
    let mut cfg = ServerConfig::load()?;
    if let Some(bind) = cli.bind {
        cfg.bind = bind;
    }
    if let Some(port) = cli.port {
        cfg.port = port;
    }
    if let Some(token) = &cli.token {
        cfg.token = Some(token.clone());
    }
    Ok(cfg)
}

/// Resolve the authentication policy from flags + config.
///
/// Precedence: `--allow-no-auth` disables auth; otherwise a token from
/// CLI/config is used; otherwise a fresh random token is generated, logged
/// once, and used (secure-by-default on a fresh install). The token value is
/// only logged when it was auto-generated (so the operator can copy it); a
/// configured token is never logged.
fn resolve_auth(cfg: &ServerConfig, allow_no_auth: bool) -> AuthConfig {
    if allow_no_auth {
        warn!("authentication disabled (--allow-no-auth); only safe on a loopback bind");
        return AuthConfig::Disabled;
    }
    match &cfg.token {
        Some(token) => AuthConfig::Token(token.clone()),
        None => {
            let token = generate_token();
            info!("no token configured; generated a one-time bearer token for this run: {token}");
            info!("set `[server] token` in config.toml (or pass --token) to persist it");
            AuthConfig::Token(token)
        }
    }
}

/// Generate a random bearer token: two v4 UUIDs (256 bits of OS-RNG entropy)
/// rendered as hex without separators.
fn generate_token() -> String {
    let a = uuid::Uuid::new_v4().simple().to_string();
    let b = uuid::Uuid::new_v4().simple().to_string();
    format!("{a}{b}")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    setup_logging(cli.debug);

    let cfg = resolve_config(&cli)?;

    // Hard error (not just a warning) if --allow-no-auth is used on a
    // non-loopback bind: that would expose an unauthenticated API to the
    // network. Decision lives in a pure, unit-tested lib function.
    check_no_auth_bind(cfg.bind, cli.allow_no_auth)
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

    let auth = resolve_auth(&cfg, cli.allow_no_auth);

    if cli.tls {
        warn!("--tls requested; TLS support requires the `tls` build feature (not yet wired)");
    }

    let config = claude_commander_core::Config::load()?;
    let commander_enabled = config.commander_enabled;
    let service = CommanderService::for_cli(config, frontend())?;
    // Drive the same background loops the local TUI runs (agent-state polling,
    // PR-status checks, project auto-pull, state-sync) so remote clients see live
    // data via `/workspace` + `/agent-states` polls. Handles run for the process
    // lifetime; we don't need to hold them.
    let _background = service.spawn_background_tasks(BackgroundOpts { commander_enabled });
    let state = AppState::new(service, auth).with_cors(cfg.cors_allowed_origins.clone());
    let app = build_router(state);

    let addr = SocketAddr::new(cfg.bind, cfg.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("claude-commander-server listening on http://{addr}");

    axum::serve(listener, app).await?;
    Ok(())
}
