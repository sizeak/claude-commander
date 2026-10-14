//! The generated `claude-commander-cli-reference` skill, and the test that keeps
//! it fresh.
//!
//! Core bundles the agent skills in `crates/claude-commander-core/skills/` and
//! writes them into every agent's working directory at launch. Most of them are
//! handwritten workflow guidance; this one is the CLI reference, rendered from
//! this binary's clap tree. Only this crate can render it (core is clap-free),
//! so the output is committed and [`cli_reference_skill_is_up_to_date`]
//! regenerates it and fails on any difference — a CLI change that forgets to
//! regenerate is a red test rather than a skill documenting flags that don't
//! exist. Same shape as the protocol crate's `ts_export.rs`.
//!
//! Regenerate with:
//!
//! ```text
//! CC_SKILLS_REGENERATE=1 cargo test -p claude-commander skills_gen
//! ```

use std::path::PathBuf;

use crate::cli_args::cli_reference;

/// Set to rewrite the committed skill instead of checking it.
const REGENERATE_ENV: &str = "CC_SKILLS_REGENERATE";

/// The generated skill's name — its directory and its frontmatter `name`.
const SKILL_NAME: &str = "claude-commander-cli-reference";

/// The committed skill file, relative to this crate.
fn committed_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../claude-commander-core/skills")
        .join(SKILL_NAME)
        .join("SKILL.md")
}

/// Render the skill: frontmatter an agent matches on, then the reference.
fn cli_reference_skill() -> String {
    format!(
        "---\n\
         name: {SKILL_NAME}\n\
         description: Complete reference for the `claude-commander` CLI — every subcommand and \
         flag, matching the installed version. Use it before running a `claude-commander` \
         command whose arguments you are not certain of.\n\
         ---\n\
         \n\
         <!-- Generated from crates/claude-commander/src/cli_args.rs by skills_gen.rs. \
         Do not edit; regenerate with: \
         CC_SKILLS_REGENERATE=1 cargo test -p claude-commander skills_gen -->\n\
         \n\
         # claude-commander CLI reference\n\
         \n\
         Each section is the `--help` output of the program or one of its subcommands.\n\
         \n\
         {}",
        cli_reference().trim_end()
    ) + "\n"
}

#[test]
fn cli_reference_skill_is_up_to_date() {
    let path = committed_path();
    let generated = cli_reference_skill();
    if std::env::var_os(REGENERATE_ENV).is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &generated).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == generated,
        "{} is stale; regenerate with `{REGENERATE_ENV}=1 cargo test -p claude-commander skills_gen`",
        path.display()
    );
}

#[test]
fn cli_reference_skill_documents_the_session_creation_flags() {
    let skill = cli_reference_skill();
    assert!(skill.starts_with(&format!("---\nname: {SKILL_NAME}\n")));
    for flag in ["--origin-backend", "--workspace", "--initial-prompt"] {
        assert!(skill.contains(flag), "reference is missing {flag}");
    }
}
