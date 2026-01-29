//! Claude Commander - Terminal UI for managing Claude coding sessions
//!
//! Run with `claude-commander` or `claude-commander --help` for usage.

use std::io::Write;
use std::process::Command;

use clap::{Parser, Subcommand};
use color_eyre::eyre::Result;
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use claude_commander::{
    config::{AppState, Config},
    tui::App,
    APP_NAME, VERSION,
};

#[derive(Parser)]
#[command(name = APP_NAME)]
#[command(version = VERSION)]
#[command(about = "A high-performance terminal UI for managing Claude coding sessions")]
#[command(long_about = None)]
struct Cli {
    /// Enable debug logging
    #[arg(short, long)]
    debug: bool,

    /// Path to config file
    #[arg(short, long)]
    config: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the interactive TUI (default)
    Tui,

    /// List all sessions
    List {
        /// Show all sessions including stopped ones
        #[arg(short, long)]
        all: bool,
    },

    /// Create a new session
    New {
        /// Session name
        name: String,

        /// Program to run (default: claude)
        #[arg(short, long)]
        program: Option<String>,

        /// Project path (default: current directory)
        #[arg(short = 'd', long)]
        path: Option<std::path::PathBuf>,
    },

    /// Attach to an existing session
    Attach {
        /// Session name or ID
        session: String,
    },

    /// Show configuration
    Config {
        /// Initialize config file with defaults
        #[arg(long)]
        init: bool,
    },
}

fn setup_logging(debug: bool, to_file: bool) -> Result<()> {
    let filter = if debug {
        EnvFilter::new("debug")
    } else {
        // Use info level for our crate, warn for dependencies
        EnvFilter::new("info")
            .add_directive("gix=warn".parse()?)
            .add_directive("tokio=warn".parse()?)
    };

    if to_file {
        // Log to file when running TUI (so logs don't interfere with display)
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/claude-commander.log")?;

        tracing_subscriber::registry()
            .with(fmt::layer().with_writer(file).with_target(false))
            .with(filter)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(fmt::layer().with_target(false))
            .with(filter)
            .init();
    }

    Ok(())
}

/// Run the TUI and return an optional attach command
async fn run_tui(config: Config, app_state: AppState) -> Result<Option<String>> {
    let mut app = App::new(config, app_state);
    Ok(app.run().await?)
}

/// Execute tmux attach command synchronously (outside tokio runtime)
fn execute_attach(cmd: &str) {
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    // Parse the command (format: "tmux attach-session -t <name>")
    let parts: Vec<&str> = cmd.split_whitespace().collect();

    if parts.len() >= 4 && parts[0] == "tmux" {
        let status = Command::new("tmux")
            .args(&parts[1..])
            .status();

        match status {
            Ok(exit_status) => {
                if !exit_status.success() {
                    eprintln!("tmux exited with code: {:?}", exit_status.code());
                }
            }
            Err(e) => {
                eprintln!("Failed to run tmux: {}", e);
            }
        }
    }
}

