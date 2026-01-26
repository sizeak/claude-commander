//! Claude Commander - Terminal UI for managing Claude coding sessions
//!
//! Run with `claude-commander` or `claude-commander --help` for usage.

use clap::{Parser, Subcommand};
use color_eyre::eyre::Result;
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use claude_commander::{config::Config, APP_NAME, VERSION};

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

fn setup_logging(debug: bool) -> Result<()> {
    let filter = if debug {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
            .add_directive("gix=warn".parse()?)
            .add_directive("tokio=warn".parse()?)
    };

    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false))
        .with(filter)
        .init();

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install color-eyre error hooks
    color_eyre::install()?;

    let cli = Cli::parse();

    setup_logging(cli.debug)?;

    // Load configuration
    let config = Config::load().unwrap_or_else(|e| {
        tracing::warn!("Failed to load config, using defaults: {}", e);
        Config::default()
    });

    // Ensure required directories exist
    if let Err(e) = config.ensure_directories() {
        tracing::warn!("Failed to create directories: {}", e);
    }

    match cli.command {
        None | Some(Commands::Tui) => {
            info!("Starting Claude Commander TUI v{}", VERSION);
            // TODO: Start TUI
            println!("TUI not yet implemented. Run with --help for available commands.");
        }

        Some(Commands::List { all }) => {
            info!("Listing sessions (all: {})", all);
            // TODO: Implement list command
            println!("List command not yet implemented.");
        }

        Some(Commands::New {
            name,
            program,
            path,
        }) => {
            let program = program.unwrap_or_else(|| config.default_program.clone());
            let path = path.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            info!(
                "Creating new session '{}' with program '{}' in {:?}",
                name, program, path
            );
            // TODO: Implement new session
            println!("New session command not yet implemented.");
        }

        Some(Commands::Attach { session }) => {
            info!("Attaching to session '{}'", session);
            // TODO: Implement attach
            println!("Attach command not yet implemented.");
        }

        Some(Commands::Config { init }) => {
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
            }
        }
    }

    Ok(())
}
