//! User configuration settings
//!
//! Layered configuration: environment variables → config file → CLI args

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers};
use directories::ProjectDirs;
use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};

use crate::config::keybindings::{BindableAction, KeyBinding, KeyBindings};
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

    /// Shell program for shell sessions
    pub shell_program: String,

    /// Interval in seconds between GitHub PR checks (0 = disabled)
    pub pr_check_interval_secs: u64,

    /// Editor/IDE command for opening sessions (e.g. "code", "zed", "nvim")
    pub editor: Option<String>,

    /// Whether the editor is a GUI application (true) or terminal-based (false).
    /// If unset, auto-detected from a known list of GUI editors.
    pub editor_gui: Option<bool>,

    /// When true, binds Ctrl+<editor_key> as an additional hotkey for opening
    /// the editor. Unlike the plain editor key, the Ctrl variant is also
    /// recognised from inside an attached tmux session — press it to detach
    /// and launch the editor on the current session's worktree.
    pub editor_ctrl_hotkey_for_tmux_session: bool,

    /// Fetch the latest changes from origin before creating a new session
    #[serde(alias = "pull_before_create")]
    pub fetch_before_create: bool,

    /// Interval in milliseconds for checking state file changes from other instances (0 = disabled)
    pub state_sync_interval_ms: u64,

    /// Interval in milliseconds for polling agent state (Working/Idle/Waiting) (0 = disabled)
    pub agent_state_poll_interval_ms: u64,

    /// Show status indicator circles (●/◐/○) in the session list
    pub show_status_indicator: bool,

    /// Dim the right pane (preview/diff/shell) when the session list is focused
    pub dim_unfocused_preview: bool,

    /// How much to dim unfocused pane colors (0.0 = fully dimmed/black, 1.0 = no dimming).
    /// Uses a foreground color override instead of terminal DIM modifier for cross-terminal
    /// compatibility. Only takes effect when `dim_unfocused_preview` is true.
    pub dim_unfocused_opacity: f32,

    /// Leader key for quick-switch modal (e.g. " " for Space, "ctrl+k", "f1")
    pub leader_key: String,

    /// Show sequential numbers next to sessions for quick-jump hotkeys
    pub show_session_numbers: bool,

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
            editor: None,
            editor_gui: None,
            editor_ctrl_hotkey_for_tmux_session: false,
            shell_program: std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string()),
            pr_check_interval_secs: 600,
            fetch_before_create: true,
            state_sync_interval_ms: 2000,
            agent_state_poll_interval_ms: 3000,
            show_status_indicator: true,
            dim_unfocused_preview: true,
            dim_unfocused_opacity: 0.4,
            leader_key: " ".to_string(),
            show_session_numbers: false,
            session_number_debounce_ms: 250,
            ai_summary_enabled: true,
            ai_summary_model: "claude-haiku-4-5-20251001".to_string(),
            debug: false,
            log_file: None,
            keybindings: KeyBindings::default(),
            theme: ThemeOverrides::default(),
        }
    }
}

impl Config {
    /// Load configuration from all sources
    pub fn load() -> Result<Self> {
        let config_path = Self::config_file_path()?;

        let mut config: Config = Figment::new()
            // Start with defaults
            .merge(Serialized::defaults(Config::default()))
            // Layer config file if it exists
            .merge(Toml::file(&config_path))
            // Layer environment variables (CC_DEFAULT_PROGRAM, etc.)
            .merge(Env::prefixed("CC_").split("_"))
            .extract()
            .map_err(|e| ConfigError::LoadFailed(e.to_string()))?;

        config.apply_derived_keybindings();

        Ok(config)
    }

    /// Apply keybindings derived from other config options, such as adding
    /// `Ctrl-<editor_key>` when `editor_ctrl_hotkey_for_tmux_session` is set.
    ///
    /// Idempotent — safe to call multiple times.
    pub fn apply_derived_keybindings(&mut self) {
        if self.editor_ctrl_hotkey_for_tmux_session
            && let Some(c) = self.editor_ctrl_hotkey_char()
        {
            self.keybindings.add_binding(
                BindableAction::OpenInEditor,
                KeyBinding::new(KeyCode::Char(c), KeyModifiers::CONTROL),
            );
        }
    }

