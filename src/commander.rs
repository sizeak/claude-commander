//! Persistent top-level "commander" session.
//!
//! The commander is a long-lived Claude session that is not tied to any
//! project or worktree. It coordinates other sessions by driving the
//! `claude-commander` CLI. This module owns the commander's scratch directory
//! and the priming files Claude reads there.
//!
//! Layering: the pure helpers (`claude_md_content`, `generate_cli_reference`,
//! `seed_notes_md`, `write_claude_md`, `plan_session_action`) carry the logic
//! and are unit-tested here. `ensure_session`/`is_running` are thin IO wrappers
//! over tmux and are exercised by tmux-gated integration tests.

use std::path::Path;

use crate::config::Config;
use crate::error::{Result, SessionError};
use crate::tmux::TmuxExecutor;

/// tmux session name for the singleton commander session.
pub const COMMANDER_TMUX_NAME: &str = "cc-commander";

/// Handwritten role/safety preamble. Ships with each build; humans edit this
/// when the commander's intent changes. The live CLI reference is appended at
/// runtime so the generated `CLAUDE.md` never drifts from the actual commands.
const COMMANDER_PRIME: &str = include_str!("commander_prime.md");

/// Seed contents for `NOTES.md`. Written only if the file is absent, so the
/// commander's accumulated notes are never clobbered.
const NOTES_SEED: &str = "# Commander notes\n\n\
    Long-term notes for the commander session live here. \
    Claude Commander never touches this file.\n\n\
    ## Active workflows\n";

/// Build the full `CLAUDE.md` body: the handwritten preamble followed by a
/// CLI reference generated from the live clap command tree.
pub fn claude_md_content(cmd: &clap::Command) -> String {
    format!(
        "{}\n{}",
        COMMANDER_PRIME.trim_end(),
        generate_cli_reference(cmd)
    )
}

/// Render a markdown CLI reference by walking the clap command tree and
/// emitting each visible subcommand's long help verbatim. Hidden subcommands
/// (e.g. internal popup helpers) are skipped.
fn generate_cli_reference(cmd: &clap::Command) -> String {
    let bin = cmd.get_name().to_string();
    let mut out = String::new();
    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() {
            continue;
        }
        out.push_str(&format!("### `{bin} {}`\n\n", sub.get_name()));
        let help = sub.clone().render_long_help();
        out.push_str("```\n");
        out.push_str(help.to_string().trim_end());
        out.push_str("\n```\n\n");
    }
    out
}

/// Write `CLAUDE.md` into the commander directory, overwriting any existing
/// copy (the file is owned by Claude Commander, not the commander session).
pub fn write_claude_md(dir: &Path, cmd: &clap::Command) -> Result<()> {
    std::fs::write(dir.join("CLAUDE.md"), claude_md_content(cmd))?;
    Ok(())
}

/// Create `NOTES.md` with the seed skeleton if it does not already exist.
/// Existing notes are left untouched.
pub fn seed_notes_md(dir: &Path) -> Result<()> {
    let path = dir.join("NOTES.md");
    if !path.exists() {
        std::fs::write(&path, NOTES_SEED)?;
    }
    Ok(())
}

/// What `ensure_session` must do to the commander tmux session, given its
/// current state. Pure decision split out from the IO so the lifecycle logic
/// is unit-testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionAction {
    /// No session exists — create one.
    Create,
    /// A live session exists — reuse it.
    Reuse,
    /// A dead pane lingers (`remain-on-exit`) — kill it and create fresh,
    /// otherwise the user would re-attach to a corpse.
    RecreateDead,
}

/// Decide the action for the commander session from observed tmux state.
/// `pane_dead` is only meaningful when `exists` is true.
fn plan_session_action(exists: bool, pane_dead: bool) -> SessionAction {
    match (exists, pane_dead) {
        (false, _) => SessionAction::Create,
        (true, false) => SessionAction::Reuse,
        (true, true) => SessionAction::RecreateDead,
    }
}

/// Ensure the commander session is ready to attach to, idempotently:
/// refresh `CLAUDE.md`, seed `NOTES.md`, and spawn (or respawn a dead) tmux
/// session. Returns the tmux session name.
///
/// `CLAUDE.md` is rewritten on every call so the CLI reference stays current;
/// `NOTES.md` is only seeded when absent so accumulated notes survive.
///
/// Returns [`SessionError::CommanderDisabled`] when the feature is off. This is
/// the single enforcement point for the enable gate — every caller (CLI, TUI)
/// routes through here, so a new caller cannot accidentally bypass the check.
pub async fn ensure_session(
    config: &Config,
    tmux: &TmuxExecutor,
    cmd: &clap::Command,
) -> Result<String> {
    if !config.commander_enabled {
        return Err(SessionError::CommanderDisabled.into());
    }

    // Friendly error on tmux-less machines, matching the create-session path.
    tmux.check_installed().await?;

    let dir = config.commander_dir()?;
    std::fs::create_dir_all(&dir)?;
    write_claude_md(&dir, cmd)?;
    seed_notes_md(&dir)?;

    let exists = tmux.session_exists(COMMANDER_TMUX_NAME).await?;
    let pane_dead = if exists {
        tmux.is_pane_dead(COMMANDER_TMUX_NAME).await?
    } else {
        false
    };

    let action = plan_session_action(exists, pane_dead);
    if action == SessionAction::RecreateDead {
        tmux.kill_session(COMMANDER_TMUX_NAME).await?;
    }
    if action != SessionAction::Reuse {
        tmux.create_session(COMMANDER_TMUX_NAME, &dir, Some(&config.commander_program()))
            .await?;
    }

    Ok(COMMANDER_TMUX_NAME.to_string())
}

