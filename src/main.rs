//! Claude Commander - Terminal UI for managing Claude coding sessions
//!
//! Run with `claude-commander` or `claude-commander --help` for usage.

use std::io::{IsTerminal, Write};

use clap::Parser;
use color_eyre::eyre::Result;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use claude_commander::{
    VERSION,
    cli_args::{Cli, Commands, cli_command},
    config::{AppState, Config, ConfigStore, StateStore},
    tmux::{AttachResult, attach_to_session},
    tui::App,
};

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

/// Raise the soft limit on open file descriptors toward the hard cap, so
/// fan-out subprocess work (gh, git diff, tmux) doesn't EMFILE on the
/// stingy macOS launchd default of 256.
///
/// Best-effort: failures are logged to stderr and do not abort startup.
#[cfg(unix)]
fn raise_fd_limit() {
    // Target soft limit. macOS's hard cap is typically kern.maxfilesperproc
    // (~184k); Linux varies. We cap our request to keep things reasonable.
    const TARGET_SOFT: nix::libc::rlim_t = 8192;

    let mut rlim = nix::libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };

    // SAFETY: rlim is a valid stack pointer to a properly-sized rlimit.
    let rc = unsafe { nix::libc::getrlimit(nix::libc::RLIMIT_NOFILE, &mut rlim) };
    if rc != 0 {
        eprintln!(
            "Warning: getrlimit(RLIMIT_NOFILE) failed: {}",
            std::io::Error::last_os_error()
        );
        return;
    }

    if rlim.rlim_cur >= TARGET_SOFT {
        return;
    }

    let new_soft = TARGET_SOFT.min(rlim.rlim_max);
    if new_soft <= rlim.rlim_cur {
        return;
    }

    let new_rlim = nix::libc::rlimit {
        rlim_cur: new_soft,
        rlim_max: rlim.rlim_max,
    };
    // SAFETY: new_rlim is a valid stack pointer to a properly-sized rlimit.
    let rc = unsafe { nix::libc::setrlimit(nix::libc::RLIMIT_NOFILE, &new_rlim) };
    if rc != 0 {
        eprintln!(
            "Warning: setrlimit(RLIMIT_NOFILE, {}) failed: {}",
            new_soft,
            std::io::Error::last_os_error()
        );
    }
}

#[cfg(not(unix))]
fn raise_fd_limit() {}

