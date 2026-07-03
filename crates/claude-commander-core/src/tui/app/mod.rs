//! Main TUI application
//!
//! Event-driven application that coordinates:
//! - Terminal rendering with ratatui
//! - User input handling
//! - Background state updates

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        KeyboardEnhancementFlags, MouseButton, MouseEventKind, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
};
use tracing::{debug, error, info, warn};
use tui_input::Input;

use super::event::{AppEvent, EventLoop, InputEvent, StateUpdate, UserCommand};
use super::path_completer::PathCompleter;
use super::theme::Theme;
use super::widgets::{
    InfoContent, InfoProjectData, InfoSessionData, InfoView, InfoViewState, Preview, PreviewState,
    TreeList, TreeListState,
};
use crate::api::{CommanderService, DiffSide};
use crate::backend::{
    AttachConnection, AttachKind, BResult, BackendError, BackendHandle, BackendId, BackendView,
    CommanderBackend, ConnectionState, LOCAL_BACKEND_ID, LocalBackend, PlaceholderBackend,
    RemoteBackendFactory, SessionRef,
};
use crate::config::{BindableAction, Config, ConfigStore, StateStore};
use crate::error::{Result, TuiError};
use crate::git::{
    AiSummary, BlockReason, DiffInfo, EnrichedPrInfo, diff_hash, fetch_branch_summary,
    fetch_enriched_pr, is_gh_available,
};
use crate::session::{AgentState, ProjectId, SessionId, SessionListItem, SessionStatus};

mod actions;
mod background;
mod conversation;
mod event_loop;
mod input;
mod modals;
mod render;
mod review;
mod selection;
mod settings;
mod state;

#[cfg(test)]
mod tests;

pub use review::DiffReviewState;
pub(crate) use review::ImageEntry;
pub use review::ReviewPrepared;

/// Direction for mouse scroll events
enum ScrollDirection {
    Up,
    Down,
}

/// iTerm2 advertises Kitty-graphics support (since 3.5) so the stdio probe can
/// land on `Kitty`, but iTerm2 renders that protocol unreliably — the escapes
/// are emitted and nothing is drawn. Its native inline-image protocol (OSC
/// 1337) is solid, so when the probe chose Kitty *and* the terminal identifies
/// as iTerm2, fall back to iTerm2. Returns the protocol to force, or `None` to
/// keep `detected`. Pure (env passed in) so it is unit-testable without a tty.
fn iterm2_kitty_override(
    detected: ratatui_image::picker::ProtocolType,
    term_program: Option<&str>,
    lc_terminal: Option<&str>,
) -> Option<ratatui_image::picker::ProtocolType> {
    use ratatui_image::picker::ProtocolType;
    if detected != ProtocolType::Kitty {
        return None;
    }
    let is_iterm2 = term_program.is_some_and(|t| t.contains("iTerm"))
        || lc_terminal.is_some_and(|t| t.contains("iTerm"));
    is_iterm2.then_some(ProtocolType::Iterm2)
}

/// Whether an ended attached session should be auto-restarted fresh.
///
/// Shell sessions (suffix `-sh`) and the project-less commander are never
/// auto-restarted: the commander is absent from `state.sessions`, so a
/// restart-by-name would fail — it is revived lazily by
/// [`commander::ensure_session`](crate::commander::ensure_session) on next
/// open. A crash-loop guard stops after 3 consecutive ends.
fn should_auto_restart_ended(session_name: &str, consecutive_ends: u8) -> bool {
    !session_name.ends_with("-sh")
        && session_name != crate::commander::COMMANDER_TMUX_NAME
        && consecutive_ends < 3
}

/// What the TUI attaches to once it tears down for an attach. Set by the
/// select / shell / commander handlers and consumed by the attach loop in
/// [`App::run`], which drives it over [`crate::tmux::run_attach`].
#[derive(Clone, Debug)]
pub enum AttachTarget {
    /// A session's pane, attached through the owning backend. `Ctrl+\` toggles
    /// between [`AttachKind::Agent`] and [`AttachKind::Shell`] on the same id.
    Session {
        session: SessionRef,
        kind: AttachKind,
    },
    /// A name-only tmux session with no `SessionId` — the commander session or
    /// a project shell — attached via the local backend by name. Local-only:
    /// these are absent from the workspace snapshot.
    LocalName(String),
}

/// Minimum left pane width as a percentage of the content area
const MIN_LEFT_PANE_PCT: u16 = 15;
/// Maximum left pane width as a percentage of the content area
const MAX_LEFT_PANE_PCT: u16 = 60;
/// Default left pane width as a percentage of the content area
const DEFAULT_LEFT_PANE_PCT: u16 = 30;

/// Which pane is currently focused
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusedPane {
    #[default]
    SessionList,
    RightPane,
}

/// Which view is shown in the right pane
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RightPaneView {
    #[default]
    Preview,
    Info,
    Shell,
}

// `ViewMode` lives in `crate::config` so that `AppState` can persist it.
// Re-exported here so existing call sites in this module's submodules
// (which all do `use super::*;`) keep compiling unchanged.
pub use crate::config::ViewMode;

/// A single entry in the pre-computed stack chain for the Info pane.
#[derive(Debug, Clone)]
pub struct StackChainEntry {
    pub title: String,
    pub status: SessionStatus,
    pub is_current: bool,
}

/// Modal dialog state
/// Caret glyph shown at the cursor position in single-line text inputs,
/// matching the review comment box.
pub(super) const INPUT_CARET: char = '▏';

/// Render a [`tui_input::Input`]'s value with the caret glyph spliced in at the
/// cursor (appended when the cursor is at the end). Single-line inputs across
/// the app render through this so the caret tracks the cursor instead of
/// sitting at the end of the text.
pub(super) fn input_with_caret(input: &Input) -> String {
    let chars: Vec<char> = input.value().chars().collect();
    let mut out = String::with_capacity(input.value().len() + INPUT_CARET.len_utf8());
    for (i, ch) in chars.iter().enumerate() {
        if i == input.cursor() {
            out.push(INPUT_CARET);
        }
        out.push(*ch);
    }
    if input.cursor() >= chars.len() {
        out.push(INPUT_CARET);
    }
    out
}

/// Forward an editing key to a single-line `tui-input` field, returning `true`
/// when the text value changed (so callers can recompute dependent state such
/// as fuzzy filters or path completions). Cursor-only moves and keys `tui-input`
/// doesn't recognise (Enter, Esc, Tab, Up/Down, …) return `false` and are left
/// for the caller to handle.
pub(super) fn edit_text_input(input: &mut Input, key: crossterm::event::KeyEvent) -> bool {
    use tui_input::backend::crossterm::to_input_request;
    match to_input_request(&crossterm::event::Event::Key(key)) {
        Some(req) => input.handle(req).is_some_and(|c| c.value),
        None => false,
    }
}

/// Insert a string at the cursor of a `tui-input` field (it has no bulk insert).
pub(super) fn insert_into_input(input: &mut Input, s: &str) {
    for c in s.chars() {
        input.handle(tui_input::InputRequest::InsertChar(c));
    }
}

#[derive(Debug, Clone)]
pub enum Modal {
    /// No modal open
    None,
    /// Text input modal
    Input {
        title: String,
        prompt: String,
        value: Input,
        on_submit: InputAction,
        /// When `Some`, the dialog renders a dynamic hint indicating whether
        /// the sanitized session name corresponds to an already-existing
        /// branch (local or remote). Populated by `handle_new_session` /
        /// `handle_new_stacked_session`; other Input flows (Rename) leave
        /// this as `None`.
        ///
        /// Entries are the local-name form returned by `load_branch_entries`
        /// (i.e. remote-only `origin/<x>` is stored as just `<x>` to match
        /// `finalize_session`'s resolution logic).
        existing_branches: Option<Vec<String>>,
        /// When `Some`, the dialog renders a filterable project picker beneath
        /// the name field; the highlighted project becomes the session's target.
        /// Only the plain New Session flow populates this — stacked sessions are
        /// locked to the parent's project, and non-creating flows leave it `None`.
        project_picker: Option<ProjectPicker>,
        /// When `Some`, the dialog renders a program picker beneath the name
        /// field and the chosen entry's command launches the session. `None`
        /// for Input flows that don't create sessions (Rename, AddProject…).
        program_picker: Option<ProgramPicker>,
        /// Which field currently has focus. Tab cycles through the fields that
        /// are present (see `InputFocus::next`).
        focus: InputFocus,
        /// Whether the focused picker row's dropdown is expanded.
        /// `expanded && focus == Project` means the project dropdown is open
        /// (and likewise for the program picker). Focus cannot move while a
        /// dropdown is open (Tab/arrows are captured by the dropdown), so the
        /// flag stays consistent with `focus` without needing an explicit reset.
        expanded: bool,
    },
    /// Confirmation modal
    Confirm {
        title: String,
        message: String,
        on_confirm: ConfirmAction,
    },
    /// Path input modal with a live-filtered subdirectory list.
    ///
    /// The list is populated on open and re-filtered on every keystroke.
    /// Arrow keys move `completer.selected_idx`; `scroll` keeps the
    /// highlighted row inside the visible window (same pattern as
    /// `Modal::QuickSwitch`).
    PathInput {
        title: String,
        prompt: String,
        value: Input,
        on_submit: InputAction,
        completer: PathCompleter,
        /// First visible row of the completions list.
        scroll: usize,
    },
    /// Loading spinner modal (non-interactive). `hint`, when set, renders as a
    /// dimmed line beneath the spinner (e.g. how to turn the operation off).
    Loading {
        title: String,
        message: String,
        hint: Option<String>,
    },
    /// Help modal. `scroll` is the first visible line of `build_help_lines`.
    /// Clamped against the rendered content height in `render_help_modal`.
    Help { scroll: u16 },
    /// Error modal
    Error { message: String },
    /// Settings modal
    Settings(SettingsState),
    /// Quick-switch palette modal — searches sessions and/or commands.
    QuickSwitch {
        /// Entry mode. `Unified` mixes sessions and commands; `CommandOnly`
        /// was opened via Shift+leader and only shows commands regardless of
        /// query. A leading `>` in a `Unified` query *effectively* filters
        /// to commands without changing this field, so backspacing past the
        /// `>` naturally restores the unified view.
        mode: PaletteMode,
        query: Input,
        matches: Vec<QuickSwitchItem>,
        selected_idx: usize,
        /// Index of the first visible row — keeps `selected_idx` inside
        /// the visible window when the list is longer than can fit.
        scroll: usize,
    },
    /// Checkout-existing-branch modal. Shows an input field plus a
    /// filterable/scrollable list of branches (local + remote) and
    /// creates a worktree session from the selected branch on submit.
    CheckoutBranch {
        /// Project the session will belong to
        project_id: ProjectId,
        /// Current input text (filter + paste target)
        query: Input,
        /// All branches loaded from the repo (source for filtering)
        all_branches: Vec<BranchEntry>,
        /// Filtered view of branches matching `query`
        filtered: Vec<BranchEntry>,
        /// Index into `filtered` of the currently highlighted branch
        selected_idx: usize,
        /// Scroll offset into `filtered` (first visible row)
        scroll: usize,
        /// True while `git fetch origin` is running in the background
        fetching: bool,
    },
    /// Full-screen review-diff-and-comment view.
    ReviewDiff(Box<DiffReviewState>),
    /// Full-screen conversation overlay (view onto the headless `claude`
    /// session). View-only state; the session itself lives on `App`, so closing
    /// this leaves the conversation running.
    Conversation { input: Input, scroll: u16 },
}

