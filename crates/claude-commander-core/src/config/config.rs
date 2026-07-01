//! User configuration settings
//!
//! Layered configuration: defaults → config file

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers};
use directories::ProjectDirs;
use figment::{
    Figment,
    providers::{Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};

use crate::config::keybindings::KeyBindings;
use crate::config::theme::ThemeOverrides;
use crate::error::{ConfigError, Error, Result};

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Default program to run in new sessions
    pub default_program: String,

    /// Branch name prefix for new sessions (empty string means no prefix)
    pub branch_prefix: String,

    /// Maximum concurrent tmux commands
    pub max_concurrent_tmux: usize,

    /// Content capture cache TTL in milliseconds
    pub capture_cache_ttl_ms: u64,

    /// Diff cache TTL in milliseconds
    pub diff_cache_ttl_ms: u64,

    /// UI refresh rate in FPS
    pub ui_refresh_fps: u32,

    /// Path to worktrees directory
    pub worktrees_dir: Option<PathBuf>,

    /// Socket directory to isolate every tmux command commander spawns onto.
    ///
    /// For hermetic tests and the e2e harness only — leave unset (`None`) for
    /// normal use. When set, each spawned tmux command runs with
    /// `TMUX_TMPDIR=<dir>` and with `$TMUX`/`$TMUX_PANE` stripped, so it talks to
    /// a throwaway per-test tmux server instead of the developer's real one
    /// (tmux clients otherwise prefer an inherited live `$TMUX` over
    /// `TMUX_TMPDIR`). `None` touches the environment not at all.
    pub tmux_tmpdir: Option<PathBuf>,

    /// Organize worktrees into per-repository subdirectories
    pub per_repo_worktree_dirs: bool,

    /// Shell program for shell sessions
    pub shell_program: String,

    /// Interval in seconds between GitHub PR checks (0 = disabled)
    pub pr_check_interval_secs: u64,

    /// Enable periodic fast-forward of each project's main branch from origin.
    /// When disabled (default) no background fetch/FF runs on project branches.
    pub project_pull_enabled: bool,

    /// Interval in seconds between project-branch pulls. Minimum enforced
    /// at 60s by the settings UI to avoid thrashing.
    pub project_pull_interval_secs: u64,

    /// Label names that mark an open PR as awaiting reviewer action (case-insensitive).
    /// When any of these labels are present on an open PR, the PR badge is coloured
    /// with the "review" colour (light purple) instead of the regular open colour.
    #[serde(default = "default_pr_review_labels")]
    pub pr_review_labels: Vec<String>,

    /// Editor/IDE command for opening sessions (e.g. "code", "zed", "nvim")
    pub editor: Option<String>,

    /// Whether the editor is a GUI application (true) or terminal-based (false).
    /// If unset, auto-detected from a known list of GUI editors.
    pub editor_gui: Option<bool>,

    /// Fetch the latest changes from origin before creating a new session
    #[serde(alias = "pull_before_create")]
    pub fetch_before_create: bool,

    /// Pass `--resume` to the program when restarting or recreating a session,
    /// so the agent picks up where it left off. When false, the program is
    /// started fresh.
    pub resume_session: bool,

    /// Launch sessions inside `nix develop` when the project has a `flake.nix`
    /// at its root and `nix` is on PATH. Applies to Claude sessions and shell
    /// sessions alike. Default true; projects without a flake are unaffected.
    #[serde(default = "default_true")]
    pub nix_develop: bool,

    /// Interval in milliseconds for checking state file changes from other instances (0 = disabled)
    pub state_sync_interval_ms: u64,

    /// Interval in milliseconds for polling agent state (Working/Idle/Waiting) (0 = disabled)
    pub agent_state_poll_interval_ms: u64,

    /// When true, render PR labels as colored text on the default background
    /// (the pre-pill behavior). When false (default), PR labels render as a
    /// pill — colored background block with contrasting text — so they stand
    /// out more in the session list.
    pub invert_pr_label_color: bool,

    /// Show the program running in each session as a `(program)` suffix in
    /// the session list. Only rendered when sessions use more than one
    /// distinct program, so enabling this for a single-program setup is a
    /// no-op. Default true.
    #[serde(default = "default_true")]
    pub show_session_program: bool,

    /// Dim the right pane (preview/diff/shell) when the session list is focused
    pub dim_unfocused_preview: bool,

    /// How much to dim unfocused pane colors (0.0 = fully dimmed/black, 1.0 = no dimming).
    /// Uses a foreground color override instead of terminal DIM modifier for cross-terminal
    /// compatibility. Only takes effect when `dim_unfocused_preview` is true.
    pub dim_unfocused_opacity: f32,

    /// Leader key for quick-switch modal (e.g. " " for Space, "ctrl+k", "f1")
    pub leader_key: String,

    /// Debounce delay in ms when typing multi-digit session numbers
    pub session_number_debounce_ms: u64,

    /// Enable AI-generated branch summaries in the Info pane
    pub ai_summary_enabled: bool,

    /// Claude model to use for AI summaries (Haiku recommended for cost efficiency)
    pub ai_summary_model: String,

    /// Enable debug logging
    pub debug: bool,

    /// Log file path (if set, logs to file instead of stderr)
    pub log_file: Option<PathBuf>,

    /// Key bindings
    pub keybindings: KeyBindings,

    /// Theme color overrides
    #[serde(default)]
    pub theme: ThemeOverrides,

    /// Use rounded border corners (╭╮╰╯) instead of square (┌┐└┘). Default false.
    #[serde(default)]
    pub rounded_borders: bool,

    /// When opening the review view, precompute every file's render caches
    /// (word-diff segments + syntax highlighting) up front behind a loading
    /// spinner, instead of building each file's cache lazily on first
    /// navigation. Trades a one-off wait when opening for instant file
    /// switching afterwards. Default true.
    #[serde(default = "default_true")]
    pub precompute_review_caches: bool,

    /// Section definitions for grouping sessions in the TUI list.
    /// First-match-wins in declared order; unmatched sessions fall into a
    /// built-in "Other" catch-all.
    #[serde(default)]
    pub sections: Vec<crate::session::SectionConfig>,

    /// Advisory WIP limit for the implicit "In Progress" catch-all section.
    /// When set, the section header shows `count/n` and renders in a warning
    /// colour once `count >= n`. Purely informational — never blocks session
    /// creation.
    #[serde(default)]
    pub in_progress_limit: Option<u32>,

    /// Enable the persistent top-level "commander" Claude session — a session
    /// (not tied to any project) that coordinates other sessions via the CLI.
    /// Disabled by default; opt-in via config or the settings UI.
    #[serde(default)]
    pub commander_enabled: bool,

    /// Program (with flags) to launch for the commander session. When unset,
    /// falls back to `default_program`. Use this to pin a specific model,
    /// e.g. `claude --model opus-4-7`.
    #[serde(default)]
    pub commander_program: Option<String>,

    /// Working directory for the commander session. When unset, defaults to
    /// `<data dir>/commander`.
    #[serde(default)]
    pub commander_dir: Option<PathBuf>,

    /// Conversation mode (TTS): speak the commander's replies aloud via an
    /// OpenAI-compatible TTS engine. Disabled by default.
    #[serde(default)]
    pub conversation: ConversationConfig,

    /// Speech-to-text (voice input): transcribe the microphone via an
    /// OpenAI-compatible transcription engine and feed it to the conversation
    /// agent (Alt-V). Disabled by default.
    #[serde(default)]
    pub stt: SttConfig,

    /// Usage-telemetry settings (on by default, opt-out). See [`TelemetryConfig`].
    #[serde(default)]
    pub telemetry: TelemetryConfig,
}