/// Execute async PTY-based attach to a tmux session
async fn execute_attach(session_name: &str, editor_triggers: Vec<Vec<u8>>) {
    // CLI `attach` resolves a Claude session by title/ID, never a shell. The
    // review toggle has no standalone UI here, so it's disabled (empty triggers).
    // Voice input needs the TUI's conversation runtime, so it's disabled here too.
    match attach_to_session(
        session_name,
        editor_triggers,
        Vec::new(),
        Vec::new(),
        None,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        true,
    )
    .await
    {
        Ok(outcome) => match outcome.result {
            AttachResult::Detached
            | AttachResult::SwitchToShell
            | AttachResult::SwitchToReview
            | AttachResult::OpenEditor => {
                info!("Detached from session");
            }
            AttachResult::SessionEnded => {
                info!("Session ended");
            }
            AttachResult::Error(e) => {
                eprintln!("Attach error: {}", e);
            }
        },
        Err(e) => {
            eprintln!("Failed to attach: {}", e);
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install color-eyre error hooks
    color_eyre::install()?;

    // macOS launchd hands processes a soft RLIMIT_NOFILE of 256, which is
    // easily exhausted once we fan out subprocesses across many sessions
    // (gh pr list, git diff, tmux commands). Raise our own soft limit
    // toward the hard cap before anything else runs.
    raise_fd_limit();

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

    match cli.command {
        None | Some(Commands::Tui) => {
            setup_logging(cli.debug, true)?;
            info!("Starting Claude Commander TUI v{}", VERSION);

            let config_store = std::sync::Arc::new(ConfigStore::new(config.clone())?);
            let app_state = AppState::load_or_exit();
            let store = std::sync::Arc::new(StateStore::new(app_state)?);
            let mut app = App::new(config_store, store);
            app.run().await?;
        }

        Some(Commands::List { all, json }) => {
            setup_logging(cli.debug, false)?;

            if json {
                let service = claude_commander::api::CommanderService::for_cli(config)?;
                let sessions = service.list_sessions(all).await?;
                let entries: Vec<_> = sessions
                    .iter()
                    .map(claude_commander::cli::SessionJsonEntry::from_info)
                    .collect();
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                let app_state = AppState::load_or_exit();

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
                                claude_commander::SessionStatus::Creating
                                | claude_commander::SessionStatus::Merging
                                | claude_commander::SessionStatus::Pushing => "⠋",
                                claude_commander::SessionStatus::Running => "●",
                                claude_commander::SessionStatus::Stopped => "○",
                                claude_commander::SessionStatus::CascadePaused => "⏸",
                            };
                            match claude_commander::session::display_branch(
                                &session.title,
                                &session.branch,
                            ) {
                                Some(shown_branch) => println!(
                                    "    {} {} [{}] ({})",
                                    status_icon, session.title, shown_branch, session.program
                                ),
                                None => println!(
                                    "    {} {} ({})",
                                    status_icon, session.title, session.program
                                ),
                            }
                        }
                    }
                    println!();
                }
            }
        }

        Some(Commands::Status { session, json }) => {
            setup_logging(cli.debug, false)?;

            let service = claude_commander::api::CommanderService::for_cli(config)?;
            let detail = match service.get_session_detail(&session, None).await? {
                Some(d) => d,
                None => {
                    eprintln!("Session not found: {}", session);
                    eprintln!("Use 'claude-commander list' to see available sessions.");
                    std::process::exit(1);
                }
            };

            let entry = claude_commander::cli::StatusJsonEntry::from_detail(&detail);

            if json {
                println!("{}", serde_json::to_string_pretty(&entry)?);
            } else {
                println!("{}", claude_commander::cli::format_status_human(&entry));
            }
        }

        Some(Commands::Delete { session, force }) => {
            setup_logging(cli.debug, false)?;

            let service = claude_commander::api::CommanderService::for_cli(config)?;

            let info = match service.find_session(&session).await? {
                Some(i) => i,
                None => {
                    eprintln!("Session not found: {}", session);
                    eprintln!("Use 'claude-commander list' to see available sessions.");
                    std::process::exit(1);
                }
            };

            match claude_commander::cli::delete_guard(force, std::io::stdin().is_terminal()) {
                claude_commander::cli::DeleteGuard::RequireForce => {
                    eprintln!(
                        "Refusing to delete \"{}\" without confirmation. Re-run with --force.",
                        info.title
                    );
                    std::process::exit(1);
                }
                claude_commander::cli::DeleteGuard::Prompt => {
                    print!(
                        "Delete \"{}\"? This kills its tmux session and removes the worktree. [y/N] ",
                        info.title
                    );
                    std::io::stdout().flush()?;
                    let mut answer = String::new();
                    std::io::stdin().read_line(&mut answer)?;
                    if !claude_commander::cli::parse_yes_no(&answer) {
                        println!("Aborted.");
                        return Ok(());
                    }
                }
                claude_commander::cli::DeleteGuard::Proceed => {}
            }

            service.delete_session(&info.session_id).await?;
            println!("Session deleted: {}", info.title);
        }

        Some(Commands::Log { session, lines }) => {
            setup_logging(cli.debug, false)?;

            let service = claude_commander::api::CommanderService::for_cli(config)?;
            match service.get_pane_content(&session, Some(lines)).await? {
                Some(content) => {
                    if content.ends_with('\n') {
                        print!("{}", content);
                    } else {
                        println!("{}", content);
                    }
                }
                None => {
                    eprintln!("Session not found or has no live tmux session: {}", session);
                    eprintln!("Use 'claude-commander list' to see available sessions.");
                    std::process::exit(1);
                }
            }
        }

        Some(Commands::New {
            name,
            program,
            path,
            initial_prompt,
            effort,
            mode,
            base_branch,
            section,
        }) => {
            setup_logging(cli.debug, false)?;

            let service = claude_commander::api::CommanderService::for_cli(config)?;

            println!("Creating session '{}'...", name);
            let session_id = match service
                .create_session(claude_commander::api::CreateSessionOpts {
                    project_path: path
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
                    title: name,
                    program,
                    initial_prompt,
                    effort,
                    mode,
                    base_branch,
                    section,
                })
                .await
            {
                Ok(id) => id,
                Err(claude_commander::Error::Session(
                    claude_commander::error::SessionError::InvalidProgram(msg),
                )) => {
                    clap::Error::raw(clap::error::ErrorKind::ArgumentConflict, format!("{msg}\n"))
                        .exit();
                }
                Err(e) => return Err(e.into()),
            };

            println!("Session created: {}", session_id);
            println!();
            println!("Attach with: claude-commander attach {}", session_id);
        }

        Some(Commands::Attach { session }) => {
            setup_logging(cli.debug, false)?;

            let app_state = AppState::load_or_exit();

            match claude_commander::cli::find_session(&app_state, &session) {
                Some(s) => {
                    let triggers = claude_commander::editor_trigger_bytes(&config.keybindings);
                    execute_attach(&s.tmux_session_name, triggers).await;
                }
                None => {
                    eprintln!("Session not found: {}", session);
                    eprintln!("Use 'claude-commander list' to see available sessions.");
                }
            }
        }

        Some(Commands::Commander) => {
            setup_logging(cli.debug, false)?;

            // One-shot CLI process: a bare executor is sufficient (no shared
            // semaphore to honour as there is in the long-lived TUI). The
            // enable gate lives in `ensure_session`, so the disabled case
            // surfaces as a typed error rather than an inline check here.
            let tmux = claude_commander::tmux::TmuxExecutor::new();
            let cmd = cli_command();
            match claude_commander::commander::ensure_session(&config, &tmux, &cmd).await {
                Ok(name) => {
                    let triggers = claude_commander::editor_trigger_bytes(&config.keybindings);
                    execute_attach(&name, triggers).await;
                }
                Err(
                    e @ claude_commander::Error::Session(
                        claude_commander::error::SessionError::CommanderDisabled,
                    ),
                ) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                Err(e) => return Err(e.into()),
            }
        }

        Some(Commands::ListenToggle { start, stop }) => {
            setup_logging(cli.debug, false)?;

            use claude_commander::conversation::{ListenAction, ipc};
            let action = if start {
                ListenAction::Start
            } else if stop {
                ListenAction::Stop
            } else {
                ListenAction::Toggle
            };
            match ipc::send_default(action).await {
                Ok(reply) => println!("{reply}"),
                Err(e) => {
                    eprintln!(
                        "Could not reach claude-commander: {e}\n\
                         Is the TUI running with STT enabled?"
                    );
                    std::process::exit(1);
                }
            }
        }

        Some(Commands::PickSession { out, current }) => {
            // No logging — the popup terminal is the picker's UI.
            claude_commander::picker::run_session_picker(&out, current.as_deref())?;
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
