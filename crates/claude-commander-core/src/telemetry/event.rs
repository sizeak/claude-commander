//! Telemetry event payloads: the **fixed, typed** data we are willing to send.
//!
//! Privacy is enforced *by construction*: every field below is a declared
//! struct field with a known, non-sensitive meaning. There is no map of
//! arbitrary keys and no code path that forwards a user-supplied string, so we
//! cannot accidentally ship typed text, prompts, branch/session names, repo
//! paths, or arbitrary environment variables. Serializing one of these structs
//! emits *only* its declared fields — a property the unit tests pin down.

use serde::Serialize;

use crate::config::{Config, ViewMode};

/// Coarse description of the host environment. Strictly an allowlist — we never
/// iterate `std::env::vars()`, only read the specific variables below.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnvFingerprint {
    /// Compile-time target OS (`linux`, `macos`, …) via `std::env::consts::OS`.
    pub os: String,
    /// Compile-time target arch (`x86_64`, `aarch64`, …).
    pub arch: String,
    /// Terminal program: `TERM_PROGRAM` if set, else `TERM`. `None` if neither.
    pub terminal: Option<String>,
    /// Shell *basename* only (e.g. `zsh`), never the full path.
    pub shell: Option<String>,
    /// Detected terminal colour capability (`basic`/`indexed`/`truecolor`).
    pub color_mode: Option<String>,
}

impl EnvFingerprint {
    /// Collect the fingerprint from the live environment. `color_mode` is
    /// supplied by the caller when available (the service detects it via
    /// `ColorMode::detect`); pass `None` when no terminal colour capability is
    /// known.
    pub fn collect(color_mode: Option<&str>) -> Self {
        let terminal = std::env::var("TERM_PROGRAM")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("TERM").ok())
            .filter(|s| !s.is_empty());
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| basename(&s));
        Self {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            terminal,
            shell,
            color_mode: color_mode.map(str::to_string),
        }
    }
}

/// Non-sensitive snapshot of config choices, for popularity analysis
/// (e.g. "most-used theme"). Only enums, booleans, and de-pathed program names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigSnapshot {
    /// Theme preset name (`monokai-dimmed`, `rose-pine`, …) or `None` for auto.
    pub theme_preset: Option<String>,
    /// Active session-list view mode.
    pub view_mode: Option<String>,
    /// Bare program name only (e.g. `claude`) — flags and any path stripped.
    pub default_program: String,
    pub commander_enabled: bool,
    pub conversation_enabled: bool,
    pub stt_enabled: bool,
    pub ai_summary_enabled: bool,
    pub nix_develop: bool,
    pub rounded_borders: bool,
    /// Whether the user has configured any custom list sections.
    pub sections_configured: bool,
}

impl ConfigSnapshot {
    pub fn from_config(config: &Config, view_mode: Option<ViewMode>) -> Self {
        Self {
            theme_preset: config.theme.preset.clone(),
            view_mode: view_mode.map(view_mode_name).map(str::to_string),
            default_program: program_basename(&config.default_session_program()),
            commander_enabled: config.commander_enabled,
            conversation_enabled: config.conversation.enabled,
            stt_enabled: config.stt.enabled,
            ai_summary_enabled: config.ai_summary_enabled,
            nix_develop: config.nix_develop,
            rounded_borders: config.rounded_borders,
            sections_configured: !config.sections.is_empty(),
        }
    }
}

/// Stable identifier for a [`ViewMode`], independent of display labels (which
/// may change). Used as the logged value.
fn view_mode_name(mode: ViewMode) -> &'static str {
    match mode {
        ViewMode::ProjectGrouped => "ProjectGrouped",
        ViewMode::SectionGrouped => "SectionGrouped",
        ViewMode::SectionStacks => "SectionStacks",
    }
}

/// Last path component of `s` (works for both `/usr/bin/zsh` and a bare `zsh`).
fn basename(s: &str) -> String {
    std::path::Path::new(s)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| s.to_string())
}

/// The bare program name from a command line: first whitespace-delimited token,
/// de-pathed. `"/home/me/bin/claude --model opus /secret"` → `"claude"`. This
/// deliberately drops flags and any directory component so a custom program
/// path can't leak a home directory or arguments.
fn program_basename(program: &str) -> String {
    let first = program.split_whitespace().next().unwrap_or("");
    if first.is_empty() {
        return String::new();
    }
    basename(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// The allowlist made executable: an `EnvFingerprint` serializes to exactly
    /// these keys and nothing else. If someone adds a field that captures a raw
    /// env var, this fails — the privacy guard.
    #[test]
    fn env_fingerprint_serializes_only_allowlisted_keys() {
        let fp = EnvFingerprint {
            os: "linux".into(),
            arch: "x86_64".into(),
            terminal: Some("tmux".into()),
            shell: Some("zsh".into()),
            color_mode: Some("truecolor".into()),
        };
        let Value::Object(map) = serde_json::to_value(&fp).unwrap() else {
            panic!("expected object");
        };
        let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["arch", "color_mode", "os", "shell", "terminal"]);
    }

    #[test]
    fn shell_is_reduced_to_basename() {
        assert_eq!(basename("/usr/bin/zsh"), "zsh");
        assert_eq!(basename("/opt/homebrew/bin/fish"), "fish");
        assert_eq!(basename("bash"), "bash");
    }

    #[test]
    fn program_basename_strips_path_and_flags() {
        assert_eq!(program_basename("claude"), "claude");
        assert_eq!(program_basename("claude --model opus-4-8"), "claude");
        // The privacy case: a custom path with secret args must not leak.
        assert_eq!(
            program_basename("/home/alice/bin/claude --resume /home/alice/secret-notes"),
            "claude"
        );
        assert_eq!(program_basename(""), "");
    }

    #[test]
    fn config_snapshot_maps_fields_and_omits_paths() {
        let mut config = Config::default();
        config.theme.preset = Some("rose-pine".into());
        config.programs = vec![crate::config::ProgramEntry {
            label: "Claude Opus".into(),
            command: "/home/alice/bin/claude --model opus".into(),
        }];
        config.commander_enabled = true;
        config.conversation.enabled = true;
        // A path that must never appear in the snapshot.
        config.worktrees_dir = Some(std::path::PathBuf::from("/home/alice/secret-worktrees"));

        let snap = ConfigSnapshot::from_config(&config, Some(ViewMode::SectionStacks));
        assert_eq!(snap.theme_preset.as_deref(), Some("rose-pine"));
        assert_eq!(snap.view_mode.as_deref(), Some("SectionStacks"));
        assert_eq!(snap.default_program, "claude");
        assert!(snap.commander_enabled);
        assert!(snap.conversation_enabled);

        // By construction the snapshot has no path-typed field; prove the
        // serialized form contains no trace of the planted secret path.
        let json = serde_json::to_string(&snap).unwrap();
        assert!(
            !json.contains("secret-worktrees") && !json.contains("/home/alice"),
            "config snapshot leaked a filesystem path: {json}"
        );
    }
}