    /// The lowercase ASCII letter used as the editor hotkey — derived from
    /// the first plain-char binding of `OpenInEditor`.
    ///
    /// Only ASCII alphabetic characters are supported, because the Ctrl-byte
    /// mapping is only well-defined for those.
    pub fn editor_ctrl_hotkey_char(&self) -> Option<char> {
        let c = self
            .keybindings
            .primary_char_binding(BindableAction::OpenInEditor)?;
        if c.is_ascii_alphabetic() {
            Some(c.to_ascii_lowercase())
        } else {
            None
        }
    }

    /// The raw byte produced by Ctrl+<editor_char> inside a terminal — used
    /// to intercept the combination when forwarding keystrokes into an
    /// attached tmux PTY. Returns None if the feature is disabled or the
    /// editor binding is not a plain ASCII letter.
    pub fn editor_ctrl_hotkey_byte(&self) -> Option<u8> {
        if !self.editor_ctrl_hotkey_for_tmux_session {
            return None;
        }
        let c = self.editor_ctrl_hotkey_char()?;
        // ASCII convention: Ctrl+<letter> = letter_upper - 0x40 (so 'e' -> 0x05)
        Some((c.to_ascii_uppercase() as u8) - 0x40)
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
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.default_program, "claude");
        assert_eq!(config.branch_prefix, "");
        assert_eq!(config.max_concurrent_tmux, 16);
        assert_eq!(config.capture_cache_ttl_ms, 50);
        assert_eq!(config.ui_refresh_fps, 30);
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
        assert_eq!(config.pr_check_interval_secs, 600);
        assert!(config.fetch_before_create);
        assert_eq!(config.state_sync_interval_ms, 2000);
        assert_eq!(config.agent_state_poll_interval_ms, 3000);
        assert!(config.show_status_indicator);
        assert!(config.ai_summary_enabled);
        assert_eq!(config.ai_summary_model, "claude-haiku-4-5-20251001");
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
    fn test_default_session_numbers_config() {
        let config = Config::default();
        assert!(!config.show_session_numbers);
        assert_eq!(config.session_number_debounce_ms, 250);
    }

    #[test]
    fn test_editor_ctrl_hotkey_default_off() {
        let config = Config::default();
        assert!(!config.editor_ctrl_hotkey_for_tmux_session);
        assert_eq!(config.editor_ctrl_hotkey_byte(), None);
    }

    #[test]
    fn test_editor_ctrl_hotkey_when_enabled_derives_byte_from_default_e() {
        let mut config = Config {
            editor_ctrl_hotkey_for_tmux_session: true,
            ..Config::default()
        };
        // Default binding for OpenInEditor is 'e'
        assert_eq!(config.editor_ctrl_hotkey_char(), Some('e'));
        // Ctrl+E = 0x05 per standard ASCII control-byte convention
        assert_eq!(config.editor_ctrl_hotkey_byte(), Some(0x05));

        // After applying derived keybindings the KeyBindings table should
        // resolve Ctrl+E to OpenInEditor
        config.apply_derived_keybindings();
        let ctrl_e = crossterm::event::KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert_eq!(
            config.keybindings.resolve(&ctrl_e),
            Some(BindableAction::OpenInEditor)
        );
        // The original 'e' binding is preserved.
        let plain_e = crossterm::event::KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE);
        assert_eq!(
            config.keybindings.resolve(&plain_e),
            Some(BindableAction::OpenInEditor)
        );
    }

    #[test]
    fn test_apply_derived_keybindings_is_idempotent() {
        let mut config = Config {
            editor_ctrl_hotkey_for_tmux_session: true,
            ..Config::default()
        };
        config.apply_derived_keybindings();
        let count_after_first = config
            .keybindings
            .keys_for(BindableAction::OpenInEditor)
            .len();
        config.apply_derived_keybindings();
        let count_after_second = config
            .keybindings
            .keys_for(BindableAction::OpenInEditor)
            .len();
        assert_eq!(count_after_first, count_after_second);
    }

    #[test]
    fn test_editor_ctrl_hotkey_disabled_gives_no_byte_even_with_letter_binding() {
        let config = Config {
            editor_ctrl_hotkey_for_tmux_session: false,
            ..Config::default()
        };
        // Editor character is still 'e' but the feature is off.
        assert_eq!(config.editor_ctrl_hotkey_char(), Some('e'));
        assert_eq!(config.editor_ctrl_hotkey_byte(), None);
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
