//! User configuration settings
//!
//! Layered configuration: defaults → config file

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyModifiers};
use directories::ProjectDirs;
use figment::{
    Figment,
    providers::{Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};

use crate::config::keybindings::KeyBindings;
use crate::config::migrations;
use crate::config::theme::ThemeOverrides;
use crate::error::{ConfigError, Error, Result, SessionError};

/// A selectable agent harness in the new-session program picker: a display
/// `label` paired with the `command` to launch (program plus any flags).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramEntry {
    /// Friendly name shown in the picker (e.g. "Claude (Opus)").
    pub label: String,
    /// Command launched for the session (e.g. `claude --model opus`). Its first
    /// token determines the agent harness.
    pub command: String,
}

impl From<claude_commander_protocol::api::ProgramInfo> for ProgramEntry {
    fn from(p: claude_commander_protocol::api::ProgramInfo) -> Self {
        ProgramEntry {
            label: p.label,
            command: p.command,
        }
    }
}

impl From<&ProgramEntry> for claude_commander_protocol::api::ProgramInfo {
    fn from(p: &ProgramEntry) -> Self {
        claude_commander_protocol::api::ProgramInfo {
            label: p.label.clone(),
            command: p.command.clone(),
        }
    }
}

/// Connection details for one remote `claude-commander-server` the TUI drives
/// alongside the local backend. Serialised as a `[[remote_servers]]` TOML table:
///
/// ```toml
/// [[remote_servers]]
/// name = "buildbox"
/// url = "http://buildbox:7878"
/// token = "..."
/// ```
///
/// `Debug` is hand-written to redact `token`, so an accidental `{:?}` on the
/// enclosing [`Config`] (which is logged on some error paths) can never leak the
/// bearer secret. The binary maps this into the remote crate's
/// `RemoteServerSpec` (whose `SecretString` gives the same guarantee on the
/// wire-client side).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteServerConfig {
    /// Human-readable label shown as the server's tree header. Must be unique
    /// and non-empty across all configured servers.
    pub name: String,
    /// Base URL of the server (scheme + host + port, e.g. `http://buildbox:7878`).
    pub url: String,
    /// Bearer token, or `None` for an auth-disabled (loopback) server.
    #[serde(default)]
    pub token: Option<String>,
}

impl std::fmt::Debug for RemoteServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the token — redact its presence, not its value.
        f.debug_struct("RemoteServerConfig")
            .field("name", &self.name)
            .field("url", &self.url)
            .field(
                "token",
                &self.token.as_ref().map(|_| "<redacted>").unwrap_or("None"),
            )
            .finish()
    }
}

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Legacy default program setting. Config files are migrated to put this
    /// command first in `programs`; runtime code should use
    /// [`Config::default_session_program`] instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_program: Option<String>,

    /// Selectable agent harnesses for the new-session program picker. Each entry
    /// pairs a display `label` with the `command` launched (program plus any
    /// flags). The first entry is the default. When empty, the picker offers a
    /// single `claude` entry so existing empty configs keep working unchanged.
    #[serde(default)]
    pub programs: Vec<ProgramEntry>,

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

    /// Base directory for pasted-image temp files (remote image paste). `None`
    /// (normal use) means the OS temp dir, which is space-free on every platform
    /// and readable by the agent. Set only by hermetic tests to redirect writes
    /// (and the store's prune) into a `TempDir` instead of the real `/tmp`, per
    /// the repo's test-isolation rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paste_images_dir: Option<PathBuf>,

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

    /// Skip git-LFS smudging during `git worktree add` so session creation is
    /// fast on LFS repos (the checkout leaves cheap pointer files). The real
    /// LFS content is fetched afterwards with `git lfs pull` — asynchronously
    /// in the TUI (with a `⇣ LFS` indicator) or synchronously on the CLI.
    /// Default true.
    #[serde(default = "default_true")]
    pub skip_lfs_smudge: bool,

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

    /// Enable automatic hibernation of idle sessions: a background loop stops
    /// the tmux process (freeing ~400MB per idle `claude`) for sessions that
    /// have been idle past `hibernate_idle_timeout_secs`, keeping the worktree
    /// and metadata. The session transparently resumes on next attach.
    /// Disabled by default.
    #[serde(default)]
    pub hibernate_enabled: bool,

    /// Idle duration in seconds before an eligible session is hibernated. A
    /// session counts as idle only while its agent is Idle (not Working or
    /// WaitingForInput) and no tmux client is attached. Default 86400 (1 day).
    #[serde(default = "default_hibernate_idle_timeout_secs")]
    pub hibernate_idle_timeout_secs: u64,

    /// Interval in seconds between hibernation policy checks. Default 600
    /// (10 min). `0` disables the loop entirely (the loop is not spawned),
    /// matching the convention used by `agent_state_poll_interval_ms`.
    #[serde(default = "default_hibernate_check_interval_secs")]
    pub hibernate_check_interval_secs: u64,

    /// When true, render PR labels as colored text on the default background
    /// (the pre-pill behavior). When false (default), PR labels render as a
    /// pill — colored background block with contrasting text — so they stand
    /// out more in the session list.
    pub invert_pr_label_color: bool,

    /// Show the program running in each session as a `(program)` suffix in
    /// the session list. Only rendered when sessions use more than one
    /// distinct program, so enabling this for a single-program setup is a
    /// no-op. Default false. (Inherits the struct-level `#[serde(default)]`,
    /// so an omitted field resolves to `Config::default()`'s `false`.)
    pub show_session_program: bool,

    /// Whether to hide empty section headers in the session list.
    ///
    /// Enabled by default. When true, sections with no sessions (including
    /// "In Progress") are not rendered. This is a UI-only change; backend
    /// section assignment is unaffected.
    #[serde(default = "default_true")]
    pub hide_empty_sections: bool,

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
    /// When set, the section header shows `count/n`, rendering in the warning
    /// colour when `count == n` and the error colour when `count > n`. Purely
    /// informational — never blocks session creation.
    #[serde(default)]
    pub in_progress_limit: Option<u32>,

    /// Number of most-recently-attached sessions to surface in the "Recent"
    /// block at the top of the session list (across all servers). `0` hides
    /// the block entirely. Default 5.
    #[serde(default = "default_recent_sessions_limit")]
    pub recent_sessions_limit: u32,

    /// Enable the persistent top-level "commander" Claude session — a session
    /// (not tied to any project) that coordinates other sessions via the CLI.
    /// Disabled by default; opt-in via config or the settings UI.
    #[serde(default)]
    pub commander_enabled: bool,

    /// Program (with flags) to launch for the commander session. When unset,
    /// falls back to the first configured program. Use this to pin a specific model,
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

    /// Slack-integration settings (`[slack]` table). Disabled unless both tokens
    /// are set and an allowlist is configured — see [`SlackConfig::is_enabled`].
    #[serde(default)]
    pub slack: SlackConfig,

    /// Remote `claude-commander-server` instances the TUI drives alongside the
    /// local backend. Each entry becomes a per-server node in the session tree,
    /// in declared order after the always-present local backend. Empty by
    /// default. Validated on load (see [`Config::validate_remote_servers`]).
    #[serde(default)]
    pub remote_servers: Vec<RemoteServerConfig>,
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

    /// Microphone to capture from, as cpal's stable device id (the PipeWire
    /// `node.name`, e.g. `alsa_input.pci-0000_c1_00.6.analog-stereo`). `None`
    /// uses the system default input device. Set it via the picker in
    /// Settings ▸ Conversation rather than by hand. Ids — not friendly names —
    /// are stored because a mic and its speaker's loopback share a name; if the
    /// device is absent at record time, capture falls back to the default (with
    /// a warning) rather than failing.
    pub input_device: Option<String>,
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
            input_device: None,
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