/// Conversation-mode (text-to-speech) settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConversationConfig {
    /// Master switch for conversation mode: gates both the Alt-c overlay and
    /// spoken replies. When off, Alt-c does nothing. Off by default.
    pub enabled: bool,

    /// Display name / nickname for the assistant (the default voice is female,
    /// so this defaults to "Claudette"). Used as the chat label and told to the
    /// agent in its CLAUDE.md.
    pub name: String,

    /// Binary to run for the headless conversation session.
    pub command: String,

    /// `--permission-mode` for the conversation agent. `auto` lets it act
    /// without interactive approval prompts (which it can't answer headlessly).
    pub permission_mode: String,

    /// Base URL of the OpenAI-compatible TTS API (include the `/v1`).
    pub base_url: String,

    /// Model name sent in each request (engines serving one model ignore it).
    pub model: String,

    /// Voice name. `None` lets the server use its configured default.
    pub voice: Option<String>,

    /// Audio container requested per chunk. `wav` avoids a server-side
    /// transcode and client-side mp3 decode, for the lowest local latency.
    pub response_format: String,

    /// Playback speed (0.25–4.0).
    pub speed: f32,

    /// How much of each reply to speak.
    pub speak_scope: crate::conversation::SpeakScope,

    /// Playback volume (0.0–2.0; 1.0 = unchanged).
    pub volume: f32,
}

