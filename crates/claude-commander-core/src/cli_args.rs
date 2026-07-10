//! Command-line argument definitions.
//!
//! These live in the library (rather than `main.rs`) so that library code —
//! notably the commander module, which generates a CLI reference for the
//! commander session's `CLAUDE.md` — can obtain the exact clap command tree
//! via [`cli_command`]. The binary (`main.rs`) imports these and owns the
//! dispatch `match`.

use clap::{CommandFactory, Parser, Subcommand};

use crate::{APP_NAME, VERSION};

#[derive(Parser)]
#[command(name = APP_NAME)]
#[command(version = VERSION)]
#[command(about = "A high-performance terminal UI for managing Claude coding sessions")]
#[command(long_about = None)]
pub struct Cli {
    /// Enable debug logging
    #[arg(short, long)]
    pub debug: bool,

    /// Path to config file
    #[arg(short, long)]
    pub config: Option<std::path::PathBuf>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

// A clap subcommand enum is parsed once at startup and never stored in bulk, so
// the size gap between the arg-heavy `New` variant and the unit variants is
// immaterial — boxing a variant would only fight clap's derive for no runtime
// benefit.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum Commands {
    /// Start the interactive TUI (default)
    Tui,

    /// List all sessions
    List {
        /// Show all sessions including stopped ones
        #[arg(short, long)]
        all: bool,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Show detailed status of a session
    Status {
        /// Session name or ID prefix
        session: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Delete a session: kill its tmux session and remove the git worktree.
    #[command(visible_alias = "rm")]
    Delete {
        /// Session name or ID prefix
        session: String,

        /// Delete without confirmation
        #[arg(short, long)]
        force: bool,
    },

    /// Toggle (or explicitly set) keep-alive on a session, exempting it from
    /// automatic idle-hibernation. With no flag, toggles the current value.
    KeepAlive {
        /// Session name or ID prefix
        session: String,

        /// Turn keep-alive on (never auto-hibernate this session)
        #[arg(long, conflicts_with = "off")]
        on: bool,

        /// Turn keep-alive off (allow auto-hibernation when idle)
        #[arg(long, conflicts_with = "on")]
        off: bool,
    },

    /// Dump recent terminal output from a session
    Log {
        /// Session name or ID prefix
        session: String,

        /// Number of scrollback lines to capture (default: 100, max: 10000)
        #[arg(short, long, default_value_t = 100)]
        lines: usize,
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

        /// Existing project to create the session in, by name (case-insensitive).
        /// Resolves to the project's repo path via the backend, so a remote
        /// project needs no `--path`. If the name matches more than one project
        /// (names aren't unique), use `--path` to disambiguate. Mutually
        /// exclusive with `--path` (which seeds a new project).
        #[arg(long, conflicts_with = "path")]
        project: Option<String>,

        /// Initial prompt to send to the Claude agent
        #[arg(short = 'i', long)]
        initial_prompt: Option<String>,

        /// Claude effort level
        #[arg(short, long)]
        effort: Option<String>,

        /// Claude permission mode
        #[arg(short, long)]
        mode: Option<String>,

        /// Model to run the agent with (Claude or Codex)
        #[arg(long)]
        model: Option<String>,

        /// Branch to fork from (default: origin/main)
        #[arg(short = 'b', long)]
        base_branch: Option<String>,

        /// Place session in a specific section
        #[arg(short = 's', long)]
        section: Option<String>,

        /// Create the session on a configured remote server (by name from
        /// `[[remote_servers]]`) instead of locally. Pair with `--project` to
        /// pick an existing server-side project by name, or `--path` to seed a
        /// new one (the path is resolved on the server, not this machine).
        #[arg(long)]
        remote: Option<String>,
    },

    /// Attach to an existing session
    Attach {
        /// Session name or ID
        session: String,

        /// Attach to a session on a configured remote server (by name from
        /// `[[remote_servers]]`) instead of a local one.
        #[arg(long)]
        remote: Option<String>,
    },

    /// Open the persistent commander session — a project-less Claude session
    /// that coordinates other sessions via this CLI. Creates it on first use.
    Commander,

    /// Show configuration
    Config {
        /// Initialize config file with defaults
        #[arg(long)]
        init: bool,
    },

    /// Toggle voice input in the running TUI. Intended for binding to a desktop
    /// global keyboard shortcut (e.g. a KDE Plasma custom shortcut) so STT can be
    /// triggered system-wide, not just when the terminal is focused. Talks to the
    /// already-running instance over a local socket; with no flag it toggles.
    ListenToggle {
        /// Start recording (instead of toggling)
        #[arg(long, conflicts_with = "stop")]
        start: bool,

        /// Stop recording and transcribe (instead of toggling)
        #[arg(long, conflicts_with = "start")]
        stop: bool,
    },

    /// Show the in-session session picker (used by Ctrl+Space inside an attached
    /// session via `tmux display-popup`). Writes the chosen tmux session name
    /// to `--out` on selection; writes nothing on cancel.
    #[command(hide = true)]
    PickSession {
        /// Path to write the chosen tmux session name to
        #[arg(long)]
        out: std::path::PathBuf,
        /// tmux name of the currently-attached session — excluded from the
        /// picker list (Alt+Tab style; switching to where you already are
        /// is a no-op).
        #[arg(long)]
        current: Option<String>,
    },
}

/// The clap command tree for the CLI. Single source of truth shared by the
/// binary's argument parsing and any library code that needs to introspect
/// the available subcommands (e.g. commander CLAUDE.md generation).
pub fn cli_command() -> clap::Command {
    Cli::command()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_command_exposes_known_subcommands() {
        let cmd = cli_command();
        let names: Vec<&str> = cmd.get_subcommands().map(|s| s.get_name()).collect();
        for expected in [
            "list",
            "status",
            "log",
            "new",
            "attach",
            "config",
            "commander",
            "listen-toggle",
            "keep-alive",
        ] {
            assert!(
                names.contains(&expected),
                "cli_command() missing subcommand `{expected}`; got {names:?}"
            );
        }
    }

    #[test]
    fn cli_command_parses_without_panicking() {
        // `Cli::command()` panics at runtime if the derive is malformed
        // (duplicate args, bad short flags, etc.). Exercising debug_assert
        // here turns that into a test failure rather than a surprise at launch.
        cli_command().debug_assert();
    }
}