/// Slack-integration settings (`[slack]` table).
///
/// Off unless [`SlackConfig::is_enabled`] — both tokens set and at least one
/// allowlisted user. The two tokens are secrets: they are stripped by
/// [`Config::with_secrets_redacted`] before the config is served over the wire,
/// and the hand-written `Debug` impl redacts them so an accidental `{:?}` on the
/// enclosing [`Config`] can't leak them.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SlackConfig {
    /// Socket Mode app-level token (`xapp-…`). Required to connect the bridge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_token: Option<String>,

    /// Bot user OAuth token (`xoxb-…`). Required for Web API calls
    /// (post message, add reaction, open DM, fetch thread).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_token: Option<String>,

    /// Slack user ids permitted to invoke the bridge (`@commander` mentions and
    /// DMs). Anyone not listed is ignored. The feature stays disabled while this
    /// is empty, so a misconfigured token pair can't accidentally accept
    /// everyone.
    #[serde(default)]
    pub allowed_user_ids: Vec<String>,

    /// Hard cap in seconds on a single headless invocation before it is killed
    /// (then retried once with a "be brief" nudge). Default 300 (5 min).
    #[serde(default = "default_slack_invocation_timeout_secs")]
    pub invocation_timeout_secs: u64,

    /// How long in seconds a headless process lingers after replying, ready for
    /// an instant follow-up in the same thread, before it is reaped. Default 300
    /// (5 min).
    #[serde(default = "default_slack_linger_secs")]
    pub linger_secs: u64,

    /// Keep one warm (non-resume) process ready so a brand-new thread doesn't
    /// pay cold-start latency. Default true.
    #[serde(default = "default_true")]
    pub warm_pool: bool,

    /// How often in seconds the warm process is respawned to avoid staleness.
    /// Default 3600 (1 hour).
    #[serde(default = "default_slack_warm_respawn_secs")]
    pub warm_respawn_secs: u64,
}

impl Default for SlackConfig {
    fn default() -> Self {
        Self {
            app_token: None,
            bot_token: None,
            allowed_user_ids: Vec::new(),
            invocation_timeout_secs: default_slack_invocation_timeout_secs(),
            linger_secs: default_slack_linger_secs(),
            warm_pool: true,
            warm_respawn_secs: default_slack_warm_respawn_secs(),
        }
    }
}

impl SlackConfig {
    /// Whether the Slack bridge should run: both tokens present (non-empty) and
    /// at least one allowlisted user. Independent of `commander_enabled` — the
    /// bridge is gated by this config alone.
    pub fn is_enabled(&self) -> bool {
        self.app_token.as_ref().is_some_and(|t| !t.is_empty())
            && self.bot_token.as_ref().is_some_and(|t| !t.is_empty())
            && !self.allowed_user_ids.is_empty()
    }
}

impl std::fmt::Debug for SlackConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the tokens — redact their presence, not their value.
        let redact = |t: &Option<String>| t.as_ref().map(|_| "<redacted>").unwrap_or("None");
        f.debug_struct("SlackConfig")
            .field("app_token", &redact(&self.app_token))
            .field("bot_token", &redact(&self.bot_token))
            .field("allowed_user_ids", &self.allowed_user_ids)
            .field("invocation_timeout_secs", &self.invocation_timeout_secs)
            .field("linger_secs", &self.linger_secs)
            .field("warm_pool", &self.warm_pool)
            .field("warm_respawn_secs", &self.warm_respawn_secs)
            .finish()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_program: None,
            programs: Vec::new(),
            branch_prefix: String::new(),
            max_concurrent_tmux: 16,
            capture_cache_ttl_ms: 50,
            diff_cache_ttl_ms: 500,
            ui_refresh_fps: 30,
            worktrees_dir: None,
            tmux_tmpdir: None,
            paste_images_dir: None,
            per_repo_worktree_dirs: false,
            editor: None,
            editor_gui: None,
            shell_program: std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string()),
            pr_check_interval_secs: 120,
            project_pull_enabled: true,
            project_pull_interval_secs: 3600,
            pr_review_labels: default_pr_review_labels(),
            fetch_before_create: true,
            skip_lfs_smudge: true,
            resume_session: true,
            nix_develop: true,
            state_sync_interval_ms: 2000,
            agent_state_poll_interval_ms: 3000,
            hibernate_enabled: false,
            hibernate_idle_timeout_secs: default_hibernate_idle_timeout_secs(),
            hibernate_check_interval_secs: default_hibernate_check_interval_secs(),
            invert_pr_label_color: false,
            show_session_program: false,
            hide_empty_sections: true,
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
            recent_sessions_limit: default_recent_sessions_limit(),
            commander_enabled: false,
            commander_program: None,
            commander_dir: None,
            conversation: ConversationConfig::default(),
            stt: SttConfig::default(),
            telemetry: TelemetryConfig::default(),
            slack: SlackConfig::default(),
            remote_servers: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_recent_sessions_limit() -> u32 {
    5
}