impl Default for ConversationConfig {
    fn default() -> Self {
        Self {
            // Master switch for the whole feature: gates both the Alt-c overlay
            // and spoken replies. Off by default — enable it in Settings.
            enabled: false,
            name: "Claudette".to_string(),
            command: "claude".to_string(),
            permission_mode: "auto".to_string(),
            base_url: "http://127.0.0.1:8002/v1".to_string(),
            model: "kokoro".to_string(),
            voice: None,
            response_format: "wav".to_string(),
            speed: 1.0,
            speak_scope: crate::conversation::SpeakScope::ProseOnly,
            volume: 1.0,
        }
    }
}

/// Speech-to-text (voice-input) settings.
///
/// Mirrors [`ConversationConfig`] for the transcription side: an
/// OpenAI-compatible `POST {base_url}/audio/transcriptions` endpoint. The
/// transcribed text is fed to the conversation session, so STT only does
/// anything useful with conversation mode running.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SttConfig {
    /// Master switch for voice input. When off, Alt-V does nothing. Off by
    /// default.
    pub enabled: bool,

    /// Base URL of the OpenAI-compatible transcription API (include the `/v1`).
    pub base_url: String,

    /// Model name sent with each request (engines serving one model ignore it).
    pub model: String,

    /// ISO-639-1 language hint. `None` lets the server auto-detect.
    pub language: Option<String>,

    /// Optional decoding prompt (domain vocabulary / spelling hints). `None`
    /// sends nothing.
    pub prompt: Option<String>,

    /// API key, sent as a `Bearer` header when set. `None` for local servers
    /// that don't authenticate.
    pub api_key: Option<String>,

    /// Pause other media players while recording voice input, resuming them once
    /// the assistant finishes its spoken reply. Best-effort via `playerctl`
    /// (Linux) / `osascript` (macOS); a silent no-op when neither is available.
    /// On by default.
    pub pause_media: bool,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            // Off by default — enable it in Settings ▸ Conversation.
            enabled: false,
            // Localhost placeholder, like the TTS default; override in config to
            // point at your transcription server.
            base_url: "http://127.0.0.1:8000/v1".to_string(),
            model: "Systran/faster-whisper-base".to_string(),
            language: None,
            prompt: None,
            api_key: None,
            pause_media: true,
        }
    }
}