/// A session match in the quick-switch modal
#[derive(Debug, Clone)]
pub struct QuickSwitchMatch {
    pub session_id: SessionId,
    pub title: String,
    pub branch: String,
    pub project_name: String,
    pub status: SessionStatus,
}

/// How the quick-switch palette was opened.
///
/// `Unified` is the default (plain leader key) — sessions and commands are
/// both shown. `CommandOnly` is entered via Shift+leader and restricts the
/// list to commands regardless of query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    Unified,
    CommandOnly,
    /// Section picker for a specific session. The palette is populated with
    /// one entry per configured `[[sections]]` plus an "Auto" entry; selecting
    /// an entry sets (or clears) the session's `section_override`.
    SectionPicker {
        session_id: SessionId,
    },
}

/// A row in the quick-switch palette — either an open session, a
/// keybound command, or a section-move target.
#[derive(Debug, Clone)]
pub enum QuickSwitchItem {
    Session(QuickSwitchMatch),
    Command(CommandEntry),
    /// Selecting this row pins `session_id` to `target` (Some = section name,
    /// None = "Auto" / clear override).
    SectionMove {
        session_id: SessionId,
        target: Option<String>,
        /// Pre-formatted display label.
        label: String,
    },
}

/// A command row in the quick-switch palette.
#[derive(Debug, Clone)]
pub struct CommandEntry {
    /// The action to dispatch when the user presses Enter on this row.
    pub action: BindableAction,
    /// Human-readable label (from `BindableAction::description`).
    pub label: &'static str,
    /// Pre-formatted key-binding string (from `KeyBindings::keys_display`).
    /// Empty when the action has no binding — the palette intentionally
    /// still lists these so it can function as the primary command surface
    /// as hotkeys are trimmed over time.
    pub keys: String,
}

/// A single branch entry in the checkout modal list
#[derive(Debug, Clone)]
pub struct BranchEntry {
    /// Local branch name used for checkout (e.g. `"feature-auth"`).
    /// For remote-only branches this is the remote ref without the
    /// `origin/` prefix.
    pub local_name: String,
    /// Label shown in the UI — for remote-only branches this is the
    /// full `origin/<name>` form; for local branches it's the same as
    /// `local_name`.
    pub display_name: String,
    /// True when this branch only exists remotely (no local tracking branch).
    pub is_remote: bool,
}

/// Which tab is active in the settings modal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    General,
    Conversation,
    Keybindings,
    Theme,
    Sections,
}

impl SettingsTab {
    const ALL: [SettingsTab; 5] = [
        Self::General,
        Self::Conversation,
        Self::Keybindings,
        Self::Theme,
        Self::Sections,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Conversation => "Conversation",
            Self::Keybindings => "Keybindings",
            Self::Theme => "Theme",
            Self::Sections => "Sections",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::General => Self::Conversation,
            Self::Conversation => Self::Keybindings,
            Self::Keybindings => Self::Theme,
            Self::Theme => Self::Sections,
            Self::Sections => Self::General,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::General => Self::Sections,
            Self::Conversation => Self::General,
            Self::Keybindings => Self::Conversation,
            Self::Theme => Self::Keybindings,
            Self::Sections => Self::Theme,
        }
    }
}

/// Which pane is focused in the Sections tab
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SectionsFocus {
    #[default]
    List,
    Predicates,
}

/// State for the Sections tab within the settings modal
#[derive(Debug, Clone)]
pub struct SectionsState {
    pub selected_section: usize,
    pub focus: SectionsFocus,
    pub pred_selected: usize,
    pub editing: Option<SectionsEditing>,
}

/// Editing state for the Sections tab
#[derive(Debug, Clone)]
pub enum SectionsEditing {
    RenamingSection { value: Input },
    EditingPredicate { value: Input },
    CreatingSection { value: Input },
}

impl Default for SectionsState {
    fn default() -> Self {
        Self {
            selected_section: 0,
            focus: SectionsFocus::List,
            pred_selected: 0,
            editing: None,
        }
    }
}

/// State for the settings modal
#[derive(Debug, Clone)]
pub struct SettingsState {
    pub tab: SettingsTab,
    pub selected_row: usize,
    pub editing: Option<SettingsEditing>,
    /// Cached row data for the current tab
    pub rows: Vec<SettingsRow>,
    /// State for the Sections tab (lazily initialised on first tab switch)
    pub sections_state: SectionsState,
    /// Active search filter for the Keybindings tab. `Some` while the search
    /// box is focused (typing filters the shortcut list live); `None` when the
    /// list is browsed normally.
    pub search: Option<Input>,
}

/// Kind of a settings row, carrying its typed value.
///
/// Each variant holds its own value — a display/edit string for `Text`, a live
/// `bool` for `Toggle` — so the row model is fully typed rather than stuffing
/// everything into a string. String conversion only happens at the
/// preferences-file boundary (serde) and the text-input edit path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsRowKind {
    /// Free-text field edited via a text-input box.
    Text(String),
    /// Two-state boolean rendered as a checkbox and flipped in place.
    Toggle(bool),
    /// A non-selectable section heading (Keybindings tab grouping). Carries no
    /// value and is skipped by navigation.
    Header,
}

/// A single row in the settings list
#[derive(Debug, Clone)]
pub struct SettingsRow {
    pub label: String,
    pub field_key: String,
    pub kind: SettingsRowKind,
    /// Optional color for displaying a swatch next to the value (Theme tab only)
    pub color_swatch: Option<Color>,
}

impl SettingsRow {
    /// A free-text settings row.
    pub fn text(
        label: impl Into<String>,
        value: impl Into<String>,
        field_key: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            field_key: field_key.into(),
            kind: SettingsRowKind::Text(value.into()),
            color_swatch: None,
        }
    }

    /// A two-state boolean toggle row.
    pub fn toggle(label: impl Into<String>, on: bool, field_key: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            field_key: field_key.into(),
            kind: SettingsRowKind::Toggle(on),
            color_swatch: None,
        }
    }

    /// A non-selectable section heading.
    pub fn header(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            field_key: String::new(),
            kind: SettingsRowKind::Header,
            color_swatch: None,
        }
    }

    /// Whether this row can be highlighted/edited. Section headers cannot.
    pub fn is_selectable(&self) -> bool {
        !matches!(self.kind, SettingsRowKind::Header)
    }

    /// A text row that also displays a color swatch (Theme tab).
    pub fn swatch(
        label: impl Into<String>,
        value: impl Into<String>,
        field_key: impl Into<String>,
        color: Color,
    ) -> Self {
        Self {
            label: label.into(),
            field_key: field_key.into(),
            kind: SettingsRowKind::Text(value.into()),
            color_swatch: Some(color),
        }
    }

    /// The display/edit string for a `Text` row; `""` for a `Toggle` row.
    pub fn text_value(&self) -> &str {
        match &self.kind {
            SettingsRowKind::Text(v) => v,
            SettingsRowKind::Toggle(_) | SettingsRowKind::Header => "",
        }
    }

    /// For a Toggle row, the flipped boolean; `None` for other kinds.
    pub fn toggled(&self) -> Option<bool> {
        match self.kind {
            SettingsRowKind::Toggle(on) => Some(!on),
            SettingsRowKind::Text(_) | SettingsRowKind::Header => None,
        }
    }
}

/// Editing state within the settings modal
#[derive(Debug, Clone)]
pub enum SettingsEditing {
    /// Editing a text value
    TextInput { value: Input },
    /// Capturing a key for keybinding
    KeyCapture {
        action_name: String,
        keys: Vec<String>,
    },
    /// Picking from a list of options (used for theme presets)
    OptionPicker {
        options: Vec<String>,
        selected: usize,
    },
}

/// Which field of the New Session input modal currently has focus. Tab cycles
/// through the fields that are present; the modal owns the value so the
/// name/project/program sections stay mutually exclusive by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFocus {
    /// The session-name text field (default on open).
    Name,
    /// The filterable project picker.
    Project,
    /// The program/agent picker.
    Program,
}