fn default_hibernate_idle_timeout_secs() -> u64 {
    86400
}

fn default_hibernate_check_interval_secs() -> u64 {
    600
}

fn default_slack_invocation_timeout_secs() -> u64 {
    300
}

fn default_slack_linger_secs() -> u64 {
    300
}

fn default_slack_warm_respawn_secs() -> u64 {
    3600
}

fn default_pr_review_labels() -> Vec<String> {
    vec![
        "dev-review-required".to_string(),
        "ready-for-test".to_string(),
        "trivial".to_string(),
    ]
}

impl Config {
    /// A copy with every credential field cleared, for serving over the wire
    /// (`GET /api/config`). The redacting `Debug` impls don't help serde, so
    /// without this a client holding one server's bearer token could harvest
    /// every OTHER server's token from `remote_servers` (plus the STT API key
    /// and telemetry credential) when the TUI and server share a config file.
    pub fn with_secrets_redacted(mut self) -> Self {
        for server in &mut self.remote_servers {
            server.token = None;
        }
        self.stt.api_key = None;
        self.telemetry.token = None;
        self.slack.app_token = None;
        self.slack.bot_token = None;
        self
    }

    /// Load configuration from all sources
    pub fn load() -> Result<Self> {
        let config_path = Self::config_file_path()?;

        Self::load_from_path(config_path)
    }

    /// Load configuration from a specific path using the standard migration and
    /// layered resolution pipeline.
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let config_path = path.as_ref();

        let mut figment = Figment::new()
            // Start with defaults
            .merge(Serialized::defaults(Config::default()));

        if let Some(toml) = migrations::migrate_config_file(config_path)? {
            figment = figment.merge(Toml::string(&toml));
        }

        let config: Config = figment
            .extract()
            .map_err(|e| ConfigError::LoadFailed(e.to_string()))?;

        config.validate_remote_servers()?;