/// Whether a *live* commander session is currently running. A dead pane left
/// behind by `remain-on-exit` counts as not running. Any tmux error is treated
/// as "not running" so callers never claim a commander we cannot confirm.
pub async fn is_running(tmux: &TmuxExecutor) -> bool {
    match tmux.session_exists(COMMANDER_TMUX_NAME).await {
        Ok(true) => !tmux.is_pane_dead(COMMANDER_TMUX_NAME).await.unwrap_or(true),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A miniature clap command tree mirroring the shape of the real CLI:
    /// a couple of visible subcommands (one with args) plus a hidden one.
    fn sample_cli() -> clap::Command {
        clap::Command::new("claude-commander")
            .subcommand(clap::Command::new("list").about("List all sessions"))
            .subcommand(
                clap::Command::new("new")
                    .about("Create a new session")
                    .arg(clap::Arg::new("name").help("Session name")),
            )
            .subcommand(clap::Command::new("pick-session").hide(true))
    }

    #[test]
    fn cli_reference_includes_visible_subcommands() {
        let reference = generate_cli_reference(&sample_cli());
        assert!(reference.contains("claude-commander list"));
        assert!(reference.contains("List all sessions"));
        assert!(reference.contains("claude-commander new"));
        assert!(reference.contains("Create a new session"));
    }

    #[test]
    fn cli_reference_skips_hidden_subcommands() {
        let reference = generate_cli_reference(&sample_cli());
        assert!(
            !reference.contains("pick-session"),
            "hidden subcommands must not leak into the CLI reference"
        );
    }

    #[test]
    fn claude_md_starts_with_prime_and_appends_reference() {
        let content = claude_md_content(&sample_cli());
        assert!(
            content.starts_with("# Commander Claude"),
            "generated CLAUDE.md must lead with the handwritten preamble"
        );
        // The safety boundary from the preamble survives.
        assert!(content.contains("cannot do"));
        // The live CLI reference is appended.
        assert!(content.contains("claude-commander list"));
    }

    #[test]
    fn write_claude_md_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CLAUDE.md");
        std::fs::write(&path, "stale content the commander scribbled").unwrap();

        write_claude_md(dir.path(), &sample_cli()).unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        assert!(!written.contains("stale content"));
        assert!(written.starts_with("# Commander Claude"));
    }

    #[test]
    fn seed_notes_md_creates_when_absent() {
        let dir = TempDir::new().unwrap();
        seed_notes_md(dir.path()).unwrap();
        let written = std::fs::read_to_string(dir.path().join("NOTES.md")).unwrap();
        assert!(written.starts_with("# Commander notes"));
    }

    // -- lifecycle decision (plan_session_action) --
    //
    // `create_session` enables `remain-on-exit on` globally, so a commander
    // whose Claude has exited leaves a *dead but still-existing* pane. These
    // cases pin down the only correct reaction for each (exists, dead) state.

    #[test]
    fn plan_creates_when_no_session() {
        assert_eq!(plan_session_action(false, false), SessionAction::Create);
    }

    #[test]
    fn plan_creates_when_no_session_ignoring_dead_flag() {
        // `dead` is meaningless when the session does not exist; must still create.
        assert_eq!(plan_session_action(false, true), SessionAction::Create);
    }

    #[test]
    fn plan_reuses_live_session() {
        assert_eq!(plan_session_action(true, false), SessionAction::Reuse);
    }

    #[test]
    fn plan_recreates_dead_pane() {
        // The corpse-reattach bug: existing + dead must kill & recreate, never reuse.
        assert_eq!(plan_session_action(true, true), SessionAction::RecreateDead);
    }

    #[tokio::test]
    async fn ensure_session_errors_when_disabled() {
        // `commander_enabled` defaults to false. The guard must fire before any
        // tmux work, so this passes even on a machine with no tmux server.
        let config = Config::default();
        assert!(!config.commander_enabled);
        let tmux = TmuxExecutor::new();
        let err = ensure_session(&config, &tmux, &sample_cli())
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                crate::error::Error::Session(SessionError::CommanderDisabled)
            ),
            "disabled commander must short-circuit with CommanderDisabled, got {err:?}"
        );
    }

    #[test]
    fn seed_notes_md_does_not_overwrite_existing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("NOTES.md");
        std::fs::write(&path, "## my own notes\nimportant").unwrap();

        seed_notes_md(dir.path()).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, "## my own notes\nimportant");
    }
}