impl InputFocus {
    /// Ordered ring of the fields that exist; Name is always present.
    fn ring(has_project: bool, has_program: bool) -> Vec<InputFocus> {
        let mut ring = vec![InputFocus::Name];
        if has_project {
            ring.push(InputFocus::Project);
        }
        if has_program {
            ring.push(InputFocus::Program);
        }
        ring
    }

    /// The next focus when Tab is pressed, skipping fields that aren't present.
    /// Cycles Name → Project → Program → Name.
    pub fn next(self, has_project: bool, has_program: bool) -> InputFocus {
        let ring = Self::ring(has_project, has_program);
        let idx = ring.iter().position(|f| *f == self).unwrap_or(0);
        ring[(idx + 1) % ring.len()]
    }

    /// The previous focus when Shift+Tab is pressed, skipping absent fields.
    /// Cycles Name → Program → Project → Name.
    pub fn prev(self, has_project: bool, has_program: bool) -> InputFocus {
        let ring = Self::ring(has_project, has_program);
        let idx = ring.iter().position(|f| *f == self).unwrap_or(0);
        ring[(idx + ring.len() - 1) % ring.len()]
    }
}

/// Maximum project rows shown at once in the New Session project picker; the
/// list scrolls to keep the highlighted row visible. Shared by the input
/// handler (scroll bookkeeping) and the renderer.
pub(super) const MAX_PROJECT_ROWS: usize = 6;

/// Program-picker state embedded in the New Session input modal: the
/// selectable harnesses and the highlighted index.
#[derive(Debug, Clone)]
pub struct ProgramPicker {
    /// Selectable harnesses, from `Config::program_choices`.
    pub choices: Vec<crate::config::ProgramEntry>,
    /// Index into `choices` of the highlighted entry.
    pub selected: usize,
}

impl ProgramPicker {
    /// The launch command of the highlighted entry, if any.
    pub fn selected_command(&self) -> Option<String> {
        self.choices.get(self.selected).map(|e| e.command.clone())
    }

    /// Move the highlight up one entry (saturating at the top).
    pub fn select_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Move the highlight down one entry (saturating at the bottom).
    pub fn select_down(&mut self) {
        if self.selected + 1 < self.choices.len() {
            self.selected += 1;
        }
    }
}

/// A project that can be chosen as the target of a new session.
#[derive(Debug, Clone)]
pub struct ProjectChoice {
    pub id: ProjectId,
    pub name: String,
    pub repo_path: PathBuf,
}

/// Filterable project-picker state embedded in the New Session input modal.
/// `choices` holds every project (sorted by name); `filtered` is the subset
/// matching `filter`, and `selected` indexes into `filtered`.
#[derive(Debug, Clone)]
pub struct ProjectPicker {
    pub choices: Vec<ProjectChoice>,
    pub filter: String,
    pub filtered: Vec<usize>,
    pub selected: usize,
    /// First visible row (index into `filtered`) — keeps the highlight on
    /// screen as the list scrolls, maintained via `adjust_list_scroll`.
    pub scroll: usize,
    /// Memoized branch lists per repo path, so switching the highlight only
    /// lists a given project's branches once per dialog session.
    pub branch_cache: HashMap<PathBuf, Option<Vec<String>>>,
}

impl ProjectPicker {
    /// Build a picker over `choices`, highlighting `default` (falling back to
    /// the first entry). The filter starts empty, so all choices are visible.
    pub fn new(choices: Vec<ProjectChoice>, default: ProjectId) -> Self {
        let filtered = (0..choices.len()).collect();
        let selected = choices.iter().position(|c| c.id == default).unwrap_or(0);
        let mut picker = Self {
            choices,
            filter: String::new(),
            filtered,
            selected,
            scroll: 0,
            branch_cache: HashMap::new(),
        };
        picker.adjust_scroll();
        picker
    }

    /// Keep `scroll` positioned so the highlighted row stays visible, reusing
    /// the same window helper as the other scrolling lists.
    fn adjust_scroll(&mut self) {
        self.scroll = actions::adjust_list_scroll(self.selected, self.scroll, MAX_PROJECT_ROWS);
    }

    /// The highlighted project, if any.
    pub fn selected_choice(&self) -> Option<&ProjectChoice> {
        self.filtered
            .get(self.selected)
            .and_then(|&i| self.choices.get(i))
    }

    /// The id of the highlighted project, if any.
    pub fn selected_id(&self) -> Option<ProjectId> {
        self.selected_choice().map(|c| c.id)
    }

    /// The repo path of the highlighted project, if any.
    pub fn selected_repo_path(&self) -> Option<PathBuf> {
        self.selected_choice().map(|c| c.repo_path.clone())
    }

    /// Move the highlight up one entry (saturating at the top).
    pub fn select_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.adjust_scroll();
    }

    /// Move the highlight down one entry (saturating at the bottom).
    pub fn select_down(&mut self) {
        if self.selected + 1 < self.filtered.len() {
            self.selected += 1;
        }
        self.adjust_scroll();
    }

    /// Recompute `filtered` from `filter`, re-anchoring the highlight onto the
    /// previously-selected project when it survives (else clamp to the top).
    /// An empty filter shows every project in name order; otherwise entries are
    /// ranked best-fuzzy-match first via `crate::fuzzy::fuzzy_score`.
    pub fn apply_filter(&mut self) {
        let prev_id = self.selected_id();
        if self.filter.is_empty() {
            self.filtered = (0..self.choices.len()).collect();
        } else {
            let mut scored: Vec<(usize, i64)> = self
                .choices
                .iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    crate::fuzzy::fuzzy_score(&c.name, &self.filter).map(|s| (i, s))
                })
                .collect();
            // Highest score first; ties keep the original (name-sorted) order.
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            self.filtered = scored.into_iter().map(|(i, _)| i).collect();
        }
        self.selected = prev_id
            .and_then(|id| self.filtered.iter().position(|&i| self.choices[i].id == id))
            .unwrap_or(0);
        self.adjust_scroll();
    }
}

/// Action to perform when input modal is submitted
#[derive(Debug, Clone)]
pub enum InputAction {
    CreateSession {
        project_id: ProjectId,
        /// Section the tree cursor was in when the modal opened; the new
        /// session is placed there (`None` = default "In Progress").
        section: Option<String>,
    },
    CreateStackedSession {
        project_id: ProjectId,
        parent_session_id: SessionId,
        parent_branch: String,
    },
    AddProject,
    ScanDirectory,
    RenameSession {
        session_id: SessionId,
    },
}

/// Action to perform when confirm modal is confirmed
#[derive(Debug, Clone)]
pub enum ConfirmAction {
    DeleteSession { session_id: SessionId },
    DeleteMergedPrSessions { session_ids: Vec<SessionId> },
    RestartSession { session_id: SessionId },
    RemoveProject { project_id: ProjectId },
}

