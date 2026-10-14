//! Command-line argument definitions for this binary.
//!
//! These live in the binary crate because a clap derive takes its program name
//! and version from the package it compiles in: defined here, `--version`
//! reports `claude-commander <version>` with nothing hardcoded and nothing for
//! a rename or a version bump to get wrong. (In the library they resolved to
//! `claude-commander-core`, which also leaked into the generated commander CLI
//! reference as unrunnable `claude-commander-core <sub>` invocations.)
//!
//! Rendering the CLI reference lives here too, for the same reason: walking a
//! clap tree means depending on clap, and only a binary has a CLI to describe.
//! The rendered markdown is committed as core's bundled
//! `claude-commander-cli-reference` agent skill and kept fresh by the
//! `skills_gen` drift test, so core stays clap-free and embedders of the
//! library don't inherit a CLI parser they have no use for. The renderer is
//! therefore test-only.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version)]
#[command(about = "A high-performance terminal UI for managing Claude coding sessions")]
#[command(long_about = None)]
pub struct Cli {
    /// Enable debug logging
    #[arg(short, long)]
    pub debug: bool,

    /// Path to config file
    #[arg(short, long)]
    pub config: Option<std::path::PathBuf>,

    /// Run the HTTP server in this process for as long as the TUI is open, so
    /// clients on this machine (or the LAN, with `[server] bind`) can reach it
    /// without starting `claude-commander-server` separately. Overrides
    /// `[server] auto_start` for one run. TUI only.
    #[arg(long, conflicts_with = "no_serve")]
    pub serve: bool,

    /// Do not run the HTTP server, even if `[server] auto_start` is on.
    /// TUI only.
    #[arg(long)]
    pub no_serve: bool,

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

        /// Only list sessions in this workspace (by name, case-insensitive;
        /// `main` or Main's label for the built-in workspace).
        #[arg(short = 'w', long)]
        workspace: Option<String>,
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
        /// Exact session title (case-insensitive) or full session ID. Prefixes
        /// are rejected so a typo can't delete the wrong session
        session: String,

        /// Delete without confirmation
        #[arg(short, long)]
        force: bool,
    },

    /// Toggle (or explicitly set) keep-alive on a session, exempting it from
    /// automatic idle-hibernation. With no flag, toggles the current value.
    KeepAlive {
        /// Exact session title (case-insensitive) or full session ID
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

        /// Model to run the agent with (Claude, Codex, OpenCode or omp)
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

        /// Chat backend the session was requested from (e.g. `slack`), recorded
        /// as its origin so a worker's `notify` reports back to the thread. Set
        /// by the commander agent when creating a session from a chat request.
        /// Requires `--origin-channel` and `--origin-thread`.
        #[arg(long, requires_all = ["origin_channel", "origin_thread"])]
        origin_backend: Option<String>,

        /// Conversation the session was requested in (a Slack channel id, …).
        /// Requires `--origin-backend`.
        #[arg(long, requires = "origin_backend")]
        origin_channel: Option<String>,

        /// Thread within that conversation that replies go to (a Slack thread
        /// timestamp, …). Requires `--origin-backend`.
        #[arg(long, requires = "origin_backend")]
        origin_thread: Option<String>,

        /// Link to the originating message, when the platform has one.
        /// Requires `--origin-backend`.
        #[arg(long, requires = "origin_backend")]
        origin_permalink: Option<String>,
        /// Workspace for the project when `--path` registers a new one (an
        /// existing project keeps its workspace). An unknown name creates the
        /// workspace; `main` is the built-in one.
        #[arg(short = 'w', long, requires = "path")]
        workspace: Option<String>,
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

    /// Relay a message to chat about a session. A session created from a chat
    /// request posts back into its originating thread; any other session is
    /// sent as a direct message on the default chat backend. Worker sessions run
    /// this to report progress. The message is Markdown; the chat backend
    /// converts it.
    ///
    /// Requires a running server with a chat backend enabled — the TUI with
    /// `[server] auto_start` (or `--serve`), or `claude-commander-server` —
    /// discovered via its runtime info file.
    Notify {
        /// Session to notify about (name or id prefix). Defaults to the session
        /// whose worktree contains the current directory.
        #[arg(long)]
        session: Option<String>,

        /// Message text. If omitted, the message is read from stdin to EOF.
        #[arg(short, long)]
        message: Option<String>,
    },

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
}