        Ok(config)
    }

    /// Validate the configured [`remote_servers`](Self::remote_servers): names
    /// must be non-empty and unique, and each `url` must parse as an absolute
    /// URL with a host (so the backend factory can build a client). Returns the
    /// first problem as a [`ConfigError::InvalidValue`], so a bad entry is
    /// rejected at load with a clear message rather than surfacing later as a
    /// silently-degraded backend.
    pub fn validate_remote_servers(&self) -> Result<()> {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for server in &self.remote_servers {
            let name = server.name.trim();
            if name.is_empty() {
                return Err(ConfigError::InvalidValue {
                    key: "remote_servers.name".to_string(),
                    reason: "server name must not be empty".to_string(),
                }
                .into());
            }
            // "local" is the built-in local backend's descriptor name; the TUI's
            // selection persistence and per-backend lookups are name-keyed, so a
            // remote sharing it would collide with the local node.
            if name.eq_ignore_ascii_case("local") {
                return Err(ConfigError::InvalidValue {
                    key: "remote_servers.name".to_string(),
                    reason: "server name 'local' is reserved for the local machine".to_string(),
                }
                .into());
            }
            // Dedup case-insensitively: lookups (`find_remote_server`, the TUI's
            // name-keyed selection) match names with `eq_ignore_ascii_case`, so
            // `Box` and `box` would resolve ambiguously to whichever comes first.
            // Reject the collision at load rather than silently pick one.
            if !seen.insert(name.to_ascii_lowercase()) {
                return Err(ConfigError::InvalidValue {
                    key: "remote_servers.name".to_string(),
                    reason: format!("duplicate server name '{name}' (names are case-insensitive)"),
                }
                .into());
            }
            let parsed = url::Url::parse(&server.url).map_err(|e| ConfigError::InvalidValue {
                key: format!("remote_servers.{name}.url"),
                reason: format!("invalid url '{}': {e}", server.url),
            })?;
            if parsed.cannot_be_a_base() || parsed.host().is_none() {
                return Err(ConfigError::InvalidValue {
                    key: format!("remote_servers.{name}.url"),
                    reason: format!(
                        "url '{}' must include a host (e.g. http://host:port)",
                        server.url
                    ),
                }
                .into());
            }
            // Only http(s) is supported — the WS attach URL is derived by
            // rewriting the scheme (`http`→`ws`, `https`→`wss`), so any other
            // scheme (e.g. `ftp://`) would silently produce an unusable
            // endpoint. Reject it here with a clear message instead.
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(ConfigError::InvalidValue {
                    key: format!("remote_servers.{name}.url"),
                    reason: format!(
                        "url '{}' must use http or https (got '{}')",
                        server.url,
                        parsed.scheme()
                    ),
                }
                .into());
            }
        }
        Ok(())
    }

    /// Look up a configured remote server by name (case-insensitive, matching
    /// the uniqueness rule in [`validate_remote_servers`](Self::validate_remote_servers)).
    /// Returns a [`ConfigError::InvalidValue`] listing the available server
    /// names when no entry matches, so a CLI `--remote <name>` typo produces an
    /// actionable message rather than a silent local fallback.
    pub fn find_remote_server(&self, name: &str) -> Result<&RemoteServerConfig> {
        self.remote_servers
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                let available = if self.remote_servers.is_empty() {
                    "none configured".to_string()
                } else {
                    self.remote_servers
                        .iter()
                        .map(|s| s.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                ConfigError::InvalidValue {
                    key: "remote".to_string(),
                    reason: format!("no remote server named '{name}' (available: {available})"),
                }
                .into()
            })
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
    /// to the first configured program when `commander_program` is unset.
    pub fn commander_program(&self) -> String {
        self.commander_program
            .clone()
            .unwrap_or_else(|| self.default_session_program())
    }

    /// Command used when a session creation flow does not explicitly choose a
    /// program. The first configured program is the source of truth; unmigrated
    /// legacy configs fall back to `default_program`, then to built-in `claude`.
    pub fn default_session_program(&self) -> String {
        if let Some(entry) = self.programs.first() {
            entry.command.clone()
        } else if let Some(default_program) = &self.default_program {
            default_program.clone()
        } else {
            "claude".to_string()
        }
    }

    /// Gate what a Slack-originated `claude-commander new` may launch: both the
    /// `--program` and the `--mode` (permission mode) flags.
    ///
    /// The headless Slack commander is allowed to run `claude-commander new`, so
    /// prompt-injected thread text could otherwise steer it into an unattended
    /// worker that executes arbitrary code. Two flags are RCE-relevant:
    ///
    /// - `--program` chooses the binary executed in the created session's tmux
    ///   pane; `new --program "bash -lc '<payload>'"` would be direct code
    ///   execution outside the read-only deny fence.
    /// - `--mode` is folded into the launched command as `--permission-mode` (see
    ///   [`program_with_agent_flags`](crate::session::program_with_agent_flags)).
    ///   With the allowed program `claude` but `--mode bypassPermissions`, the
    ///   worker runs unattended with *no* permission prompt and an attacker's
    ///   `-i` prompt — arbitrary code execution even though the program is
    ///   benign.
    ///
    /// So when a `new` invocation is known to descend from the headless commander
    /// (detected at the CLI boundary via the inherited
    /// [`crate::commander::headless::PROGRAM_LOCK_ENV`] marker, which the agent can
    /// neither forge nor strip):
    ///
    /// - `program`: `None` uses the server's configured default (allowed);
    ///   `Some(p)` is allowed iff `p` exactly matches one of the configured
    ///   [`programs`](Self::programs) commands or the
    ///   [`default_session_program`](Self::default_session_program). Any other
    ///   string is rejected with [`SessionError::ProgramNotAllowed`].
    /// - `mode`: `None`, `default` and `plan` keep the permission prompt and are
    ///   allowed; any mode that disables the prompt (`bypassPermissions`,
    ///   `acceptEdits`) is rejected with [`SessionError::ModeNotAllowed`]. Benign
    ///   flags (`--effort`, `--model`, `-i`) are *not* gated: a default-mode
    ///   worker still prompts before executing tool calls, so they cannot on
    ///   their own achieve unattended execution.
    ///
    /// Pure over its inputs (no env access) so it is unit-testable; the env marker
    /// is read once at the CLI boundary and its result passed in as the gate.
    pub fn ensure_slack_create_allowed(
        &self,
        program: Option<&str>,
        mode: Option<&str>,
    ) -> std::result::Result<(), SessionError> {
        if let Some(program) = program {
            let allowed = program == self.default_session_program()
                || self.programs.iter().any(|p| p.command == program);
            if !allowed {
                return Err(SessionError::ProgramNotAllowed(program.to_string()));
            }
        }
        if let Some(mode) = mode
            && mode_disables_permission_gate(mode)
        {
            return Err(SessionError::ModeNotAllowed(mode.to_string()));
        }
        Ok(())
    }

    /// The non-empty list of harnesses offered in the new-session program
    /// picker. Returns the configured `programs` verbatim, or — when none are
    /// configured — a single built-in `claude` entry, so the picker always has
    /// at least one choice.
    pub fn program_choices(&self) -> Vec<ProgramEntry> {
        if self.programs.is_empty() {
            let default_program = self.default_session_program();
            vec![ProgramEntry {
                label: default_program.clone(),
                command: default_program,
            }]
        } else {
            self.programs.clone()
        }
    }

    /// Index into [`Self::program_choices`] of the entry to pre-select in the
    /// picker. The first configured program is the default.
    pub fn default_program_index(&self) -> usize {
        0
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

        super::write_private_file(&config_path, toml)
            .map_err(|e| ConfigError::SaveFailed(e.to_string()))?;

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

/// Whether a `--mode` (Claude `--permission-mode`) value disables the
/// interactive permission prompt, letting an unattended worker run tool calls
/// with no human gate. Claude Code's permission modes are `default`, `plan`,
/// `acceptEdits`, and `bypassPermissions`: `default`/`plan` keep the prompt,
/// while `bypassPermissions` skips every prompt and `acceptEdits` auto-approves
/// file edits/writes — both let a freshly-created, unattended Slack worker act
/// without anyone to approve, so both are forbidden.
///
/// Matched case-insensitively after trimming so a case/whitespace variant of a
/// dangerous mode cannot slip past (fails closed); an unrecognised mode is
/// treated as safe here — Claude itself rejects it, so it never launches a
/// bypassing worker.
fn mode_disables_permission_gate(mode: &str) -> bool {
    let m = mode.trim();
    m.eq_ignore_ascii_case("bypassPermissions") || m.eq_ignore_ascii_case("acceptEdits")
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
    #[test]
    fn with_secrets_redacted_clears_every_credential_field() {
        let c = Config {
            remote_servers: vec![RemoteServerConfig {
                name: "b".into(),
                url: "http://b:7878".into(),
                token: Some("server-secret".into()),
            }],
            stt: SttConfig {
                api_key: Some("stt-secret".into()),
                ..Default::default()
            },
            telemetry: TelemetryConfig {
                token: Some("telemetry-secret".into()),
                ..Default::default()
            },
            slack: SlackConfig {
                app_token: Some("slack-app-secret".into()),
                bot_token: Some("slack-bot-secret".into()),
                allowed_user_ids: vec!["U123".into()],
                ..Default::default()
            },
            ..Default::default()
        };

        let redacted = c.with_secrets_redacted();
        assert!(redacted.remote_servers[0].token.is_none());
        assert!(redacted.stt.api_key.is_none());
        assert!(redacted.telemetry.token.is_none());
        assert!(redacted.slack.app_token.is_none());
        assert!(redacted.slack.bot_token.is_none());
        // Non-secret fields survive.
        assert_eq!(redacted.remote_servers[0].url, "http://b:7878");
        assert_eq!(redacted.slack.allowed_user_ids, vec!["U123".to_string()]);
        let json = serde_json::to_string(&redacted).unwrap();
        for secret in [
            "server-secret",
            "stt-secret",
            "telemetry-secret",
            "slack-app-secret",
            "slack-bot-secret",
        ] {
            assert!(!json.contains(secret), "{secret} survived redaction");
        }
    }

    #[test]
    fn test_slack_defaults() {
        let c = SlackConfig::default();
        assert!(c.app_token.is_none());
        assert!(c.bot_token.is_none());
        assert!(c.allowed_user_ids.is_empty());
        assert_eq!(c.invocation_timeout_secs, 300);
        assert_eq!(c.linger_secs, 300);
        assert!(c.warm_pool);
        assert_eq!(c.warm_respawn_secs, 3600);
        // A default (empty) config never enables the bridge.
        assert!(!c.is_enabled());
        assert!(!Config::default().slack.is_enabled());
    }

    #[test]
    fn test_slack_is_enabled_requires_both_tokens_and_allowlist() {
        // Both tokens but no allowlisted user → disabled.
        let c = SlackConfig {
            app_token: Some("xapp-1".into()),
            bot_token: Some("xoxb-1".into()),
            allowed_user_ids: Vec::new(),
            ..Default::default()
        };
        assert!(!c.is_enabled());

        // Missing bot token → disabled.
        let c = SlackConfig {
            app_token: Some("xapp-1".into()),
            bot_token: None,
            allowed_user_ids: vec!["U1".into()],
            ..Default::default()
        };
        assert!(!c.is_enabled());

        // Empty-string token counts as absent.
        let c = SlackConfig {
            app_token: Some(String::new()),
            bot_token: Some("xoxb-1".into()),
            allowed_user_ids: vec!["U1".into()],
            ..Default::default()
        };
        assert!(!c.is_enabled());

        // Both tokens + an allowlisted user → enabled.
        let c = SlackConfig {
            app_token: Some("xapp-1".into()),
            bot_token: Some("xoxb-1".into()),
            allowed_user_ids: vec!["U1".into()],
            ..Default::default()
        };
        assert!(c.is_enabled());
    }

    #[test]
    fn test_slack_toml_roundtrip_and_tunable_defaults() {
        let toml_src = r#"
[slack]
app_token = "xapp-abc"
bot_token = "xoxb-def"
allowed_user_ids = ["U111", "U222"]
linger_secs = 120
"#;
        let config: Config = toml::from_str(toml_src).expect("toml parse");
        assert_eq!(config.slack.app_token.as_deref(), Some("xapp-abc"));
        assert_eq!(config.slack.bot_token.as_deref(), Some("xoxb-def"));
        assert_eq!(config.slack.allowed_user_ids, vec!["U111", "U222"]);
        // Explicit override survives.
        assert_eq!(config.slack.linger_secs, 120);
        // Unspecified tunables keep their defaults.
        assert_eq!(config.slack.invocation_timeout_secs, 300);
        assert!(config.slack.warm_pool);
        assert_eq!(config.slack.warm_respawn_secs, 3600);
        assert!(config.slack.is_enabled());
    }

    #[test]
    fn test_empty_toml_yields_slack_defaults() {
        let config: Config = toml::from_str("").expect("empty toml");
        assert!(!config.slack.is_enabled());
        assert_eq!(config.slack.invocation_timeout_secs, 300);
    }

    #[test]
    fn test_slack_debug_redacts_tokens() {
        let c = SlackConfig {
            app_token: Some("xapp-supersecret".into()),
            bot_token: Some("xoxb-supersecret".into()),
            allowed_user_ids: vec!["U1".into()],
            ..Default::default()
        };
        let dbg = format!("{c:?}");
        assert!(
            !dbg.contains("supersecret"),
            "token leaked via Debug: {dbg}"
        );
        assert!(dbg.contains("<redacted>"));
        // Non-secret fields are still shown.
        assert!(dbg.contains("U1"));
    }

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
        assert_eq!(config.default_program, None);
        assert_eq!(config.default_session_program(), "claude");
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
        assert_eq!(c.input_device, None);
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
input_device = "alsa_input.pci-0000_c1_00.6.analog-stereo"
"#;
        let config: Config = toml::from_str(toml_src).expect("toml parse");
        assert!(config.stt.enabled);
        assert_eq!(config.stt.base_url, "http://192.168.1.10:8080/v1");
        assert_eq!(config.stt.model, "large-v3-turbo");
        assert_eq!(config.stt.language.as_deref(), Some("en"));
        assert_eq!(
            config.stt.input_device.as_deref(),
            Some("alsa_input.pci-0000_c1_00.6.analog-stereo")
        );
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
        assert!(!toml.contains("default_program"));
    }

    #[test]
    fn test_load_from_path_migrates_default_program() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "default_program = \"codex\"\n").unwrap();

        let config = Config::load_from_path(&path).unwrap();

        assert_eq!(config.default_program, None);
        assert_eq!(config.default_session_program(), "codex");
        let migrated = std::fs::read_to_string(path).unwrap();
        assert!(!migrated.contains("default_program"));
        assert!(migrated.contains("command = \"codex\""));
    }

    #[test]
    fn test_load_from_path_migrates_default_program_with_empty_inline_programs() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "default_program = \"codex\"\nprograms = []\n").unwrap();

        let config = Config::load_from_path(&path).unwrap();

        assert_eq!(config.default_session_program(), "codex");
        let migrated = std::fs::read_to_string(path).unwrap();
        assert!(!migrated.contains("default_program"));
        assert!(migrated.contains("command = \"codex\""));
    }

    #[test]
    fn test_default_session_program_falls_back_to_legacy_default_program() {
        let config = Config {
            default_program: Some("codex".to_string()),
            programs: Vec::new(),
            ..Config::default()
        };

        assert_eq!(config.default_session_program(), "codex");
    }

    #[test]
    fn slack_program_lock_allows_configured_and_default_rejects_arbitrary() {
        let config = Config {
            programs: vec![
                ProgramEntry {
                    label: "Claude".to_string(),
                    command: "claude".to_string(),
                },
                ProgramEntry {
                    label: "Codex".to_string(),
                    command: "codex --model o3".to_string(),
                },
            ],
            ..Config::default()
        };

        // No override → the server's configured default is used → always allowed.
        assert!(config.ensure_slack_create_allowed(None, None).is_ok());

        // The default session program (first entry) is allowed.
        assert_eq!(config.default_session_program(), "claude");
        assert!(
            config
                .ensure_slack_create_allowed(Some("claude"), None)
                .is_ok()
        );

        // Any other configured program is allowed, matched exactly (with flags).
        assert!(
            config
                .ensure_slack_create_allowed(Some("codex --model o3"), None)
                .is_ok()
        );

        // An arbitrary injected payload is rejected.
        let err = config
            .ensure_slack_create_allowed(Some("bash -lc 'id'"), None)
            .unwrap_err();
        assert!(matches!(err, SessionError::ProgramNotAllowed(p) if p == "bash -lc 'id'"));

        // A near-miss (extra flag on a configured command) is not an exact match.
        assert!(
            config
                .ensure_slack_create_allowed(Some("claude --dangerously-skip-permissions"), None)
                .is_err()
        );
    }

    #[test]
    fn slack_program_lock_allows_builtin_default_when_no_programs_configured() {
        // Empty `programs` → default_session_program() is the built-in `claude`.
        let config = Config {
            programs: Vec::new(),
            ..Config::default()
        };
        assert!(config.ensure_slack_create_allowed(None, None).is_ok());
        assert!(
            config
                .ensure_slack_create_allowed(Some("claude"), None)
                .is_ok()
        );
        assert!(
            config
                .ensure_slack_create_allowed(Some("codex"), None)
                .is_err()
        );
    }

    #[test]
    fn slack_create_lock_rejects_permission_bypassing_modes() {
        let config = Config {
            programs: vec![ProgramEntry {
                label: "Claude".to_string(),
                command: "claude".to_string(),
            }],
            ..Config::default()
        };

        // Safe modes (and no mode) keep the interactive permission prompt, so an
        // allowed program plus these is permitted.
        assert!(
            config
                .ensure_slack_create_allowed(Some("claude"), None)
                .is_ok()
        );
        assert!(
            config
                .ensure_slack_create_allowed(Some("claude"), Some("default"))
                .is_ok()
        );
        assert!(
            config
                .ensure_slack_create_allowed(Some("claude"), Some("plan"))
                .is_ok()
        );

        // Permission-bypassing modes are rejected even with an allowed program.
        let err = config
            .ensure_slack_create_allowed(Some("claude"), Some("bypassPermissions"))
            .unwrap_err();
        assert!(matches!(err, SessionError::ModeNotAllowed(m) if m == "bypassPermissions"));
        assert!(matches!(
            config
                .ensure_slack_create_allowed(Some("claude"), Some("acceptEdits"))
                .unwrap_err(),
            SessionError::ModeNotAllowed(_)
        ));

        // Case/whitespace variants of a bypassing mode still fail closed.
        assert!(
            config
                .ensure_slack_create_allowed(Some("claude"), Some(" BYPASSPERMISSIONS "))
                .is_err()
        );
        assert!(
            config
                .ensure_slack_create_allowed(Some("claude"), Some("acceptedits"))
                .is_err()
        );

        // The mode gate applies even when no program override is given (the
        // configured default program is used, but the mode still disables the
        // prompt).
        assert!(
            config
                .ensure_slack_create_allowed(None, Some("bypassPermissions"))
                .is_err()
        );

        // The program gate still fires first for an injected program.
        assert!(matches!(
            config
                .ensure_slack_create_allowed(Some("bash -lc id"), Some("default"))
                .unwrap_err(),
            SessionError::ProgramNotAllowed(_)
        ));
    }

    #[test]
    fn mode_disables_permission_gate_classifies_modes() {
        assert!(mode_disables_permission_gate("bypassPermissions"));
        assert!(mode_disables_permission_gate("acceptEdits"));
        assert!(mode_disables_permission_gate("  bypassPermissions  "));
        assert!(mode_disables_permission_gate("BYPASSPERMISSIONS"));
        assert!(!mode_disables_permission_gate("default"));
        assert!(!mode_disables_permission_gate("plan"));
        assert!(!mode_disables_permission_gate(""));
    }

    #[test]
    fn test_program_choices_synthesises_legacy_default_program_when_unmigrated() {
        let config = Config {
            default_program: Some("codex".to_string()),
            programs: Vec::new(),
            ..Config::default()
        };

        let choices = config.program_choices();
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].label, "codex");
        assert_eq!(choices[0].command, "codex");
    }

    #[cfg(unix)]
    #[test]
    fn test_load_from_path_honours_legacy_default_when_migration_write_fails() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "default_program = \"codex\"\n").unwrap();
        if std::fs::metadata(&path).unwrap().uid() == 0 {
            return;
        }

        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o444);
        std::fs::set_permissions(&path, permissions).unwrap();

        let config = Config::load_from_path(&path).unwrap();

        assert_eq!(config.default_session_program(), "codex");
    }

    #[cfg(unix)]
    #[test]
    fn test_load_from_path_uses_migrated_content_when_migration_write_fails() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"default_program = "codex"