/// Application UI state
pub struct AppUiState {
    /// Session list state
    pub list_state: TreeListState,
    /// Preview pane state
    pub preview_state: PreviewState,
    /// Info pane state
    pub info_state: InfoViewState,
    /// Enriched PR info for the currently selected session
    pub enriched_pr: Option<(SessionId, EnrichedPrInfo)>,
    /// Cached AI summaries keyed by session ID
    pub ai_summaries: std::collections::HashMap<SessionId, AiSummary>,
    /// Currently focused pane
    pub focused_pane: FocusedPane,
    /// Which view is shown in the right pane
    pub right_pane_view: RightPaneView,
    /// Current modal
    pub modal: Modal,
    /// Session list items (flattened hierarchy)
    pub list_items: Vec<SessionListItem>,
    /// Preview content
    pub preview_content: String,
    /// Shell pane state
    pub shell_state: PreviewState,
    /// Shell content
    pub shell_content: String,
    /// Diff info
    pub diff_info: Arc<DiffInfo>,
    /// Status message (with expiry time)
    pub status_message: Option<(String, Instant)>,
    /// Should quit
    pub should_quit: bool,
    /// Last known terminal size (updated each render frame)
    pub terminal_size: Rect,
    /// Inner rect of the review-diff body pane, recorded each render frame so
    /// mouse events can map a screen position to a diff line. `None` unless the
    /// review view is open.
    pub review_body_rect: Option<Rect>,
    /// Inner rect of the review-diff file-list pane, recorded each render frame
    /// so mouse events can map a screen position to a tree row. `None` unless
    /// the review view is open.
    pub review_file_list_rect: Option<Rect>,
    /// Rows-only area of the open list modal (quick-switch, checkout-branch,
    /// or path-input completions), recorded each render frame so mouse clicks
    /// can be mapped to list indices. `None` when no list modal is open.
    pub modal_list_rect: Option<Rect>,
    /// Clickable action-button regions from the main-view status bar, recorded
    /// each render frame so a left-click can be mapped back to its
    /// `BindableAction`. Rebuilt every frame; empty when no buttons are drawn.
    pub action_buttons: Vec<crate::tui::hotkey::ActionButton>,
    /// Clickable action-button regions from the review-view footer, recorded
    /// each render frame. Review keys are view-local, so each button carries a
    /// synthesized `KeyEvent` fed straight into `handle_review_key`.
    pub review_buttons: Vec<review::ReviewButton>,
    /// Sessions with at least one pending (not-yet-applied) review comment.
    /// Drives the `*` marker in the session list; refreshed on startup and
    /// whenever the review view closes.
    pub sessions_with_comments: HashSet<SessionId>,
    /// Currently selected session (for preview/diff), qualified by the backend
    /// that owns it so actions route to the right machine.
    pub selected_session_id: Option<SessionRef>,
    /// Currently selected project, qualified by its owning backend.
    pub selected_project_id: Option<(BackendId, ProjectId)>,
    /// Whether the backend owning the current selection is connected. Cached in
    /// `update_selection` so the (sync, backend-unaware) `is_command_available`
    /// can gate actions on a live backend. Always `true` for the local backend.
    pub selected_backend_connected: bool,
    /// Whether the `cc-commander` tmux session is currently running. Cached from
    /// the background agent-state poll so the (sync) renderers — the footer chip
    /// — can read it without awaiting tmux.
    pub commander_running: bool,
    /// What to attach to after the TUI tears down (set by select/shell/commander).
    pub attach_request: Option<AttachTarget>,
    /// Session whose review diff should be opened on returning to the TUI —
    /// set when the user pressed Alt-r inside an attached session.
    pub pending_open_review: Option<SessionId>,
    /// Editor command + path to open after exiting TUI
    pub editor_command: Option<(String, PathBuf)>,
    /// Needs right pane clear (set on view switch, consumed on render)
    pub clear_right_pane: bool,
    /// Left pane width as a percentage (adjustable at runtime via < / >)
    pub left_pane_pct: u16,
    /// Whether the `gh` CLI is available
    pub gh_available: bool,
    /// When the last background preview fetch was spawned (None = not in flight)
    pub preview_update_spawned_at: Option<Instant>,
    /// Whether a review-diff refresh re-compose is currently in flight, so the
    /// idle trigger and a manual refresh don't double-spawn.
    pub review_refresh_in_flight: bool,
    /// Tick counter for animations (incremented each render tick)
    pub tick_count: u64,
    /// Throbber/spinner state for loading modals
    pub throbber_state: throbber_widgets_tui::ThrobberState,
    /// Current agent states for Running Claude sessions (ephemeral, from background poller)
    pub agent_states: HashMap<SessionId, AgentState>,
    /// Cached mirror of `AppState::cascade_paused_at.is_some()` — used by
    /// `is_command_available` to gate the `CascadeResume` / `CascadeAbandon`
    /// palette entries without an async read on every keystroke. Refreshed
    /// alongside `list_items`.
    pub cascade_paused: bool,
    /// Section names that are currently collapsed in the list view.
    pub collapsed_sections: std::collections::HashSet<String>,
    /// Last left-mouse click on a session-list row: (list index, timestamp).
    /// Used to detect double-click on the same row within `DOUBLE_CLICK_WINDOW`.
    pub last_left_click: Option<(usize, Instant)>,
    /// Last left-mouse click in the review diff body: (screen row, timestamp).
    /// Used to detect a double-click on the same row within `DOUBLE_CLICK_WINDOW`,
    /// which opens a comment box like a right-click.
    pub review_last_click: Option<(u16, Instant)>,
    /// Last left-mouse click on a list-modal row: (absolute list index,
    /// timestamp). Used to detect double-click on the same row within
    /// `DOUBLE_CLICK_WINDOW`. Cleared on any modal keystroke or paste, since
    /// typing can refilter the list out from under a pending click.
    pub modal_list_last_click: Option<(usize, Instant)>,
    /// Current list view mode (project-grouped vs section-grouped).
    pub view_mode: ViewMode,
    /// Pre-computed stack chain for the selected session (empty if not stacked).
    pub stack_chain: Vec<StackChainEntry>,
    /// Projects whose most recent background pull was held back, with the
    /// reason. Folded out of the workspace snapshot's `project_pull` (which the
    /// service's pull loop maintains) to drive the per-project row badge.
    pub project_pull_blocked: HashMap<ProjectId, BlockReason>,
}

impl Default for AppUiState {
    fn default() -> Self {
        Self {
            list_state: TreeListState::new(),
            preview_state: PreviewState::new(),
            info_state: InfoViewState::new(),
            enriched_pr: None,
            ai_summaries: std::collections::HashMap::new(),
            shell_state: PreviewState::new(),
            shell_content: String::new(),
            focused_pane: FocusedPane::default(),
            right_pane_view: RightPaneView::default(),
            modal: Modal::None,
            list_items: Vec::new(),
            preview_content: String::new(),
            diff_info: Arc::new(DiffInfo::empty()),
            status_message: None, // (message, expiry)
            review_body_rect: None,
            review_file_list_rect: None,
            modal_list_rect: None,
            action_buttons: Vec::new(),
            review_buttons: Vec::new(),
            sessions_with_comments: HashSet::new(),

            should_quit: false,
            selected_session_id: None,
            selected_project_id: None,
            selected_backend_connected: true,
            commander_running: false,
            attach_request: None,
            pending_open_review: None,
            editor_command: None,
            clear_right_pane: false,
            left_pane_pct: DEFAULT_LEFT_PANE_PCT,
            gh_available: false,
            preview_update_spawned_at: None,
            review_refresh_in_flight: false,
            terminal_size: Rect::default(),
            tick_count: 0,
            throbber_state: throbber_widgets_tui::ThrobberState::default(),
            agent_states: HashMap::new(),
            cascade_paused: false,
            collapsed_sections: std::collections::HashSet::new(),
            last_left_click: None,
            review_last_click: None,
            modal_list_last_click: None,
            view_mode: ViewMode::default(),
            stack_chain: Vec::new(),
            project_pull_blocked: HashMap::new(),
        }
    }
}

impl AppUiState {
    /// Whether a given command is currently invokable.
    ///
    /// These rules mirror the early-return guards scattered across
    /// `App::handle_command` and friends, so the palette only lists
    /// commands that would actually *do* something if selected. Pure with
    /// respect to `self` — safe to unit-test by constructing a default
    /// `AppUiState` and mutating a few fields.
    pub fn is_command_available(&self, action: BindableAction) -> bool {
        // A degraded/connecting remote backend can't service actions against its
        // sessions/projects, so gate those on the selected backend being live.
        // The local backend is always connected, so single-machine setups are
        // unaffected.
        let connected = self.selected_backend_connected;
        let has_session = self.selected_session_id.is_some() && connected;
        let has_project = self.selected_project_id.is_some() && connected;
        match action {
            // Session-scoped actions require a selected session
            BindableAction::Select
            | BindableAction::SelectShell
            | BindableAction::DeleteSession
            | BindableAction::RenameSession
            | BindableAction::RestartSession
            | BindableAction::OpenInEditor
            | BindableAction::OpenPullRequest
            | BindableAction::OpenReviewDiff
            | BindableAction::MoveToSection => has_session,
            // Cascade merge is only meaningful from a session that's part of
            // a stack. We accept any selected session here; the handler is
            // cheap to no-op if the stack chain turns out to be length 1.
            BindableAction::CascadeMergeMain | BindableAction::PushStack => has_session,
            // Cascade resume / abandon are only meaningful when a cascade is paused.
            BindableAction::CascadeResume | BindableAction::CascadeAbandon => self.cascade_paused,
            // Removing a project is only meaningful from a project row (no session selected)
            BindableAction::RemoveProject => has_project && !has_session,
            // GenerateSummary only does something when the Info pane is active
            BindableAction::GenerateSummary => {
                has_session && self.right_pane_view == RightPaneView::Info
            }
            // All other actions are always available
            _ => true,
        }
    }

    /// Build the palette's command rows for a given filter query.
    ///
    /// Commands with no effective keybinding are still included — the
    /// palette is intended to be the canonical access surface as hotkeys
    /// get trimmed over time. Pure list motions (navigate up/down,
    /// group/first/last jumps) are excluded because moving the tree cursor
    /// from inside the palette makes no sense.
    pub fn gather_command_entries(
        &self,
        kb: &crate::config::KeyBindings,
        filter_query: &str,
    ) -> Vec<CommandEntry> {
        let mut scored: Vec<(i64, CommandEntry)> = Vec::new();
        for &action in BindableAction::ALL {
            if matches!(
                action,
                BindableAction::NavigateUp
                    | BindableAction::NavigateDown
                    | BindableAction::NextGroup
                    | BindableAction::PreviousGroup
                    | BindableAction::NavigateFirst
                    | BindableAction::NavigateLast
            ) {
                continue;
            }
            if !self.is_command_available(action) {
                continue;
            }
            let label = action.description();
            let Some(score) = crate::fuzzy::fuzzy_score(label, filter_query) else {
                continue;
            };
            scored.push((
                score,
                CommandEntry {
                    action,
                    label,
                    keys: kb.keys_display(action),
                },
            ));
        }
        if !filter_query.is_empty() {
            // Stable sort by score desc preserves enum order among ties.
            scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        }
        scored.into_iter().map(|(_, e)| e).collect()
    }
}