/// Usage-telemetry settings. Telemetry is on by default in official builds and
/// opt-out: set `enabled = false` here or export `DO_NOT_TRACK` to disable it.
/// Only feature-usage and a coarse, non-sensitive environment/config snapshot
/// are sent — never typed text, prompts, session content, or paths. See the
/// `telemetry` module for the exact schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TelemetryConfig {
    /// Master switch. On by default; honoured alongside the `DO_NOT_TRACK` env
    /// var (either being set/false disables telemetry).
    pub enabled: bool,

    /// Override the ingest endpoint (for self-hosters). `None` uses the
    /// endpoint baked into the build.
    pub endpoint: Option<String>,

    /// Override the ingest credential — a pre-encoded HTTP Basic value,
    /// `base64("<email>:<token>")`. `None` uses the build-time baked credential
    /// (absent in third-party builds, which then send nothing).
    pub token: Option<String>,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            endpoint: None,
            token: None,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_program: "claude".to_string(),
            branch_prefix: String::new(),
            max_concurrent_tmux: 16,
            capture_cache_ttl_ms: 50,
            diff_cache_ttl_ms: 500,
            ui_refresh_fps: 30,
            worktrees_dir: None,
            tmux_tmpdir: None,
            per_repo_worktree_dirs: false,
            editor: None,
            editor_gui: None,
            shell_program: std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string()),
            pr_check_interval_secs: 120,
            project_pull_enabled: true,
            project_pull_interval_secs: 3600,
            pr_review_labels: default_pr_review_labels(),
            fetch_before_create: true,
            resume_session: true,
            nix_develop: true,
            state_sync_interval_ms: 2000,
            agent_state_poll_interval_ms: 3000,
            invert_pr_label_color: false,
            show_session_program: true,
            dim_unfocused_preview: true,
            dim_unfocused_opacity: 0.4,
            leader_key: " ".to_string(),
            session_number_debounce_ms: 250,
            ai_summary_enabled: true,
            ai_summary_model: "claude-haiku-4-5-20251001".to_string(),
            debug: false,
            log_file: None,
            keybindings: KeyBindings::default(),
            theme: ThemeOverrides::default(),
            rounded_borders: false,
            precompute_review_caches: true,
            sections: Vec::new(),
            in_progress_limit: None,
            commander_enabled: false,
            commander_program: None,
            commander_dir: None,
            conversation: ConversationConfig::default(),
            stt: SttConfig::default(),
            telemetry: TelemetryConfig::default(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_pr_review_labels() -> Vec<String> {
    vec![
        "dev-review-required".to_string(),
        "ready-for-test".to_string(),
        "trivial".to_string(),
    ]
}

impl Config {
    /// Load configuration from all sources
    pub fn load() -> Result<Self> {
        let config_path = Self::config_file_path()?;

        let config: Config = Figment::new()
            // Start with defaults
            .merge(Serialized::defaults(Config::default()))
            // Layer config file if it exists
            .merge(Toml::file(&config_path))
            .extract()
            .map_err(|e| ConfigError::LoadFailed(e.to_string()))?;

        Ok(config)
    }

    /// Get the configuration file path
    pub fn config_file_path() -> Result<PathBuf> {
        let dirs = Self::project_dirs()?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Get the data directory path
    pub fn data_dir() -> Result<PathBuf> {
        let dirs = Self::project_dirs()?;
        Ok(dirs.data_dir().to_path_buf())
    }

    /// Get the state file path
    pub fn state_file_path() -> Result<PathBuf> {
        Ok(Self::data_dir()?.join("state.json"))
    }

    /// Get the worktrees directory path
    pub fn worktrees_dir(&self) -> Result<PathBuf> {
        if let Some(ref dir) = self.worktrees_dir {
            Ok(dir.clone())
        } else {
            Ok(Self::data_dir()?.join("worktrees"))
        }
    }

    /// Working directory for the commander session (config override or the
    /// default `<data dir>/commander`).
    pub fn commander_dir(&self) -> Result<PathBuf> {
        if let Some(ref dir) = self.commander_dir {
            Ok(dir.clone())
        } else {
            Ok(Self::data_dir()?.join("commander"))
        }
    }

    /// Program (with flags) to launch for the commander session, falling back
    /// to `default_program` when `commander_program` is unset.
    pub fn commander_program(&self) -> String {
        self.commander_program
            .clone()
            .unwrap_or_else(|| self.default_program.clone())
    }

    /// Resolve the worktrees directory, nesting under repo name if configured.
    pub fn resolve_worktrees_dir(&self, repo_name: &str) -> Result<PathBuf> {
        let base = self.worktrees_dir()?;
        if self.per_repo_worktree_dirs {
            Ok(base.join(repo_name))
        } else {
            Ok(base)
        }
    }

    /// Ensure all required directories exist
    pub fn ensure_directories(&self) -> Result<()> {
        let dirs = Self::project_dirs()?;

        std::fs::create_dir_all(dirs.config_dir()).map_err(|_e| {
            Error::Config(ConfigError::DirectoryCreationFailed(
                dirs.config_dir().to_path_buf(),
            ))
        })?;

        std::fs::create_dir_all(dirs.data_dir()).map_err(|_e| {
            Error::Config(ConfigError::DirectoryCreationFailed(
                dirs.data_dir().to_path_buf(),
            ))
        })?;

        let worktrees_dir = self.worktrees_dir()?;
        std::fs::create_dir_all(&worktrees_dir)
            .map_err(|_| Error::Config(ConfigError::DirectoryCreationFailed(worktrees_dir)))?;

        // Seed a default config file if none exists so users can discover it
        let config_path = Self::config_file_path()?;
        if !config_path.exists() {
            let _ = Config::default().save();
        }

        Ok(())
    }

    /// Save current configuration to file
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_file_path()?;

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|_e| {
                Error::Config(ConfigError::DirectoryCreationFailed(parent.to_path_buf()))
            })?;
        }

        let toml =
            toml::to_string_pretty(self).map_err(|e| ConfigError::SaveFailed(e.to_string()))?;

        std::fs::write(&config_path, toml).map_err(|e| ConfigError::SaveFailed(e.to_string()))?;

        Ok(())
    }

    /// Resolve the editor command: config → $VISUAL → $EDITOR → None
    pub fn resolve_editor(&self) -> Option<String> {
        self.editor
            .clone()
            .or_else(|| std::env::var("VISUAL").ok())
            .or_else(|| std::env::var("EDITOR").ok())
    }

    /// Whether the resolved editor is a GUI application.
    /// Uses explicit `editor_gui` config if set, otherwise checks a known list.
    pub fn is_gui_editor(&self, editor: &str) -> bool {
        if let Some(gui) = self.editor_gui {
            return gui;
        }
        // Extract the basename for matching (handles paths like /usr/bin/code)
        let basename = std::path::Path::new(editor)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(editor);
        matches!(
            basename,
            "code"
                | "code-insiders"
                | "cursor"
                | "zed"
                | "zeditor"
                | "subl"
                | "sublime_text"
                | "idea"
                | "goland"
                | "rustrover"
                | "clion"
                | "pycharm"
                | "webstorm"
                | "phpstorm"
                | "atom"
                | "lapce"
                | "fleet"
                | "gedit"
                | "kate"
                | "mousepad"
                | "gvim"
                | "open"
                | "xdg-open"
        )
    }

    /// Parse the `leader_key` config string into a crossterm key code and modifiers.
    ///
    /// Supported formats:
    /// - `" "` → Space
    /// - `"ctrl+k"` → Ctrl+K
    /// - `"f1"` → F1
    /// - `"tab"` → Tab
    /// - Single character like `"x"` → Char('x')
    pub fn parse_leader_key(&self) -> (KeyCode, KeyModifiers) {
        parse_key_string(&self.leader_key)
    }

    fn project_dirs() -> Result<ProjectDirs> {
        ProjectDirs::from("com", "claude-commander", "claude-commander").ok_or_else(|| {
            Error::Config(ConfigError::LoadFailed(
                "Could not determine home directory".to_string(),
            ))
        })
    }
}