fn main() -> Result<()> {
    // Install color-eyre error hooks
    color_eyre::install()?;

    let cli = Cli::parse();

    // Load configuration
    let config = Config::load().unwrap_or_else(|e| {
        eprintln!("Warning: Failed to load config, using defaults: {}", e);
        Config::default()
    });

    // Ensure required directories exist
    if let Err(e) = config.ensure_directories() {
        eprintln!("Warning: Failed to create directories: {}", e);
    }

    // Load persistent state
    let app_state = AppState::load().unwrap_or_else(|e| {
        eprintln!("Warning: Failed to load state, starting fresh: {}", e);
        AppState::new()
    });

    match cli.command {
        None | Some(Commands::Tui) => {
            // Setup logging to file for TUI mode
            setup_logging(cli.debug, true)?;

            info!("Starting Claude Commander TUI v{}", VERSION);

            // Main loop: TUI -> attach -> detach -> TUI
            loop {
                // Reload state each iteration (may have changed)
                let current_state = AppState::load().unwrap_or_else(|_| AppState::new());

                // Run TUI in a tokio runtime, get attach command if any
                let runtime = tokio::runtime::Runtime::new()?;
                let attach_cmd = runtime.block_on(run_tui(config.clone(), current_state))?;

                // Drop the runtime before running tmux
                drop(runtime);

                // Execute attach command outside of async runtime
                if let Some(cmd) = attach_cmd {
                    info!("Executing attach command: {}", cmd);
                    execute_attach(&cmd);
                    // After tmux exits (user detached), loop back to TUI
                    info!("Returned from tmux, restarting TUI");
                } else {
                    // No attach command means user quit the TUI
                    break;
                }
            }
        }

        Some(Commands::List { all }) => {
            setup_logging(cli.debug, false)?;

            println!("Sessions:");
            println!();

            if app_state.projects.is_empty() {
                println!("  No projects. Use 'claude-commander' to add one.");
                return Ok(());
            }

            for project in app_state.projects.values() {
                println!("  {} ({})", project.name, project.main_branch);

                let sessions: Vec<_> = project
                    .worktrees
                    .iter()
                    .filter_map(|id| app_state.sessions.get(id))
                    .filter(|s| all || s.status.is_active())
                    .collect();

                if sessions.is_empty() {
                    println!("    (no sessions)");
                } else {
                    for session in sessions {
                        let status_icon = match session.status {
                            claude_commander::SessionStatus::Running => "●",
                            claude_commander::SessionStatus::Paused => "◐",
                            claude_commander::SessionStatus::Stopped => "○",
                        };
                        println!(
                            "    {} {} [{}] ({})",
                            status_icon, session.title, session.branch, session.program
                        );
                    }
                }
                println!();
            }
        }

        Some(Commands::New {
            name,
            program,
            path,
        }) => {
            setup_logging(cli.debug, false)?;

            let path = path.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

            use claude_commander::session::SessionManager;
            use std::sync::Arc;
            use tokio::sync::RwLock;

            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(async {
                let app_state = Arc::new(RwLock::new(app_state));
                let manager = SessionManager::new(config, app_state);

                // Check tmux
                manager.check_tmux().await?;

                // First, try to find or add the project
                let project_id = {
                    let state = manager.app_state.read().await;
                    state
                        .projects
                        .values()
                        .find(|p| p.repo_path == path)
                        .map(|p| p.id)
                };

                let project_id = match project_id {
                    Some(id) => id,
                    None => {
                        println!("Adding project from {:?}...", path);
                        manager.add_project(path).await?
                    }
                };

                println!("Creating session '{}'...", name);
                let session_id = manager.create_session(&project_id, name, program).await?;

                println!("Session created: {}", session_id);
                println!();
                println!("Attach with: claude-commander attach {}", session_id);
                Ok::<_, color_eyre::eyre::Error>(())
            })?;
        }

        Some(Commands::Attach { session }) => {
            setup_logging(cli.debug, false)?;

            // Find session by name or ID prefix
            let session_id = app_state
                .sessions
                .iter()
                .find(|(id, s)| {
                    s.title.to_lowercase() == session.to_lowercase()
                        || id.to_string().starts_with(&session)
                })
                .map(|(_, s)| s.tmux_session_name.clone());

            match session_id {
                Some(tmux_name) => {
                    let status = Command::new("tmux")
                        .args(["attach-session", "-t", &tmux_name])
                        .status()?;

                    if !status.success() {
                        eprintln!("Failed to attach to session");
                    }
                }
                None => {
                    eprintln!("Session not found: {}", session);
                    eprintln!("Use 'claude-commander list' to see available sessions.");
                }
            }
        }

        Some(Commands::Config { init }) => {
            setup_logging(cli.debug, false)?;

            if init {
                config.save()?;
                println!(
                    "Configuration initialized at {:?}",
                    Config::config_file_path()?
                );
            } else {
                println!("Configuration:");
                println!("{}", toml::to_string_pretty(&config)?);
                println!("\nConfig file: {:?}", Config::config_file_path()?);
                println!("Data dir: {:?}", Config::data_dir()?);
                println!("State file: {:?}", Config::state_file_path()?);
            }
        }
    }

    Ok(())
}