/// Main TUI application
pub struct App {
    /// Local config cache — refreshed from config_store on tick when file changes
    config: Config,
    /// Frozen snapshot of `commander_enabled`, captured at startup. The
    /// commander's enablement is restart-required: the agent-state poll task
    /// captures it at spawn, so the footer chip reads this same frozen value
    /// rather than the hot-reloaded `config.commander_enabled`. Toggling it at
    /// runtime only surfaces the restart warning; the commander UI doesn't move
    /// until the next launch.
    commander_enabled_at_init: bool,
    /// Unified service layer — owns SessionManager, StateStore, and ConfigStore.
    ///
    /// PHASE-C transition: retained while store/service call sites are migrated
    /// onto `backends`. The end state drops this field entirely; until then the
    /// local `BackendHandle` wraps a clone of this same service.
    service: CommanderService,
    /// The backends the TUI drives. Exactly one (the local backend) this phase;
    /// Phase E adds remote entries. Each holds a cached [`BackendView`] the
    /// render path reads synchronously, refreshed by a per-backend change-feed
    /// task via [`StateUpdate::BackendChanged`](crate::tui::event::StateUpdate).
    backends: Vec<BackendHandle>,
    /// Builds a remote backend from its config entry. Injected by the binary so
    /// core never links the remote client crate. Held past construction so the
    /// config hot-reload path can build backends for servers added at runtime.
    remote_factory: RemoteBackendFactory,
    /// Monotonic allocator for remote [`BackendId`]s. Ids are stable for a
    /// backend's lifetime and never reused, so an aborted feed task's in-flight
    /// message can't land on a freshly-added backend that happened to reuse its
    /// slot. Starts past the startup-assigned ids.
    next_remote_backend_id: usize,
    /// Frontend-owned UI preferences (view mode, last selection, pane width),
    /// persisted to `tui.json` — kept out of `state.json` so backend/session
    /// data and local UI prefs never share a file. See [`crate::tui::prefs`].
    tui_prefs: crate::tui::prefs::TuiPrefsStore,
    /// UI state
    ui_state: AppUiState,
    /// Event loop
    event_loop: EventLoop,
    /// Theme configuration
    theme: Theme,
    /// Suppress key events until this instant (filters stray bytes from
    /// unrecognized escape sequences that crossterm splits into multiple events)
    suppress_keys_until: Instant,
    /// Two-digit session number accumulator with debounce
    digit_accumulator: super::digit_accumulator::DigitAccumulator,
    /// Conversation mode runtime (headless streaming `claude` session + TTS).
    /// Lives here, not in the overlay modal, so it keeps running while closed.
    conversation: conversation::ConversationRuntime,
    /// Terminal graphics capability for the review image view, probed ONCE
    /// before the input reader starts (see `run`); `None` until then. Kept on
    /// `App` rather than in `DiffReviewState` because the protocol cache below
    /// isn't `Clone`, and `DiffReviewState` is.
    picker: Option<ratatui_image::picker::Picker>,
    /// Decoded review images keyed by (display path, side). Interior-mutable so
    /// the `&self` render path can read them; populated by the background fetch
    /// task via `StateUpdate::ReviewImageLoaded`. Cleared when a review opens.
    review_images: RefCell<HashMap<(String, DiffSide), review::ImageEntry>>,
    /// Monotonic generation, bumped each time a review opens (when
    /// `review_images` is cleared). A background fetch captures the generation
    /// it was spawned under; a late arrival from a since-closed review carries a
    /// stale generation and is dropped, so it can't poison the new review's
    /// cache (e.g. a same-named path in a different session).
    review_image_gen: Cell<u64>,
}

impl App {
    /// Create a new application. `frontend` identifies this binary for
    /// telemetry and is forwarded to [`CommanderService::new`].
    pub fn new(
        config_store: Arc<ConfigStore>,
        store: Arc<StateStore>,
        frontend: crate::telemetry::FrontendInfo,
        remote_factory: RemoteBackendFactory,
    ) -> Self {
        let config = config_store.read().clone();
        // Build the TUI prefs store from the same data dir as the state file
        // (migrating legacy prefs out of `state.json` on first launch) before
        // the store is moved into the service.
        let tui_prefs = crate::tui::prefs::TuiPrefsStore::load(&store.data_dir());
        let service = CommanderService::new(config_store, store, frontend);

        // The local backend wraps a clone of the same service (a bundle of
        // Arcs). During the Phase-C transition both coexist; call sites migrate
        // from `service` onto `backends` incrementally.
        let local: Arc<dyn CommanderBackend> = Arc::new(LocalBackend::new(service.clone()));
        let mut backends = vec![BackendHandle::new(LOCAL_BACKEND_ID, local)];
        // One backend per configured remote server, in declared order, starting
        // at BackendId(1). A server that fails to construct becomes a
        // permanently-degraded placeholder so it still shows in the tree.
        for (i, server) in config.remote_servers.iter().enumerate() {
            backends.push(Self::build_remote_handle(
                BackendId(i + 1),
                server,
                &remote_factory,
            ));
        }

        let base = config
            .theme
            .preset
            .as_deref()
            .and_then(Theme::from_preset)
            .unwrap_or_default();
        let theme = base.with_overrides(&config.theme);
        let debounce = Duration::from_millis(config.session_number_debounce_ms);
        let commander_enabled_at_init = config.commander_enabled;

        // Startup assigned remote ids 1..=len; the next allocation continues
        // past them so re-added servers get fresh, never-reused ids.
        let next_remote_backend_id = backends.len();

        Self {
            config,
            commander_enabled_at_init,
            service,
            backends,
            remote_factory,
            next_remote_backend_id,
            tui_prefs,
            ui_state: AppUiState::default(),
            event_loop: EventLoop::new(),
            theme,
            suppress_keys_until: Instant::now(),
            digit_accumulator: super::digit_accumulator::DigitAccumulator::new(debounce),
            conversation: conversation::ConversationRuntime::default(),
            picker: None,
            review_images: RefCell::new(HashMap::new()),
            review_image_gen: Cell::new(0),
        }
    }

    /// Construct the [`BackendHandle`] for one configured remote server: the
    /// real backend from the factory, or — if construction fails — a
    /// [`PlaceholderBackend`] whose view is seeded `Degraded` with the reason, so
    /// the server still appears in the tree as a permanently-errored header
    /// rather than crashing startup or silently disappearing.
    fn build_remote_handle(
        id: BackendId,
        server: &crate::config::RemoteServerConfig,
        remote_factory: &RemoteBackendFactory,
    ) -> BackendHandle {
        match remote_factory(server) {
            Ok(backend) => BackendHandle::new(id, backend),
            Err(e) => {
                warn!(
                    "Remote backend '{}' failed to construct: {} — showing as degraded",
                    server.name, e
                );
                let placeholder: Arc<dyn CommanderBackend> =
                    Arc::new(PlaceholderBackend::new(server.name.clone(), e.to_string()));
                let mut handle = BackendHandle::new(id, placeholder);
                handle.view.connection = ConnectionState::Degraded {
                    reason: e.to_string(),
                };
                handle
            }
        }
    }

    /// Reconcile the live backends against a hot-reloaded `remote_servers` list:
    /// drop handles for removed/changed servers (their `Drop` aborts the polling
    /// tasks), construct handles for added/changed servers, then reorder the Vec
    /// to match config order (local stays first). Selection on a dropped backend
    /// falls back to the local backend.
    fn apply_remote_servers_reload(
        &mut self,
        old: &[crate::config::RemoteServerConfig],
        new: &[crate::config::RemoteServerConfig],
    ) {
        let recon = reconcile_remote_servers(old, new);

        // Remove dropped/changed backends. Capture their ids first so a
        // selection pointing at one can fall back to local.
        if !recon.removed.is_empty() {
            let removed_ids: Vec<BackendId> = self
                .backends
                .iter()
                .filter(|h| {
                    h.id != LOCAL_BACKEND_ID && recon.removed.contains(&h.backend.descriptor().name)
                })
                .map(|h| h.id)
                .collect();
            // Dropping the handle aborts its feed tasks (see BackendHandle::drop).
            self.backends
                .retain(|h| h.id == LOCAL_BACKEND_ID || !removed_ids.contains(&h.id));

            let session_gone = self
                .ui_state
                .selected_session_id
                .is_some_and(|r| removed_ids.contains(&r.backend));
            let project_gone = self
                .ui_state
                .selected_project_id
                .is_some_and(|(b, _)| removed_ids.contains(&b));
            if session_gone {
                self.ui_state.selected_session_id = None;
            }
            if session_gone || project_gone {
                self.ui_state.selected_project_id = None;
                self.ui_state.selected_backend_connected = true;
            }
        }

        // Construct and wire added/changed backends.
        for server in &recon.added {
            let id = BackendId(self.next_remote_backend_id);
            self.next_remote_backend_id += 1;
            let mut handle = Self::build_remote_handle(id, server, &self.remote_factory);
            handle.feed_tasks = self.spawn_backend_feeds_for(&handle);
            self.backends.push(handle);
        }

        // Reorder so the tree shows local first, then remotes in config order.
        let position = |name: &str| new.iter().position(|s| s.name == name);
        self.backends.sort_by_key(|h| {
            if h.id == LOCAL_BACKEND_ID {
                (0usize, 0usize)
            } else {
                (
                    1,
                    position(&h.backend.descriptor().name).unwrap_or(usize::MAX),
                )
            }
        });
    }

    // -- Backend accessors (Phase C) --

    /// The backend handle for `id`, if present.
    pub(super) fn backend(&self, id: BackendId) -> Option<&BackendHandle> {
        self.backends.iter().find(|h| h.id == id)
    }

    /// The backend trait object for `id`, cloneable into a spawned task. Falls
    /// back to the local backend when the id is unknown (e.g. its server was
    /// just removed) so callers never panic on a stale id.
    pub(super) fn backend_arc(&self, id: BackendId) -> Arc<dyn CommanderBackend> {
        self.backend(id)
            .map(|h| h.backend.clone())
            .unwrap_or_else(|| self.local_arc())
    }

    /// The backend trait object that owns session ref `r`.
    pub(super) fn backend_for(&self, r: SessionRef) -> Arc<dyn CommanderBackend> {
        self.backend_arc(r.backend)
    }

    /// The cached view of the backend that owns `r` (its snapshot + connection),
    /// or the local view if the backend id is unknown.
    pub(super) fn view_for(&self, backend: BackendId) -> &BackendView {
        self.backend(backend)
            .map(|h| &h.view)
            .unwrap_or_else(|| self.local_view())
    }

    /// The local backend's cached view (single-backend convenience this phase).
    pub(super) fn local_view(&self) -> &BackendView {
        &self.backends[0].view
    }

    /// The local backend trait object, cloneable into a spawned task.
    pub(super) fn local_arc(&self) -> Arc<dyn CommanderBackend> {
        self.backends[0].backend.clone()
    }

    /// Record a UI-only telemetry feature against the local backend.
    pub(super) fn record_feature(&self, feature: &'static str) {
        self.backends[0].backend.record_feature(feature);
    }

    /// Fetch a fresh snapshot + agent states into the local backend's cached
    /// view. Call after a user-initiated backend mutation so the very next
    /// `refresh_list_items` reflects it, rather than waiting a change-feed cycle
    /// (the change-feed task still delivers a redundant, idempotent refresh).
    pub(super) async fn refresh_local_view(&mut self) {
        self.refresh_backend_view(LOCAL_BACKEND_ID).await;
    }