/// Parse a key string like `" "`, `"ctrl+k"`, `"f1"` into crossterm types.
fn parse_key_string(s: &str) -> (KeyCode, KeyModifiers) {
    let s = s.trim().to_lowercase();

    // Check for modifier prefix (ctrl+, alt+, shift+)
    if let Some(rest) = s.strip_prefix("ctrl+") {
        let code = parse_key_code(rest);
        return (code, KeyModifiers::CONTROL);
    }
    if let Some(rest) = s.strip_prefix("alt+") {
        let code = parse_key_code(rest);
        return (code, KeyModifiers::ALT);
    }

    (parse_key_code(&s), KeyModifiers::NONE)
}

fn parse_key_code(s: &str) -> KeyCode {
    match s {
        " " | "space" => KeyCode::Char(' '),
        "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "esc" | "escape" => KeyCode::Esc,
        "backspace" => KeyCode::Backspace,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "f1" => KeyCode::F(1),
        "f2" => KeyCode::F(2),
        "f3" => KeyCode::F(3),
        "f4" => KeyCode::F(4),
        "f5" => KeyCode::F(5),
        "f6" => KeyCode::F(6),
        "f7" => KeyCode::F(7),
        "f8" => KeyCode::F(8),
        "f9" => KeyCode::F(9),
        "f10" => KeyCode::F(10),
        "f11" => KeyCode::F(11),
        "f12" => KeyCode::F(12),
        s if s.len() == 1 => KeyCode::Char(s.chars().next().unwrap()),
        _ => KeyCode::Char(' '), // fallback to space
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_max_sessions_and_in_progress_limit_round_trip() {
        let toml_src = r#"
in_progress_limit = 4

[[sections]]
name = "Needs Review"
has_label = "ready-for-review"
max_sessions = 5
"#;
        let config: Config = toml::from_str(toml_src).expect("toml parse");
        assert_eq!(config.in_progress_limit, Some(4));
        assert_eq!(config.sections.len(), 1);
        assert_eq!(config.sections[0].max_sessions, Some(5));

        // Defaults are unset.
        let default = Config::default();
        assert_eq!(default.in_progress_limit, None);
    }

    #[test]
    fn test_sections_toml_deserialises() {
        let toml_src = r#"
[[sections]]
name = "Needs Review"
has_label = "ready-for-review"
is_draft = false

[[sections]]
name = "Drafts"
pr_state = "open"
is_draft = true

[[sections]]
name = "Blocked"
has_label = ["blocked", "waiting-on-author"]
"#;
        let config: Config = toml::from_str(toml_src).expect("toml parse");

        assert_eq!(config.sections.len(), 3);
        assert_eq!(config.sections[0].name, "Needs Review");
        assert_eq!(config.sections[1].name, "Drafts");
        assert_eq!(config.sections[1].is_draft, Some(true));
        assert_eq!(config.sections[2].name, "Blocked");
    }

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.default_program, "claude");
        assert_eq!(config.branch_prefix, "");
        assert_eq!(config.max_concurrent_tmux, 16);
        assert_eq!(config.capture_cache_ttl_ms, 50);
        assert_eq!(config.ui_refresh_fps, 30);
    }

    #[test]
    fn test_conversation_defaults() {
        let c = ConversationConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.base_url, "http://127.0.0.1:8002/v1");
        assert_eq!(c.model, "kokoro");
        assert_eq!(c.voice, None);
        assert_eq!(c.response_format, "wav");
        assert_eq!(c.speed, 1.0);
        assert_eq!(c.speak_scope, crate::conversation::SpeakScope::ProseOnly);
        assert_eq!(c.volume, 1.0);
    }

    #[test]
    fn test_empty_toml_yields_conversation_defaults() {
        let config: Config = toml::from_str("").expect("empty toml");
        assert!(!config.conversation.enabled);
        assert_eq!(config.conversation.base_url, "http://127.0.0.1:8002/v1");
    }

    #[test]
    fn test_stt_defaults() {
        let c = SttConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.base_url, "http://127.0.0.1:8000/v1");
        assert_eq!(c.model, "Systran/faster-whisper-base");
        assert_eq!(c.language, None);
        assert_eq!(c.prompt, None);
        assert_eq!(c.api_key, None);
    }

    #[test]
    fn test_empty_toml_yields_stt_defaults() {
        let config: Config = toml::from_str("").expect("empty toml");
        assert!(!config.stt.enabled);
        assert_eq!(config.stt.base_url, "http://127.0.0.1:8000/v1");
    }

    #[test]
    fn test_stt_toml_roundtrip() {
        let toml_src = r#"
[stt]
enabled = true
base_url = "http://192.168.1.10:8080/v1"
model = "large-v3-turbo"
language = "en"
"#;
        let config: Config = toml::from_str(toml_src).expect("toml parse");
        assert!(config.stt.enabled);
        assert_eq!(config.stt.base_url, "http://192.168.1.10:8080/v1");
        assert_eq!(config.stt.model, "large-v3-turbo");
        assert_eq!(config.stt.language.as_deref(), Some("en"));
        // Unspecified fields keep their defaults.
        assert_eq!(config.stt.prompt, None);
    }

    #[test]
    fn test_conversation_toml_roundtrip() {
        let toml_src = r#"
[conversation]
enabled = true
base_url = "http://host:9000/v1"
voice = "bm_fable"
speak_scope = "final_summary"
speed = 1.25
"#;
        let config: Config = toml::from_str(toml_src).expect("toml parse");
        assert!(config.conversation.enabled);
        assert_eq!(config.conversation.base_url, "http://host:9000/v1");
        assert_eq!(config.conversation.voice.as_deref(), Some("bm_fable"));
        assert_eq!(
            config.conversation.speak_scope,
            crate::conversation::SpeakScope::FinalSummary
        );
        assert_eq!(config.conversation.speed, 1.25);
        // Unspecified fields keep their defaults.
        assert_eq!(config.conversation.model, "kokoro");
    }

    #[test]
    fn test_config_serialization() {
        let config = Config::default();
        let toml = toml::to_string_pretty(&config).unwrap();
        assert!(toml.contains("default_program"));
        assert!(toml.contains("claude"));
    }

    #[test]
    fn test_resolve_editor_config_takes_precedence() {
        let config = Config {
            editor: Some("myeditor".to_string()),
            ..Config::default()
        };
        assert_eq!(config.resolve_editor(), Some("myeditor".to_string()));
    }

    #[test]
    fn test_resolve_editor_none_when_unset() {
        // This test assumes VISUAL and EDITOR env vars are not set.
        // If they are, the test still validates that config.editor=None
        // defers to env vars (which is correct behavior).
        let config = Config {
            editor: None,
            ..Config::default()
        };
        let result = config.resolve_editor();
        // If neither env var is set, result is None; otherwise it's the env var value.
        // Either way, config.editor being None means it wasn't used.
        assert!(
            result.is_none() || std::env::var("VISUAL").is_ok() || std::env::var("EDITOR").is_ok()
        );
    }

    #[test]
    fn test_is_gui_editor_explicit_true_overrides() {
        let config = Config {
            editor_gui: Some(true),
            ..Config::default()
        };
        // Even a terminal editor returns true when explicitly set
        assert!(config.is_gui_editor("vim"));
    }

    #[test]
    fn test_is_gui_editor_explicit_false_overrides() {
        let config = Config {
            editor_gui: Some(false),
            ..Config::default()
        };
        // Even a GUI editor returns false when explicitly set
        assert!(!config.is_gui_editor("code"));
    }

    #[test]
    fn test_is_gui_editor_known_gui_editors() {
        let config = Config::default();
        let gui_editors = [
            "code",
            "code-insiders",
            "cursor",
            "zed",
            "zeditor",
            "subl",
            "sublime_text",
            "idea",
            "goland",
            "rustrover",
            "clion",
            "pycharm",
            "webstorm",
            "phpstorm",
            "atom",
            "lapce",
            "fleet",
            "gedit",
            "kate",
            "mousepad",
            "gvim",
            "open",
            "xdg-open",
        ];
        for editor in gui_editors {
            assert!(
                config.is_gui_editor(editor),
                "{} should be detected as GUI",
                editor
            );
        }
    }

    #[test]
    fn test_is_gui_editor_terminal_editors_not_gui() {
        let config = Config::default();
        let terminal_editors = [
            "vim", "nvim", "nano", "emacs", "vi", "micro", "helix", "joe",
        ];
        for editor in terminal_editors {
            assert!(
                !config.is_gui_editor(editor),
                "{} should NOT be detected as GUI",
                editor
            );
        }
    }

    #[test]
    fn test_is_gui_editor_with_path_prefix() {
        let config = Config::default();
        assert!(config.is_gui_editor("/usr/bin/code"));
        assert!(!config.is_gui_editor("/usr/bin/nvim"));
    }

    #[test]
    fn test_default_config_additional_values() {
        let config = Config::default();
        assert_eq!(config.diff_cache_ttl_ms, 500);
        assert_eq!(config.pr_check_interval_secs, 120);
        assert!(config.fetch_before_create);
        assert!(config.resume_session);
        assert_eq!(config.state_sync_interval_ms, 2000);
        assert_eq!(config.agent_state_poll_interval_ms, 3000);
        assert!(config.ai_summary_enabled);
        assert_eq!(config.ai_summary_model, "claude-haiku-4-5-20251001");
        assert!(config.show_session_program);
        // Review cache precompute is on by default.
        assert!(config.precompute_review_caches);
    }

    #[test]
    fn test_session_list_flags_deserialise() {
        // Missing → default true.
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.show_session_program);

        // Explicit false survives round trip.
        let cfg: Config = toml::from_str(
            r#"
show_session_program = false
"#,
        )
        .unwrap();
        assert!(!cfg.show_session_program);
    }

    #[test]
    fn test_default_leader_key() {
        let config = Config::default();
        assert_eq!(config.leader_key, " ");
        let (code, mods) = config.parse_leader_key();
        assert_eq!(code, KeyCode::Char(' '));
        assert_eq!(mods, KeyModifiers::NONE);
    }

    #[test]
    fn test_default_session_number_debounce_ms() {
        let config = Config::default();
        assert_eq!(config.session_number_debounce_ms, 250);
    }

    #[test]
    fn test_resolve_worktrees_dir_flat_when_disabled() {
        let config = Config {
            worktrees_dir: Some(PathBuf::from("/tmp/worktrees")),
            per_repo_worktree_dirs: false,
            ..Config::default()
        };
        let result = config.resolve_worktrees_dir("genio").unwrap();
        assert_eq!(result, PathBuf::from("/tmp/worktrees"));
    }

    #[test]
    fn test_resolve_worktrees_dir_nested_when_enabled() {
        let config = Config {
            worktrees_dir: Some(PathBuf::from("/tmp/worktrees")),
            per_repo_worktree_dirs: true,
            ..Config::default()
        };
        let result = config.resolve_worktrees_dir("genio").unwrap();
        assert_eq!(result, PathBuf::from("/tmp/worktrees/genio"));
    }

    #[test]
    fn test_commander_disabled_by_default() {
        let config = Config::default();
        assert!(!config.commander_enabled);
        assert!(config.commander_program.is_none());
        assert!(config.commander_dir.is_none());
    }

    #[test]
    fn test_commander_program_falls_back_to_default_program() {
        let config = Config {
            default_program: "claude".to_string(),
            commander_program: None,
            ..Config::default()
        };
        assert_eq!(config.commander_program(), "claude");
    }

    #[test]
    fn test_commander_program_override_wins() {
        let config = Config {
            default_program: "claude".to_string(),
            commander_program: Some("claude --model opus-4-7".to_string()),
            ..Config::default()
        };
        assert_eq!(config.commander_program(), "claude --model opus-4-7");
    }

    #[test]
    fn test_commander_dir_override_wins() {
        let config = Config {
            commander_dir: Some(PathBuf::from("/tmp/commander")),
            ..Config::default()
        };
        assert_eq!(
            config.commander_dir().unwrap(),
            PathBuf::from("/tmp/commander")
        );
    }

    #[test]
    fn test_commander_enabled_deserialises() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(!cfg.commander_enabled);

        let cfg: Config = toml::from_str("commander_enabled = true\n").unwrap();
        assert!(cfg.commander_enabled);
    }

    #[test]
    fn test_parse_leader_key_variants() {
        let cases = vec![
            (" ", KeyCode::Char(' '), KeyModifiers::NONE),
            ("space", KeyCode::Char(' '), KeyModifiers::NONE),
            ("ctrl+k", KeyCode::Char('k'), KeyModifiers::CONTROL),
            ("ctrl+p", KeyCode::Char('p'), KeyModifiers::CONTROL),
            ("f1", KeyCode::F(1), KeyModifiers::NONE),
            ("f12", KeyCode::F(12), KeyModifiers::NONE),
            ("tab", KeyCode::Tab, KeyModifiers::NONE),
            ("x", KeyCode::Char('x'), KeyModifiers::NONE),
        ];
        for (input, expected_code, expected_mods) in cases {
            let (code, mods) = parse_key_string(input);
            assert_eq!(code, expected_code, "Failed for input: {}", input);
            assert_eq!(mods, expected_mods, "Failed for input: {}", input);
        }
    }
}