/// The clap command tree for the CLI. Single source of truth for this binary's
/// argument parsing and for the CLI reference generated from it.
#[cfg(test)]
pub fn cli_command() -> clap::Command {
    <Cli as clap::CommandFactory>::command()
}

/// Markdown documenting this CLI: the body of the bundled
/// `claude-commander-cli-reference` agent skill (see `skills_gen`).
#[cfg(test)]
pub fn cli_reference() -> String {
    generate_cli_reference(&cli_command())
}

/// Render a markdown CLI reference by walking a clap command tree and emitting
/// each visible subcommand's long help verbatim, recursing into command groups
/// (a group's subcommands), labelled with the full invocation. Hidden subcommands
/// (e.g. internal popup helpers) and clap's generated `help` are skipped.
///
/// Split from [`cli_reference`] so the walk is testable against a synthetic tree
/// with a known shape.
#[cfg(test)]
fn generate_cli_reference(cmd: &clap::Command) -> String {
    // Building propagates each subcommand's full `bin_name`, so its usage line
    // reads `Usage: claude-commander new …` rather than a bare `Usage: new`.
    let mut cmd = cmd.clone();
    cmd.build();
    // The program itself first: its usage, subcommand list and top-level
    // options (`--serve`, `--debug`, …), which no subcommand section repeats.
    let mut out = format!(
        "### `{}`\n\n```\n{}\n```\n\n",
        cmd.get_name(),
        cmd.clone().render_long_help().to_string().trim_end()
    );
    render_subcommands(&cmd, &mut out);
    out
}