    /// Fetch a fresh snapshot + agent states into `id`'s cached view, so the
    /// very next `refresh_list_items` reflects a just-issued mutation without
    /// waiting a change-feed cycle. A failed fetch leaves the cached view (and
    /// its connection state) untouched, so a degraded remote keeps its last
    /// snapshot rather than blanking.
    pub(super) async fn refresh_backend_view(&mut self, id: BackendId) {
        let backend = self.backend_arc(id);
        let snapshot = backend.workspace_snapshot().await;
        let states = backend.agent_states(false).await;
        if let Some(handle) = self.backends.iter_mut().find(|h| h.id == id) {
            if let Ok(snapshot) = snapshot {
                handle.view.snapshot = snapshot;
                handle.view.connection = ConnectionState::Connected;
            }
            if let Ok(states) = states {
                handle.view.agent_states = states;
            }
        }
    }

    /// Test-only: fold the current store state into the local backend's cached
    /// view. Production populates the view via `bootstrap_backend_views` and the
    /// change-feed task; tests that seed the store directly (bypassing backend
    /// mutations, so no change-feed bump fires) call this before asserting on
    /// the rendered tree.
    #[cfg(test)]
    pub(super) async fn sync_local_view_from_store_for_test(&mut self) {
        // Read back through the backend (which wraps the same store the test
        // seeded) rather than the store directly, so this stays clear of the
        // Phase-C store-access gate.
        if let Ok(snapshot) = self.local_arc().workspace_snapshot().await {
            self.backends[0].view.snapshot = snapshot;
            self.backends[0].view.connection = crate::backend::ConnectionState::Connected;
        }
        // Mirror any test-injected agent states (set on `ui_state.agent_states`
        // directly, since tmux isn't live in tests) into the local backend view,
        // which is what `refresh_list_items` reads per-backend.
        self.backends[0].view.agent_states.states = self.ui_state.agent_states.clone();
    }

    /// Look up a session in a backend's cached snapshot.
    pub(super) fn session(&self, r: SessionRef) -> Option<&crate::api::SessionInfo> {
        self.backend(r.backend)?
            .view
            .snapshot
            .sessions
            .iter()
            .find(|s| s.session_id == r.id)
    }

    /// Look up a project in the local backend's cached snapshot.
    pub(super) fn project(&self, id: ProjectId) -> Option<&crate::api::ProjectInfo> {
        self.local_view()
            .snapshot
            .projects
            .iter()
            .find(|p| p.id == id)
    }

    /// The local backend concretely, for the local-only affordances the trait
    /// deliberately omits (name-based attach, shell-toggle resolution, the
    /// commander session). `None` if backend 0 isn't a [`LocalBackend`] — never
    /// today, since the local backend is always present.
    pub(super) fn local_backend(&self) -> Option<&LocalBackend> {
        self.backends[LOCAL_BACKEND_ID.0]
            .backend
            .as_any()
            .downcast_ref::<LocalBackend>()
    }

    /// The backend that owns an [`AttachTarget`]: a session ref's own backend,
    /// or the local backend for a name-only target (the commander / a project
    /// shell, which are local-only affordances).
    pub(super) fn attach_target_backend(&self, target: &AttachTarget) -> BackendId {
        match target {
            AttachTarget::Session { session, .. } => session.backend,
            AttachTarget::LocalName(_) => LOCAL_BACKEND_ID,
        }
    }

    /// The tmux session name an [`AttachTarget`] resolves to, for the switcher
    /// popup, MRU/viewed tracking, and post-attach focus. `None` when a session
    /// ref isn't in the cached snapshot.
    pub(super) fn attach_target_name(&self, target: &AttachTarget) -> Option<String> {
        match target {
            AttachTarget::LocalName(name) => Some(name.clone()),
            AttachTarget::Session { session, kind } => {
                let base = self.session(*session)?.tmux_session_name.clone();
                Some(match kind {
                    AttachKind::Agent => base,
                    AttachKind::Shell => format!("{base}-sh"),
                })
            }
        }
    }

    /// Map a tmux session name back to a local [`AttachTarget`] — used after the
    /// in-session switcher lands on an arbitrary session by name. A `-sh`
    /// suffix maps to the session's shell pane; a name that matches no session
    /// (the commander, say) stays a [`AttachTarget::LocalName`].
    pub(super) fn attach_target_from_name(&self, name: &str) -> AttachTarget {
        let is_shell = name.ends_with("-sh");
        let base = name.strip_suffix("-sh").unwrap_or(name);
        match self
            .local_view()
            .snapshot
            .sessions
            .iter()
            .find(|s| s.tmux_session_name == base)
        {
            Some(s) => AttachTarget::Session {
                session: SessionRef::local(s.session_id),
                kind: if is_shell {
                    AttachKind::Shell
                } else {
                    AttachKind::Agent
                },
            },
            None => AttachTarget::LocalName(name.to_string()),
        }
    }

    /// The session id a tmux session name belongs to (agent or `-sh` shell),
    /// from the cached snapshot. `None` for the commander / an unknown name.
    pub(super) fn session_id_by_tmux_name(&self, name: &str) -> Option<SessionId> {
        let base = name.strip_suffix("-sh").unwrap_or(name);
        self.local_view()
            .snapshot
            .sessions
            .iter()
            .find(|s| s.tmux_session_name == base)
            .map(|s| s.session_id)
    }

    /// Open an attach connection for `target`: through the owning backend for a
    /// session pane (which stamps last-attached and revives a dead tmux
    /// session), or via the local name-based path for the commander / project
    /// shell.
    pub(super) async fn open_attach(
        &self,
        target: &AttachTarget,
        cols: u16,
        rows: u16,
    ) -> BResult<Box<dyn AttachConnection>> {
        match target {
            AttachTarget::Session { session, kind } => {
                self.backend(session.backend)
                    .ok_or(BackendError::NotFound)?
                    .backend
                    .attach(session.id, cols, rows, *kind)
                    .await
            }
            AttachTarget::LocalName(name) => {
                self.local_backend()
                    .ok_or(BackendError::NotFound)?
                    .attach_by_tmux_name(name, cols, rows)
                    .await
            }
        }
    }

    /// Resolve the Ctrl+\ shell-toggle partner for `current`. A session pane
    /// simply flips Agent↔Shell on the same id — `backend.attach(id, Shell)`
    /// creates the `-sh` pair on demand, so no name resolution is needed. A
    /// name-only target (a project shell, or a switcher-picked session) defers
    /// to the local backend's name-based resolver.
    pub(super) async fn toggle_shell_target(
        &self,
        current: &AttachTarget,
        current_name: &str,
    ) -> BResult<AttachTarget> {
        match current {
            AttachTarget::Session { session, kind } => Ok(AttachTarget::Session {
                session: *session,
                kind: match kind {
                    AttachKind::Agent => AttachKind::Shell,
                    AttachKind::Shell => AttachKind::Agent,
                },
            }),
            AttachTarget::LocalName(name) => {
                let be = self.local_backend().ok_or(BackendError::NotFound)?;
                // Prefer the live name reported by the attach (post-switcher).
                let base = if current_name.is_empty() {
                    name.as_str()
                } else {
                    current_name
                };
                let paired = be.resolve_shell_toggle_pair(base).await?;
                Ok(self.attach_target_from_name(&paired))
            }
        }
    }

    /// Fetch an initial snapshot + agent states for every backend, so the first
    /// list refresh reads real data rather than the empty `connecting` view.
    /// A backend that errors is left `Connecting` (its change-feed task will
    /// retry) and marked `Degraded` so its header shows the reason.
    async fn bootstrap_backend_views(&mut self) {
        for handle in &mut self.backends {
            let snapshot = handle.backend.workspace_snapshot().await;
            let states = handle.backend.agent_states(false).await;
            match (snapshot, states) {
                (Ok(snapshot), Ok(states)) => {
                    handle.view.snapshot = snapshot;
                    handle.view.agent_states = states;
                    handle.view.connection = crate::backend::ConnectionState::Connected;
                }
                (Err(e), _) | (_, Err(e)) => {
                    warn!("Initial snapshot for backend {:?} failed: {}", handle.id, e);
                    handle.view.connection = crate::backend::ConnectionState::Degraded {
                        reason: e.to_string(),
                    };
                }
            }
        }
    }

    /// Spawn one task per backend that awaits its change feed and, on each bump,
    /// fetches a fresh snapshot + agent states and forwards them as
    /// [`StateUpdate::BackendChanged`] over the event channel. The task exits
    /// when the feed's sender is dropped (backend gone).
    fn spawn_backend_change_feeds(&mut self) {
        for i in 0..self.backends.len() {
            let tasks = self.spawn_backend_feeds_for(&self.backends[i]);
            self.backends[i].feed_tasks = tasks;
        }
    }

