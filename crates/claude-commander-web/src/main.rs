//! `claude-commander-web` binary — serve the SPA and reverse-proxy to
//! `claude-commander-server`.
//!
//! Auth mode is chosen by whether a commander token is supplied:
//! - `--commander-token` set  → BFF: browser uses Basic auth, token injected upstream.
//! - `--commander-token` unset → pass-through: the browser carries the token itself.

use std::net::IpAddr;

use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use claude_commander_web::AuthMode;

#[derive(Parser, Debug)]
#[command(
    name = "claude-commander-web",
    version,
    about = "Standalone web UI for Claude Commander (proxies to claude-commander-server)"
)]
struct Cli {
    /// Interface to bind. Defaults to 127.0.0.1 (loopback only).
    #[arg(long, default_value = "127.0.0.1")]
    bind: IpAddr,

    /// Port to listen on. Defaults to 8420.
    #[arg(long, default_value_t = 8420)]
    port: u16,

    /// Base URL of the upstream claude-commander-server.
    #[arg(long, default_value = "http://127.0.0.1:7878")]
    commander_url: String,

    /// Commander bearer token. When set, runs in BFF mode: the browser logs in
    /// with Basic auth and this token is injected upstream (never exposed to the
    /// browser). When unset, runs in pass-through mode: the browser supplies the
    /// token itself.
    #[arg(long, env = "CC_WEB_COMMANDER_TOKEN")]
    commander_token: Option<String>,

    /// Basic-auth username for BFF mode.
    #[arg(long, default_value = "admin")]
    username: String,

    /// Basic-auth password for BFF mode. Required when --commander-token is set.
    #[arg(long, env = "CC_WEB_PASSWORD")]
    password: Option<String>,

    /// Verbose (debug-level) logging.
    #[arg(long)]
    debug: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    setup_logging(cli.debug);

    let commander_url = cli.commander_url.trim_end_matches('/').to_string();

    let auth = match &cli.commander_token {
        Some(token) => {
            let password = cli.password.clone().ok_or(
                "--password (or CC_WEB_PASSWORD) is required in BFF mode (when --commander-token is set)",
            )?;
            AuthMode::bff(cli.username.clone(), password, token.clone())?
        }
        None => AuthMode::PassThrough,
    };

    match &auth {
        AuthMode::Bff { username, .. } => info!(
            "Web UI on http://{}:{} — BFF mode (Basic auth, user `{username}`); proxying to {commander_url}",
            cli.bind, cli.port
        ),
        AuthMode::PassThrough => info!(
            "Web UI on http://{}:{} — pass-through mode (browser supplies the token); proxying to {commander_url}",
            cli.bind, cli.port
        ),
    }
    if !cli.bind.is_loopback() {
        warn!(
            "Bound on a non-loopback interface — put this behind TLS (e.g. `tailscale serve`) on untrusted networks."
        );
    }

    claude_commander_web::start(cli.bind, cli.port, &commander_url, auth)
        .await?
        .join()
        .await;
    Ok(())
}

fn setup_logging(debug: bool) {
    let filter = if debug {
        EnvFilter::new("debug")
    } else {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("claude_commander_web=info,warn"))
    };
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .init();
}