[[programs]]
label = "Claude"
command = "claude"

[[programs]]
label = "Codex"
command = "codex"
"#,
        )
        .unwrap();
        if std::fs::metadata(&path).unwrap().uid() == 0 {
            return;
        }

        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o444);
        std::fs::set_permissions(&path, permissions).unwrap();

        let config = Config::load_from_path(&path).unwrap();

        assert_eq!(config.default_session_program(), "codex");
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
        assert!(config.skip_lfs_smudge);
        assert!(config.resume_session);
        assert_eq!(config.state_sync_interval_ms, 2000);
        assert_eq!(config.agent_state_poll_interval_ms, 3000);
        assert!(config.ai_summary_enabled);
        assert_eq!(config.ai_summary_model, "claude-haiku-4-5-20251001");
        assert!(!config.show_session_program);
        // Hide empty sections is on by default.
        assert!(config.hide_empty_sections);
        // Review cache precompute is on by default.
        assert!(config.precompute_review_caches);
    }

    #[test]
    fn test_session_list_flags_deserialise() {
        // Missing → default false.
        let cfg: Config = toml::from_str("").unwrap();
        assert!(!cfg.show_session_program);

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
    fn test_skip_lfs_smudge_deserialise() {
        // Missing → default true.
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.skip_lfs_smudge);

        // Explicit false survives round trip.
        let cfg: Config = toml::from_str("skip_lfs_smudge = false\n").unwrap();
        assert!(!cfg.skip_lfs_smudge);
    }

    #[test]
    fn test_hide_empty_sections_deserialise() {
        // Missing → default true.
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.hide_empty_sections);

        // Explicit false survives round trip.
        let cfg: Config = toml::from_str("hide_empty_sections = false\n").unwrap();
        assert!(!cfg.hide_empty_sections);
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
    fn test_hibernation_defaults() {
        let config = Config::default();
        assert!(!config.hibernate_enabled);
        assert_eq!(config.hibernate_idle_timeout_secs, 86400);
        assert_eq!(config.hibernate_check_interval_secs, 600);
    }

    #[test]
    fn test_hibernation_fields_default_when_absent_from_toml() {
        // A config file written before this feature omits the fields; they must
        // fall back to the defaults rather than failing to parse.
        let config: Config = toml::from_str("default_program = \"claude\"").unwrap();
        assert!(!config.hibernate_enabled);
        assert_eq!(config.hibernate_idle_timeout_secs, 86400);
        assert_eq!(config.hibernate_check_interval_secs, 600);
    }

    #[test]
    fn test_commander_program_falls_back_to_first_program() {
        let config = Config {
            programs: vec![ProgramEntry {
                label: "Codex".to_string(),
                command: "codex".to_string(),
            }],
            commander_program: None,
            ..Config::default()
        };
        assert_eq!(config.commander_program(), "codex");
    }

    #[test]
    fn test_commander_program_override_wins() {
        let config = Config {
            commander_program: Some("claude --model opus-4-7".to_string()),
            ..Config::default()
        };
        assert_eq!(config.commander_program(), "claude --model opus-4-7");
    }

    #[test]
    fn test_program_choices_synthesises_claude_when_empty() {
        let config = Config::default();
        let choices = config.program_choices();
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].label, "claude");
        assert_eq!(choices[0].command, "claude");
        // Synthesised single entry is the default selection.
        assert_eq!(config.default_program_index(), 0);
    }

    #[test]
    fn test_program_choices_returns_configured_list() {
        let config = Config {
            programs: vec![
                ProgramEntry {
                    label: "Claude".to_string(),
                    command: "claude".to_string(),
                },
                ProgramEntry {
                    label: "Codex".to_string(),
                    command: "codex".to_string(),
                },
            ],
            ..Config::default()
        };
        let choices = config.program_choices();
        assert_eq!(choices.len(), 2);
        assert_eq!(config.default_program_index(), 0);
        assert_eq!(config.default_session_program(), "claude");
    }

    #[test]
    fn test_default_program_index_is_first() {
        let config = Config {
            programs: vec![
                ProgramEntry {
                    label: "Claude".to_string(),
                    command: "claude".to_string(),
                },
                ProgramEntry {
                    label: "Codex".to_string(),
                    command: "codex".to_string(),
                },
            ],
            ..Config::default()
        };
        assert_eq!(config.default_program_index(), 0);
    }

    #[test]
    fn test_program_entry_program_info_round_trip() {
        let entry = ProgramEntry {
            label: "Claude (Opus)".to_string(),
            command: "claude --model opus".to_string(),
        };
        let info: claude_commander_protocol::api::ProgramInfo = (&entry).into();
        assert_eq!(info.label, "Claude (Opus)");
        assert_eq!(info.command, "claude --model opus");
        let back: ProgramEntry = info.into();
        assert_eq!(back, entry);
    }

    #[test]
    fn test_programs_deserialise_from_toml() {
        let toml = "\
[[programs]]
label = \"Claude\"
command = \"claude\"

[[programs]]
label = \"Codex\"
command = \"codex -m gpt-5\"
";
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.programs.len(), 2);
        assert_eq!(cfg.programs[1].command, "codex -m gpt-5");
        // Empty config still deserialises to an empty list (back-compat).
        let empty: Config = toml::from_str("").unwrap();
        assert!(empty.programs.is_empty());
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
    fn test_remote_servers_default_empty() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.remote_servers.is_empty());
        assert!(cfg.validate_remote_servers().is_ok());
    }

    #[test]
    fn test_remote_servers_toml_roundtrip() {
        let toml_src = r#"
[[remote_servers]]
name = "buildbox"
url = "http://buildbox:7878"
token = "abc123"

[[remote_servers]]
name = "loopback"
url = "http://127.0.0.1:7878"
"#;
        let cfg: Config = toml::from_str(toml_src).expect("toml parse");
        assert_eq!(cfg.remote_servers.len(), 2);
        assert_eq!(cfg.remote_servers[0].name, "buildbox");
        assert_eq!(cfg.remote_servers[0].url, "http://buildbox:7878");
        assert_eq!(cfg.remote_servers[0].token.as_deref(), Some("abc123"));
        // Token is optional.
        assert_eq!(cfg.remote_servers[1].token, None);
        assert!(cfg.validate_remote_servers().is_ok());
    }

    #[test]
    fn test_remote_server_debug_redacts_token() {
        let server = RemoteServerConfig {
            name: "box".to_string(),
            url: "http://box:7878".to_string(),
            token: Some("SUPERSECRET".to_string()),
        };
        let dbg = format!("{server:?}");
        assert!(!dbg.contains("SUPERSECRET"), "token leaked in Debug: {dbg}");
        assert!(dbg.contains("box"));
        assert!(dbg.contains("<redacted>"));

        // A whole Config carrying the server must not leak it either.
        let cfg = Config {
            remote_servers: vec![server],
            ..Config::default()
        };
        assert!(!format!("{cfg:?}").contains("SUPERSECRET"));
    }

    #[test]
    fn test_validate_remote_servers_rejects_empty_name() {
        let cfg = Config {
            remote_servers: vec![RemoteServerConfig {
                name: "  ".to_string(),
                url: "http://box:7878".to_string(),
                token: None,
            }],
            ..Config::default()
        };
        let err = cfg.validate_remote_servers().unwrap_err();
        assert!(
            err.to_string().contains("must not be empty"),
            "empty name must be rejected with an empty-name reason: {err}"
        );
    }

    #[test]
    fn test_validate_remote_servers_rejects_non_http_scheme() {
        let cfg = Config {
            remote_servers: vec![RemoteServerConfig {
                name: "box".to_string(),
                url: "ftp://box".to_string(),
                token: None,
            }],
            ..Config::default()
        };
        let err = cfg.validate_remote_servers().unwrap_err();
        assert!(
            err.to_string().contains("http or https"),
            "non-http(s) scheme must be rejected at load: {err}"
        );
    }

    #[test]
    fn test_validate_remote_servers_rejects_duplicate_names() {
        let cfg = Config {
            remote_servers: vec![
                RemoteServerConfig {
                    name: "box".to_string(),
                    url: "http://a:7878".to_string(),
                    token: None,
                },
                RemoteServerConfig {
                    name: "box".to_string(),
                    url: "http://b:7878".to_string(),
                    token: None,
                },
            ],
            ..Config::default()
        };
        let err = cfg.validate_remote_servers().unwrap_err();
        assert!(err.to_string().contains("duplicate"), "{err}");
    }

    #[test]
    fn test_validate_remote_servers_rejects_case_insensitive_duplicate_names() {
        // `find_remote_server` matches case-insensitively, so `Box` and `box`
        // would resolve ambiguously — validation must reject the pair at load.
        let cfg = Config {
            remote_servers: vec![
                RemoteServerConfig {
                    name: "Box".to_string(),
                    url: "http://a:7878".to_string(),
                    token: None,
                },
                RemoteServerConfig {
                    name: "box".to_string(),
                    url: "http://b:7878".to_string(),
                    token: None,
                },
            ],
            ..Config::default()
        };
        let err = cfg.validate_remote_servers().unwrap_err();
        assert!(
            err.to_string().contains("duplicate"),
            "case-only-differing names must be rejected: {err}"
        );
    }

    #[test]
    fn test_validate_remote_servers_rejects_reserved_local_name() {
        // "local" is the local backend's own descriptor name; a remote server
        // sharing it would collide with the name-keyed selection lookups.
        for name in ["local", "Local", "LOCAL"] {
            let cfg = Config {
                remote_servers: vec![RemoteServerConfig {
                    name: name.to_string(),
                    url: "http://a:7878".to_string(),
                    token: None,
                }],
                ..Config::default()
            };
            let err = cfg.validate_remote_servers().unwrap_err();
            assert!(
                err.to_string().contains("reserved"),
                "name '{name}' should be rejected as reserved: {err}"
            );
        }
    }

    #[test]
    fn test_validate_remote_servers_rejects_unparseable_url() {
        let cfg = Config {
            remote_servers: vec![RemoteServerConfig {
                name: "box".to_string(),
                url: "not a url".to_string(),
                token: None,
            }],
            ..Config::default()
        };
        let err = cfg.validate_remote_servers().unwrap_err();
        assert!(
            err.to_string().contains("invalid url"),
            "an unparseable url must be rejected with a parse-failure reason: {err}"
        );
    }

    #[test]
    fn test_find_remote_server_matches_case_insensitively() {
        let cfg = Config {
            remote_servers: vec![RemoteServerConfig {
                name: "Box".to_string(),
                url: "http://box:7878".to_string(),
                token: None,
            }],
            ..Config::default()
        };
        let found = cfg.find_remote_server("box").unwrap();
        assert_eq!(found.url, "http://box:7878");
    }

    #[test]
    fn test_find_remote_server_unknown_lists_available() {
        let cfg = Config {
            remote_servers: vec![RemoteServerConfig {
                name: "box".to_string(),
                url: "http://box:7878".to_string(),
                token: None,
            }],
            ..Config::default()
        };
        let err = cfg.find_remote_server("nope").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("no remote server named 'nope'") && msg.contains("box"),
            "unknown remote must name the miss and list configured servers: {err}"
        );
    }

    #[test]
    fn test_find_remote_server_none_configured() {
        let cfg = Config::default();
        let err = cfg.find_remote_server("box").unwrap_err();
        assert!(
            err.to_string().contains("none configured"),
            "with no servers the error must say none are configured: {err}"
        );
    }

    #[test]
    fn test_validate_remote_servers_rejects_hostless_url() {
        // A scheme with no host (e.g. a bare path) must be rejected.
        let cfg = Config {
            remote_servers: vec![RemoteServerConfig {
                name: "box".to_string(),
                url: "file:///etc/passwd".to_string(),
                token: None,
            }],
            ..Config::default()
        };
        let err = cfg.validate_remote_servers().unwrap_err();
        assert!(
            err.to_string().contains("must include a host"),
            "a hostless url must be rejected with a missing-host reason: {err}"
        );
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