    /// Spawn the change-feed and (if the backend exposes one) connection-watch
    /// tasks for a single backend handle, returning their [`JoinHandle`]s so the
    /// handle can own (and, on drop, abort) them. Used at startup for every
    /// backend and on config hot-reload for a newly-added remote backend.
    fn spawn_backend_feeds_for(&self, handle: &BackendHandle) -> Vec<tokio::task::JoinHandle<()>> {
        let mut tasks = Vec::new();

        // Connection-watch task: forward the backend's health changes into the
        // cached view so the server header re-renders live. Backends with a
        // fixed health (local, placeholder) return `None` and spawn nothing.
        if let Some(mut conn) = handle.backend.connection_watch() {
            let backend_id = handle.id.0;
            let tx = self.event_loop.sender();
            tasks.push(tokio::spawn(async move {
                loop {
                    let state = conn.borrow().clone();
                    if tx
                        .send(AppEvent::StateUpdate(StateUpdate::BackendConnection {
                            backend_id,
                            state,
                        }))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    if conn.changed().await.is_err() {
                        break;
                    }
                }
            }));
        }

        {
            let backend_id = handle.id.0;
            let backend = handle.backend.clone();
            let mut feed = backend.change_feed();
            let tx = self.event_loop.sender();
            tasks.push(tokio::spawn(async move {
                while feed.changed().await {
                    let snapshot = match backend.workspace_snapshot().await {
                        Ok(s) => s,
                        Err(e) => {
                            debug!("Change-feed snapshot for backend {backend_id} failed: {e}");
                            continue;
                        }
                    };
                    let states = backend.agent_states(false).await.unwrap_or_else(|_| {
                        crate::api::AgentStatesSnapshot {
                            states: Default::default(),
                            commander_running: false,
                        }
                    });
                    if tx
                        .send(AppEvent::StateUpdate(StateUpdate::BackendChanged {
                            backend_id,
                            snapshot: Box::new(snapshot),
                            states: Box::new(states),
                        }))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }

        tasks
    }

    /// Run the application
    pub async fn run(&mut self) -> Result<()> {
        // Check tmux is available
        self.service.check_tmux().await?;

        // One-time setup: reconcile each backend (drop stale Creating sessions,
        // reset transient stack states, sync status against live tmux, re-run
        // section assignment). The local backend runs it in-process; a remote
        // backend's server reconciles itself (default no-op).
        for handle in &self.backends {
            if let Err(e) = handle.backend.startup_reconcile().await {
                warn!(
                    "Startup reconciliation for backend {:?} failed: {}",
                    handle.id, e
                );
            }
        }

        // Populate each backend's cached view with an initial snapshot before
        // the first list refresh, then start a per-backend change-feed task so
        // subsequent store/remote changes fold in off the render path.
        self.bootstrap_backend_views().await;
        self.spawn_backend_change_feeds();

        // Cache gh availability for the enriched-PR info fetch (the PR-status
        // loop gates on its own cached probe inside the service).
        if self.config.pr_check_interval_secs > 0 {
            self.ui_state.gh_available = is_gh_available().await;
        }

        // Probe terminal graphics capability ONCE, here — BEFORE the background
        // input reader starts below. `from_query_stdio` writes DA/DSR escape
        // queries and reads the replies from stdin; if the reader were already
        // running it would steal those replies, time out, and (since ratatui
        // 0.30) crash the loop. It manages its own raw mode for the query. On
        // any failure (non-tty, unsupported terminal) we fall back to Unicode
        // half-blocks, which render on any truecolor terminal.
        let mut picker = ratatui_image::picker::Picker::from_query_stdio()
            .unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks());
        let term_program = std::env::var("TERM_PROGRAM").ok();
        let lc_terminal = std::env::var("LC_TERMINAL").ok();
        if let Some(proto) = iterm2_kitty_override(
            picker.protocol_type(),
            term_program.as_deref(),
            lc_terminal.as_deref(),
        ) {
            debug!(?proto, "iTerm2 detected: overriding Kitty probe result");
            picker.set_protocol_type(proto);
        }
        self.picker = Some(picker);

        // Floor at 1: a hand-edited config with fps 0 must not divide by zero.
        let tick_rate = Duration::from_millis(1000 / self.config.ui_refresh_fps.max(1) as u64);
        self.event_loop.start(tick_rate);

        // Warm the syntax-highlight assets in the background so the first review
        // open doesn't pay the one-time syntax-set load on its critical path.
        tokio::spawn(async {
            let _ = tokio::task::spawn_blocking(crate::tui::syntax_highlight::warm_assets).await;
        });

        // Start the service-owned background loops (agent-state polling,
        // PR-status checks, project auto-pull, cross-instance state-sync). They
        // run inside the service and reach the TUI as fresh snapshots via the
        // backend change feed; a remote backend's server runs its own loops, so
        // this local spawn is idempotent and no-ops if already started.
        let _background = self
            .service
            .spawn_background_tasks(crate::api::BackgroundOpts {
                commander_enabled: self.commander_enabled_at_init,
            });

        // Restore the last-selected view if the user has previously chosen
        // one. If they haven't, fall back to the section-aware default:
        // SectionGrouped when sections are configured, else ProjectGrouped.
        // Any section view falls back to ProjectGrouped at refresh time if
        // sections have since been removed from config.
        let persisted_view = self.tui_prefs.prefs().view_mode;
        self.ui_state.view_mode = match persisted_view {
            Some(view) => view,
            None if !self.config.sections.is_empty() => ViewMode::SectionGrouped,
            None => ViewMode::ProjectGrouped,
        };

        // Restore last selection from persisted state
        self.refresh_list_items().await;
        self.restore_selection().await;

        // Surface any pending review comments left from a previous run.
        self.refresh_comment_indicators().await;

        // Bring the mic listener up eagerly (when STT is enabled) so a global
        // hotkey can toggle recording even before the conversation is opened.
        // The heavy headless session stays lazy — the listener spawns it on the
        // first transcript. Then start the external IPC trigger(s).
        if self.config.stt.enabled {
            self.ensure_listener_started().await;
            self.spawn_listen_ipc();
        }

        loop {
            // Setup terminal for TUI
            let mut terminal = self.setup_terminal()?;
            self.refresh_list_items().await;

            // Run main loop until quit or attach
            info!("Entering main loop");
            let result = self.main_loop(&mut terminal).await;
            match &result {
                Ok(()) => info!("Main loop exited cleanly"),
                Err(e) => error!("Main loop exited with error: {e:?}"),
            }

            // Restore terminal before propagating, attaching, or exiting — a
            // raw-mode/alternate-screen terminal must never outlive the loop,
            // even on the error path below.
            info!("Restoring terminal");
            self.restore_terminal(&mut terminal)?;
            info!("Terminal restored successfully");

            // A main-loop error is fatal: propagate it so `main` exits
            // non-zero and color-eyre reports it on stderr, rather than
            // silently dropping into the quit path. Restore has already run.
            result?;

            // Reset should_quit for next iteration
            self.ui_state.should_quit = false;

            if let Some((editor, path)) = self.ui_state.editor_command.take() {
                // Run editor as a foreground process, then return to TUI
                self.event_loop.stop_input();
                tokio::time::sleep(Duration::from_millis(100)).await;

                info!("Launching editor: {} {}", editor, path.display());
                let status = std::process::Command::new(&editor).arg(&path).status();

                if let Err(e) = status {
                    self.ui_state.modal = Modal::Error {
                        message: format!("Failed to launch '{}': {}", editor, e),
                    };
                }

                self.event_loop.restart_input();
            } else {
                match self.ui_state.attach_request.take() {
                    Some(request) => {
                        // Stop the input reader BEFORE attaching so it doesn't
                        // compete for stdin, then flush the key that triggered
                        // this attach.
                        self.event_loop.stop_input();
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        crate::tmux::flush_stdin();

                        // Pre-warm the conversation runtime so voice input
                        // (Alt-V) works immediately while attached. Idempotent.
                        if self.config.stt.enabled {
                            self.ensure_conversation_started().await;
                        }

                        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                        // Track every session viewed during this attach (via the
                        // switcher or shell toggle) so we can refresh just their
                        // agent state on the way out — see the post-loop block.
                        let mut viewed: HashSet<String> = HashSet::new();
                        let mut current = request;
                        let mut consecutive_ends: u8 = 0;
                        // The tmux name the user ends on (possibly reached via
                        // the in-session switcher), for post-attach focus.
                        let mut final_name: Option<String> = self.attach_target_name(&current);

                        loop {
                            let name = self.attach_target_name(&current);
                            if let Some(n) = &name {
                                viewed.insert(n.clone());
                            }

                            // The in-session switcher is a local capability; a
                            // remote backend forwards Ctrl+Space to the pane
                            // instead. Gate on the *attached* session's backend,
                            // re-evaluated each hop (the switcher/shell-toggle can
                            // move `current` between backends).
                            let switcher_enabled = self
                                .backend(self.attach_target_backend(&current))
                                .map(|h| h.backend.capabilities().switcher_popup)
                                .unwrap_or(false);

                            // Open the connection first (the backend revives a
                            // dead tmux session and stamps last-attached); a
                            // failure surfaces as an error modal.
                            let conn = match self.open_attach(&current, cols, rows).await {
                                Ok(c) => c,
                                Err(e) => {
                                    warn!("Failed to attach: {e}");
                                    self.ui_state.modal = Modal::Error {
                                        message: format!("Failed to attach: {e}"),
                                    };
                                    break;
                                }
                            };
                            let streams = conn.split();

                            // Only intercept Ctrl+Z / Alt-r / Alt-V for Claude
                            // (non-shell) panes: SIGTSTP would freeze a shell-
                            // less pane, and a shell's Ctrl-r must not be
                            // shadowed. Voice is meaningful only with STT on.
                            let is_shell = matches!(
                                current,
                                AttachTarget::Session {
                                    kind: AttachKind::Shell,
                                    ..
                                }
                            ) || name.as_deref().is_some_and(|n| n.ends_with("-sh"));
                            let intercept_ctrl_z = !is_shell;
                            let editor_triggers = crate::config::keybindings::editor_trigger_bytes(
                                &self.config.keybindings,
                            );
                            let review_triggers = if intercept_ctrl_z {
                                crate::config::keybindings::review_trigger_bytes(
                                    &self.config.keybindings,
                                )
                            } else {
                                Vec::new()
                            };
                            let (voice_triggers, voice_listener) =
                                if intercept_ctrl_z && self.config.stt.enabled {
                                    (
                                        crate::config::keybindings::voice_trigger_bytes(
                                            &self.config.keybindings,
                                        ),
                                        self.conversation.listener.clone(),
                                    )
                                } else {
                                    (Vec::new(), None)
                                };

                            let cfg = crate::tmux::AttachConfig {
                                editor_triggers,
                                review_triggers,
                                voice_triggers,
                                voice_listener,
                                recording: self.conversation.recording.clone(),
                                intercept_ctrl_z,
                                switcher_enabled,
                                session_name: name.clone(),
                            };

                            let outcome = match crate::tmux::run_attach(streams, cfg).await {
                                Ok(o) => o,
                                Err(e) => {
                                    warn!("Attach loop failed: {e}");
                                    self.ui_state.modal = Modal::Error {
                                        message: format!("Failed to attach: {e}"),
                                    };
                                    break;
                                }
                            };

                            // The switcher may have run `tmux switch-client`
                            // mid-attach, landing on a different session than we
                            // entered. Trust the reported final session and
                            // re-map it to a target.
                            let landed = outcome.final_session.clone();
                            if !landed.is_empty() {
                                viewed.insert(landed.clone());
                                if name.as_deref() != Some(landed.as_str()) {
                                    current = self.attach_target_from_name(&landed);
                                }
                                final_name = Some(landed.clone());
                            } else {
                                final_name = self.attach_target_name(&current);
                            }

                            match outcome.result {
                                crate::tmux::AttachResult::SwitchToShell => {
                                    current =
                                        match self.toggle_shell_target(&current, &landed).await {
                                            Ok(t) => t,
                                            Err(e) => {
                                                warn!("Failed to resolve shell session: {e}");
                                                self.ui_state.modal = Modal::Error {
                                                    message: format!("Cannot switch to shell: {e}"),
                                                };
                                                break;
                                            }
                                        };
                                    crate::tmux::flush_stdin();
                                    consecutive_ends = 0;
                                    continue;
                                }
                                crate::tmux::AttachResult::SwitchToReview => {
                                    // Queue the review view; opened below once
                                    // we're back in the TUI.
                                    self.ui_state.pending_open_review =
                                        self.session_id_by_tmux_name(&landed);
                                    break;
                                }
                                crate::tmux::AttachResult::OpenEditor => {
                                    // Launch the operator's editor on the session
                                    // worktree, then re-attach. Only meaningful for
                                    // a backend that can drive the local editor
                                    // (there's no local worktree for a remote one).
                                    let can_edit = self
                                        .backend(self.attach_target_backend(&current))
                                        .is_some_and(|h| h.backend.capabilities().open_editor);
                                    if can_edit {
                                        self.open_editor_for_tmux_session(&landed).await;
                                    }
                                    crate::tmux::flush_stdin();
                                    consecutive_ends = 0;
                                    continue;
                                }
                                crate::tmux::AttachResult::SessionEnded => {
                                    let auto = self.attach_target_name(&current).is_some_and(|n| {
                                        should_auto_restart_ended(&n, consecutive_ends)
                                    });
                                    if let (true, AttachTarget::Session { session, .. }) =
                                        (auto, &current)
                                    {
                                        consecutive_ends += 1;
                                        match self
                                            .backend_arc(session.backend)
                                            .restart_session_fresh(session.id)
                                            .await
                                        {
                                            Ok(()) => {
                                                info!(
                                                    "Auto-restarted session fresh (attempt {consecutive_ends})"
                                                );
                                                crate::tmux::flush_stdin();
                                                continue;
                                            }
                                            Err(e) => {
                                                warn!("Failed to auto-restart session: {e}");
                                                break;
                                            }
                                        }
                                    } else {
                                        break;
                                    }
                                }
                                other => {
                                    info!("Attach ended: {other:?}");
                                    break;
                                }
                            }
                        }

                        // Flush stdin again after detach to discard stale input,
                        // then restart the input reader (also draining any
                        // AgentStatesUpdated queued while attached).
                        crate::tmux::flush_stdin();
                        info!("Returned from attach, restarting input reader");
                        self.event_loop.restart_input();

                        // Refresh agent state for just the sessions we viewed, via
                        // the *attached* session's backend, applying the fresh
                        // states directly. We do NOT clear the whole map: that
                        // would blank every spinner until the next poll.
                        // `agent_states(true)` also advances the service loop's
                        // shared baseline to these observed states, so the loop
                        // won't re-flag a just-finished session on its next tick.
                        let attached_backend = self.attach_target_backend(&current);
                        let viewed_ids: HashSet<SessionId> = self
                            .view_for(attached_backend)
                            .snapshot
                            .sessions
                            .iter()
                            .filter(|s| {
                                viewed.iter().any(|n| {
                                    s.tmux_session_name == n.strip_suffix("-sh").unwrap_or(n)
                                })
                            })
                            .map(|s| s.session_id)
                            .collect();
                        if !viewed_ids.is_empty() {
                            // The service loop runs during the attach and may have
                            // flagged a watched session unread when it went idle.
                            // Clear unread for everything we actually saw — the
                            // operator watched those turns finish.
                            let backend = self.backend_arc(attached_backend);
                            for id in &viewed_ids {
                                let _ = backend.mark_read(*id).await;
                            }
                            if let Ok(fresh) = backend.agent_states(true).await {
                                let refreshed: HashMap<SessionId, AgentState> = fresh
                                    .states
                                    .into_iter()
                                    .filter(|(id, _)| viewed_ids.contains(id))
                                    .collect();
                                // Fold into the attached backend's cached view —
                                // the tree reads agent state per-backend from there.
                                if let Some(handle) =
                                    self.backends.iter_mut().find(|h| h.id == attached_backend)
                                {
                                    state::apply_viewed_session_refresh(
                                        &mut handle.view.agent_states.states,
                                        refreshed.clone(),
                                    );
                                }
                                // The local rendered map also feeds local-only
                                // consumers (commander chip, review-transition
                                // detection), so keep it in sync for a local attach.
                                if attached_backend == LOCAL_BACKEND_ID {
                                    state::apply_viewed_session_refresh(
                                        &mut self.ui_state.agent_states,
                                        refreshed,
                                    );
                                }
                            }
                            self.refresh_list_items().await;
                        }

                        // Focus the session the user just left so the tree lands
                        // on it (important after the in-session switcher).
                        if let Some(name) = final_name {
                            self.focus_session_in_tree(&name).await;
                        }

                        // Alt-r inside the attached session queued its review
                        // diff — open it now so the next frame shows it.
                        if let Some(sid) = self.ui_state.pending_open_review.take() {
                            let backend = self.backend_of_session(sid);
                            self.ui_state.selected_session_id = Some(SessionRef::new(backend, sid));
                            self.handle_open_review().await;
                        }
                    }
                    None => {
                        // Save selection before quitting
                        self.save_selection().await;
                        // Flush any queued telemetry before exit so the last
                        // session's events aren't lost to the flush interval.
                        self.local_arc().flush_telemetry().await;
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    /// Setup terminal for TUI mode
    fn setup_terminal(&self) -> Result<Terminal<CrosstermBackend<Stdout>>> {
        enable_raw_mode().map_err(|e| TuiError::InitFailed(e.to_string()))?;

        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableMouseCapture
        )
        .map_err(|e| TuiError::InitFailed(e.to_string()))?;

        // Ask the terminal for unambiguous key events (kitty keyboard
        // protocol). Lets us distinguish Shift+Space from plain Space so
        // the command palette shortcut can work. Terminals that don't
        // support the protocol silently ignore the CSI sequence, in which
        // case Shift+Space falls back to a plain-Space event (opens the
        // unified palette) and the user can still reach command-only mode
        // via the `>` prefix.
        let _ = execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES,)
        );

        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).map_err(|e| TuiError::InitFailed(e.to_string()))?;

        Ok(terminal)
    }

    /// Restore terminal to normal state
    fn restore_terminal(&self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        info!("Disabling raw mode");
        disable_raw_mode().map_err(|e| TuiError::RestoreFailed(e.to_string()))?;

        // Pop the keyboard enhancement flags we pushed on setup. Best-effort —
        // on terminals that ignored the push this is a no-op.
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);

        info!("Leaving alternate screen");
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableBracketedPaste,
            DisableMouseCapture
        )
        .map_err(|e| TuiError::RestoreFailed(e.to_string()))?;

        info!("Showing cursor");
        terminal
            .show_cursor()
            .map_err(|e| TuiError::RestoreFailed(e.to_string()))?;

        info!("Terminal restore complete");
        Ok(())
    }
}

/// The result of diffing the configured `remote_servers` on a hot-reload,
/// keyed by server *name* (the stable identity — `BackendId` is positional and
/// config order can change). A server whose `url`/`token` changed appears in
/// both `removed` and `added`, so it is torn down and rebuilt.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct RemoteServersReconcile {
    /// Servers to construct (new, or changed since the old config).
    pub added: Vec<crate::config::RemoteServerConfig>,
    /// Names of servers to tear down (gone, or changed since the old config).
    pub removed: Vec<String>,
}

/// Pure diff of two `remote_servers` lists → the add/remove decisions the TUI
/// applies to its live `Vec<BackendHandle>`. Matching is by name; a name in
/// both lists with an unchanged config is a no-op, while a changed config is a
/// remove-then-add. Order differences alone produce no add/remove (the caller
/// reorders separately).
pub(super) fn reconcile_remote_servers(
    old: &[crate::config::RemoteServerConfig],
    new: &[crate::config::RemoteServerConfig],
) -> RemoteServersReconcile {
    let mut removed = Vec::new();
    for o in old {
        match new.iter().find(|n| n.name == o.name) {
            Some(n) if n == o => {}            // unchanged: keep
            _ => removed.push(o.name.clone()), // gone or changed
        }
    }
    let mut added = Vec::new();
    for n in new {
        match old.iter().find(|o| o.name == n.name) {
            Some(o) if o == n => {}     // unchanged: keep
            _ => added.push(n.clone()), // new or changed
        }
    }
    RemoteServersReconcile { added, removed }
}