#[cfg(test)]
fn render_subcommands(cmd: &clap::Command, out: &mut String) {
    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() || sub.get_name() == "help" {
            continue;
        }
        let invocation = sub.get_bin_name().unwrap_or(sub.get_name());
        out.push_str(&format!("### `{invocation}`\n\n"));
        let help = sub.clone().render_long_help();
        out.push_str("```\n");
        out.push_str(help.to_string().trim_end());
        out.push_str("\n```\n\n");
        render_subcommands(sub, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_flag_reports_this_packages_name_and_version() {
        // The string a user actually sees. Both halves come from *this*
        // package's metadata, which only holds while the derive lives in the
        // binary crate — from the library it read `claude-commander-core`.
        let err = cli_command()
            .try_get_matches_from(["claude-commander", "--version"])
            .expect_err("--version short-circuits parsing");
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
        assert_eq!(
            err.to_string().trim(),
            format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
        );
        // Pin the literal too: `CARGO_PKG_NAME` alone would still match if this
        // package were renamed, and `claude-commander` is the installed
        // command name that docs, the Homebrew formula, and the AUR package use.
        assert_eq!(
            err.to_string().trim(),
            format!("claude-commander {}", crate::VERSION)
        );
    }

    #[test]
    fn cli_reference_documents_runnable_invocations() {
        // The commander session reads this reference as instructions, so a wrong
        // program name is a documented command that doesn't exist — the second,
        // quieter symptom of the library-side derive.
        let reference = cli_reference();
        assert!(
            reference.contains("claude-commander list"),
            "reference must document runnable invocations; got:\n{reference}"
        );
        assert!(
            !reference.contains("claude-commander-core"),
            "the library crate name must never reach the commander's CLI reference"
        );
    }

    /// A miniature clap command tree mirroring the shape of the real CLI:
    /// a couple of visible subcommands (one with args) plus a hidden one.
    fn sample_cli() -> clap::Command {
        clap::Command::new("sample-bin")
            .subcommand(clap::Command::new("list").about("List all sessions"))
            .subcommand(
                clap::Command::new("new")
                    .about("Create a new session")
                    .arg(clap::Arg::new("name").help("Session name")),
            )
            .subcommand(clap::Command::new("hidden-helper").hide(true))
    }

    #[test]
    fn cli_reference_includes_visible_subcommands() {
        let reference = generate_cli_reference(&sample_cli());
        assert!(reference.contains("sample-bin list"));
        assert!(reference.contains("List all sessions"));
        assert!(reference.contains("sample-bin new"));
        assert!(reference.contains("Create a new session"));
    }

    /// The reference claims to cover every flag, so the top-level options
    /// (`--serve`, `--debug`, …) need a section of their own.
    #[test]
    fn cli_reference_documents_the_top_level_options() {
        let reference = cli_reference();
        assert!(
            reference.starts_with("### `claude-commander`"),
            "{reference}"
        );
        assert!(reference.contains("--serve"), "{reference}");
    }

    #[test]
    fn cli_reference_usage_lines_name_the_full_invocation() {
        // An agent copies the usage line; `Usage: list` is not a command it can run.
        let reference = generate_cli_reference(&sample_cli());
        assert!(
            reference.contains("Usage: sample-bin list"),
            "usage must carry the binary name; got:\n{reference}"
        );
    }

    #[test]
    fn cli_reference_documents_nested_subcommands() {
        let cmd = sample_cli().subcommand(
            clap::Command::new("group")
                .about("A command group")
                .subcommand(
                    clap::Command::new("leaf")
                        .about("The nested command")
                        .arg(clap::Arg::new("target").long("target").help("Leaf target")),
                ),
        );
        let reference = generate_cli_reference(&cmd);
        assert!(
            reference.contains("### `sample-bin group leaf`"),
            "{reference}"
        );
        assert!(
            reference.contains("Usage: sample-bin group leaf"),
            "{reference}"
        );
        assert!(reference.contains("--target"), "{reference}");
        // clap's auto-generated `help` subcommand is noise, not a command.
        assert!(
            !reference.contains("### `sample-bin group help`"),
            "{reference}"
        );
    }

    #[test]
    fn cli_reference_skips_hidden_subcommands() {
        let reference = generate_cli_reference(&sample_cli());
        assert!(
            !reference.contains("hidden-helper"),
            "hidden subcommands must not leak into the CLI reference"
        );
    }

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
            "notify",
        ] {
            assert!(
                names.contains(&expected),
                "cli_command() missing subcommand `{expected}`; got {names:?}"
            );
        }
    }

    /// The `new` command carries the chat-origin flags the commander agent
    /// uses when creating a session from a chat request.
    #[test]
    fn new_command_exposes_the_origin_flags() {
        let cmd = cli_command();
        let new = cmd
            .get_subcommands()
            .find(|s| s.get_name() == "new")
            .expect("new subcommand present");
        let args: Vec<&str> = new.get_arguments().filter_map(|a| a.get_long()).collect();
        for expected in [
            "origin-backend",
            "origin-channel",
            "origin-thread",
            "origin-permalink",
        ] {
            assert!(
                args.contains(&expected),
                "new missing --{expected}; got {args:?}"
            );
        }
    }

    /// An origin is all-or-nothing: backend, channel and thread together.
    #[test]
    fn origin_flags_must_come_together() {
        let parse = |extra: &[&str]| {
            let mut argv = vec!["claude-commander", "new", "x"];
            argv.extend_from_slice(extra);
            cli_command().try_get_matches_from(argv)
        };
        assert!(
            parse(&[
                "--origin-backend",
                "slack",
                "--origin-channel",
                "C1",
                "--origin-thread",
                "1.2"
            ])
            .is_ok()
        );
        assert!(parse(&["--origin-backend", "slack", "--origin-channel", "C1"]).is_err());
        assert!(parse(&["--origin-thread", "1.2"]).is_err());
        assert!(parse(&["--origin-permalink", "https://x"]).is_err());
    }

    #[test]
    fn cli_command_parses_without_panicking() {
        // `Cli::command()` panics at runtime if the derive is malformed
        // (duplicate args, bad short flags, etc.). Exercising debug_assert
        // here turns that into a test failure rather than a surprise at launch.
        cli_command().debug_assert();
    }
}
