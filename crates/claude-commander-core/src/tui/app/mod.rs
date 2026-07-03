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
use crate::config::{BindableAction, Config, ConfigStore, StateStore};
use crate::error::{Result, TuiError};
use crate::git::{
    AiSummary, BlockReason, DiffInfo, EnrichedPrInfo, PrCheckResult, PullOutcome,
    check_pr_for_branch, diff_hash, fetch_branch_summary, fetch_enriched_pr, is_gh_available,
    run_project_pull,
};
use crate::session::{
    AgentState, ProjectId, SessionId, SessionListItem, SessionManager, SessionStatus,
    WorktreeSession,
};
use crate::tmux::AgentStateDetector;

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

/// Whether the agent-state poll tick can skip entirely: there is nothing to
/// detect (no regular sessions and the commander isn't running) AND the
/// commander's running state has not changed since the last emitted update.
/// Skipping keeps the no-sessions path quiet — no event, no list rebuild.
///
/// The `!commander_running` term is what the docstring promises: a *running*
/// commander always has agent state worth forwarding, so we never skip then.
/// At the current call site `sessions_empty` already implies `!commander_running`
/// (a running commander pushes its sentinel), so the term is belt-and-braces —
/// but it keeps the predicate honest if the call site ever changes.
fn poll_tick_can_skip(
    sessions_empty: bool,
    commander_running: bool,
    last_commander_running: bool,
) -> bool {
    sessions_empty && !commander_running && !last_commander_running
}

/// Whether a poll tick (that wasn't skipped) should emit an update: when there
/// are fresh agent states, or the commander's running state flipped — the
/// latter is what lets the chip turn *off* on the trailing edge.
fn poll_tick_should_send(
    states_empty: bool,
    commander_running: bool,
    last_commander_running: bool,
) -> bool {
    !states_empty || commander_running != last_commander_running
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
    /// Currently selected session (for preview/diff)
    pub selected_session_id: Option<SessionId>,
    /// Currently selected project
    pub selected_project_id: Option<ProjectId>,
    /// Whether the `cc-commander` tmux session is currently running. Cached from
    /// the background agent-state poll so the (sync) renderers — the footer chip
    /// — can read it without awaiting tmux.
    pub commander_running: bool,
    /// Attach command to run after exiting TUI
    pub attach_command: Option<String>,
    /// Session whose review diff should be opened on returning to the TUI —
    /// set when the user pressed Alt-r inside an attached session.
    pub pending_open_review: Option<SessionId>,
    /// Editor command + path to open after exiting TUI
    pub editor_command: Option<(String, PathBuf)>,
    /// When attached via shell toggle (Ctrl+\), stores the session name to switch back to.
    /// Contains (current_session_name, paired_session_name) so we can toggle between them.
    pub shell_toggle_pair: Option<(String, String)>,
    /// Needs right pane clear (set on view switch, consumed on render)
    pub clear_right_pane: bool,
    /// Left pane width as a percentage (adjustable at runtime via < / >)
    pub left_pane_pct: u16,
    /// When the last PR status check was performed
    pub last_pr_check: Option<Instant>,
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
    /// When each project last completed a background pull attempt
    /// (success or block). Drives the per-project interval scheduler.
    pub last_project_pull: HashMap<ProjectId, Instant>,
    /// Projects whose most recent pull was held back, with the reason.
    /// Cleared when a subsequent attempt advances or finds nothing to do.
    pub project_pull_blocked: HashMap<ProjectId, BlockReason>,
    /// Projects with a pull task currently in flight, so we don't double-spawn.
    pub project_pull_in_flight: std::collections::HashSet<ProjectId>,
    /// When the app launched. Used to give the background pull task a short
    /// grace period before its first fire after startup.
    pub started_at: Instant,
    /// When the project-pull scheduler last swept the project list. A cheap
    /// global throttle so the per-tick check doesn't acquire the state lock
    /// and clone the project list on every render frame.
    pub last_project_pull_sweep: Option<Instant>,
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
            commander_running: false,
            attach_command: None,
            pending_open_review: None,
            editor_command: None,
            shell_toggle_pair: None,
            clear_right_pane: false,
            left_pane_pct: DEFAULT_LEFT_PANE_PCT,
            last_pr_check: None,
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
            last_project_pull: HashMap::new(),
            project_pull_blocked: HashMap::new(),
            project_pull_in_flight: std::collections::HashSet::new(),
            started_at: Instant::now(),
            last_project_pull_sweep: None,
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
        let has_session = self.selected_session_id.is_some();
        let has_project = self.selected_project_id.is_some();
        match action {
            // Session-scoped actions require a selected session
            BindableAction::Select
            | BindableAction::SelectShell
            | BindableAction::DeleteSession
            | BindableAction::RenameSession
            | BindableAction::RestartSession
            | BindableAction::ToggleKeepAlive
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
    /// Unified service layer — owns SessionManager, StateStore, and ConfigStore
    service: CommanderService,
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
    ) -> Self {
        let config = config_store.read().clone();
        let service = CommanderService::new(config_store, store, frontend);

        let base = config
            .theme
            .preset
            .as_deref()
            .and_then(Theme::from_preset)
            .unwrap_or_default();
        let theme = base.with_overrides(&config.theme);
        let debounce = Duration::from_millis(config.session_number_debounce_ms);
        let commander_enabled_at_init = config.commander_enabled;

        Self {
            config,
            commander_enabled_at_init,
            service,
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

    /// Run the application
    pub async fn run(&mut self) -> Result<()> {
        // Check tmux is available
        self.service.check_tmux().await?;

        // One-time setup
        self.cleanup_stale_creating_sessions().await;
        self.cleanup_stale_merging_sessions().await;
        self.sync_session_states().await;
        self.reconcile_section_assignments().await;

        // Start the idle-hibernation loop now we're in the long-lived TUI
        // runtime (no-op unless enabled in config). Deliberately not started in
        // CommanderService::new so one-shot CLI commands never trigger it.
        self.service.start_hibernation_loop();

        // Check gh availability and do initial PR check
        if self.config.pr_check_interval_secs > 0 {
            self.ui_state.gh_available = is_gh_available().await;
            if self.ui_state.gh_available {
                self.spawn_pr_status_check();
            }
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

        // Start background state sync for cross-instance changes
        if self.config.state_sync_interval_ms > 0 {
            let store = self.service.store().clone();
            let tx = self.event_loop.sender();
            let interval_ms = self.config.state_sync_interval_ms;
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
                loop {
                    interval.tick().await;
                    match store.reload_if_changed().await {
                        Ok(true) => {
                            let _ = tx
                                .send(AppEvent::StateUpdate(StateUpdate::ExternalChange))
                                .await;
                        }
                        Ok(false) => {}
                        Err(e) => {
                            debug!("State sync check failed: {}", e);
                        }
                    }
                }
            });
        }

        // Start background agent state polling
        if self.config.agent_state_poll_interval_ms > 0 {
            let store = self.service.store().clone();
            let tx = self.event_loop.sender();
            let interval_ms = self.config.agent_state_poll_interval_ms;
            let tmux = self.service.session_manager().tmux.clone();
            // The commander is project-less and absent from `state.sessions`, so
            // it is polled separately. Enablement is restart-required: the poll
            // task and the footer chip share `commander_enabled_at_init` so the
            // chip can't disagree when the live config is toggled.
            let commander_enabled = self.commander_enabled_at_init;
            let commander_program = self.config.commander_program();
            let commander_tmux = tmux.clone();
            tokio::spawn(async move {
                let cache_ttl = Duration::from_millis(interval_ms.saturating_sub(500).max(500));
                let mut detector = AgentStateDetector::new(tmux, cache_ttl);
                let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
                let mut last_commander_running = false;
                loop {
                    interval.tick().await;
                    let mut sessions: Vec<(SessionId, String, String)> = {
                        let state = store.read().await;
                        state
                            .sessions
                            .values()
                            .filter(|s| s.status == SessionStatus::Running)
                            .map(|s| (s.id, s.tmux_session_name.clone(), s.program.clone()))
                            .collect()
                    };
                    let commander_running =
                        commander_enabled && crate::commander::is_running(&commander_tmux).await;
                    if commander_running {
                        sessions.push((
                            crate::commander::commander_sentinel_id(),
                            crate::commander::COMMANDER_TMUX_NAME.to_string(),
                            commander_program.clone(),
                        ));
                    }
                    // Quiet path: nothing to detect and the commander's running
                    // state is unchanged — skip the tick (no list rebuild).
                    if poll_tick_can_skip(
                        sessions.is_empty(),
                        commander_running,
                        last_commander_running,
                    ) {
                        continue;
                    }
                    let states: HashMap<SessionId, AgentState> = if sessions.is_empty() {
                        HashMap::new()
                    } else {
                        detector.detect_all(&sessions).await
                    };
                    // Send on any real change: fresh states, or the commander
                    // flipped (so its chip can turn on *and* off).
                    if poll_tick_should_send(
                        states.is_empty(),
                        commander_running,
                        last_commander_running,
                    ) {
                        last_commander_running = commander_running;
                        let _ = tx
                            .send(AppEvent::StateUpdate(StateUpdate::AgentStatesUpdated {
                                states,
                                commander_running,
                            }))
                            .await;
                    }
                }
            });
        }

        // Restore the last-selected view if the user has previously chosen
        // one. If they haven't, fall back to the section-aware default:
        // SectionGrouped when sections are configured, else ProjectGrouped.
        // Any section view falls back to ProjectGrouped at refresh time if
        // sections have since been removed from config.
        let persisted_view = self.service.store().read().await.view_mode;
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
                match self.ui_state.attach_command.take() {
                    Some(cmd) => {
                        // Stop the input reader BEFORE attaching so it doesn't compete for stdin
                        self.event_loop.stop_input();
                        // Brief delay to let the reader task actually stop
                        tokio::time::sleep(Duration::from_millis(100)).await;

                        // Flush any pending input (e.g. the Enter key that triggered this attach)
                        crate::tmux::flush_stdin();

                        // Attach to session via async PTY bridge (supports Ctrl+Q detach, Ctrl+\ shell toggle)
                        info!("Executing attach command: {}", cmd);
                        let session_name = cmd.split_whitespace().last().unwrap_or("").to_string();
                        // Track every session viewed during this attach
                        // (including ones reached via the in-tmux switcher or
                        // shell toggle) so we can refresh just their agent
                        // state on the way out — see the post-loop block.
                        let mut viewed_sessions: HashSet<String> = HashSet::new();
                        // The session the user ends up on when the attach loop
                        // exits — possibly reached via the in-session switcher,
                        // so not necessarily the one they entered with.
                        let mut final_session: Option<String> = None;
                        if !session_name.is_empty() {
                            // Pre-warm the conversation runtime so voice input
                            // (Alt-V) works immediately while attached — the mic
                            // listener only exists once the conversation has been
                            // started. Idempotent; no-op after the first attach.
                            if self.config.stt.enabled {
                                self.ensure_conversation_started().await;
                            }
                            let mut current_session = session_name.clone();
                            let mut consecutive_ends: u8 = 0;

                            loop {
                                viewed_sessions.insert(current_session.clone());

                                let editor_triggers =
                                    crate::config::keybindings::editor_trigger_bytes(
                                        &self.config.keybindings,
                                    );
                                // Shell sessions are named with a trailing
                                // "-sh" (see resolve_shell_toggle_pair). Only
                                // intercept Ctrl+Z for non-shell (Claude)
                                // sessions, where SIGTSTP would freeze the
                                // pane with no shell to recover from.
                                let intercept_ctrl_z = !current_session.ends_with("-sh");
                                // The Alt-r review toggle is intercepted only
                                // for Claude sessions. (Alt-r replaced Ctrl-r
                                // precisely so a shell's Ctrl-r reverse-history-
                                // search is never shadowed.)
                                let review_triggers = if intercept_ctrl_z {
                                    crate::config::keybindings::review_trigger_bytes(
                                        &self.config.keybindings,
                                    )
                                } else {
                                    Vec::new()
                                };
                                // Voice input (Alt-V) is toggled in-place mid-
                                // attach; only meaningful for Claude sessions
                                // with STT enabled and a listener running.
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

                                // Stamp last_attached_at so the in-tmux
                                // switcher can sort Alt+Tab-style by MRU.
                                let to_stamp = current_session.clone();
                                if let Err(e) = self
                                    .service
                                    .store()
                                    .mutate(move |state| {
                                        if let Some(session) = state
                                            .sessions
                                            .values_mut()
                                            .find(|s| s.matches_tmux_name(&to_stamp))
                                        {
                                            session.mark_attached();
                                        }
                                    })
                                    .await
                                {
                                    warn!("Failed to stamp last_attached_at: {}", e);
                                }

                                let outcome = match crate::tmux::attach_to_session(
                                    &current_session,
                                    editor_triggers,
                                    review_triggers,
                                    voice_triggers,
                                    voice_listener,
                                    self.conversation.recording.clone(),
                                    intercept_ctrl_z,
                                )
                                .await
                                {
                                    Ok(o) => o,
                                    Err(e) => {
                                        warn!("Failed to attach to session: {}", e);
                                        self.ui_state.modal = Modal::Error {
                                            message: format!("Failed to attach: {}", e),
                                        };
                                        self.ui_state.shell_toggle_pair = None;
                                        break;
                                    }
                                };

                                // `conversation.recording` is shared (`Arc`) with
                                // the attach loop, so a mid-attach Alt-V toggle is
                                // already reflected — no readback needed.

                                // The in-session switcher may have run `tmux switch-client`
                                // mid-attach, so the session we exited from isn't
                                // necessarily the one we entered with. Trust the outcome.
                                let switched_via_popup = outcome.final_session != current_session;
                                current_session = outcome.final_session;
                                viewed_sessions.insert(current_session.clone());
                                if switched_via_popup {
                                    // Picking a new session in the popup invalidates the
                                    // shell-toggle pair (which is tied to a specific
                                    // Claude/shell duo).
                                    self.ui_state.shell_toggle_pair = None;
                                }

                                match outcome.result {
                                    crate::tmux::AttachResult::SwitchToShell => {
                                        info!(
                                            "Shell toggle requested from session: {}",
                                            current_session
                                        );

                                        // Determine the paired session to switch to
                                        let next_session = match &self.ui_state.shell_toggle_pair {
                                            Some((_, paired)) => paired.clone(),
                                            None => {
                                                // First toggle — resolve the shell session
                                                match self
                                                    .resolve_shell_toggle_pair(&current_session)
                                                    .await
                                                {
                                                    Ok(paired) => paired,
                                                    Err(e) => {
                                                        warn!(
                                                            "Failed to resolve shell session: {}",
                                                            e
                                                        );
                                                        self.ui_state.modal = Modal::Error {
                                                            message: format!(
                                                                "Cannot switch to shell: {}",
                                                                e
                                                            ),
                                                        };
                                                        break;
                                                    }
                                                }
                                            }
                                        };

                                        // Update the toggle pair so next Ctrl+\ switches back
                                        self.ui_state.shell_toggle_pair =
                                            Some((next_session.clone(), current_session.clone()));
                                        current_session = next_session;

                                        // Flush between switches
                                        crate::tmux::flush_stdin();
                                        consecutive_ends = 0;
                                        info!("Switching to session: {}", current_session);
                                        continue;
                                    }
                                    crate::tmux::AttachResult::SwitchToReview => {
                                        info!(
                                            "Review toggle requested from session: {}",
                                            current_session
                                        );
                                        // Resolve the tmux session to its id and
                                        // queue the review view; the post-loop
                                        // code opens it once we're back in the
                                        // TUI. Alt-r inside the review
                                        // re-attaches (see handle_review_key).
                                        self.ui_state.pending_open_review = {
                                            let st = self.service.store().read().await;
                                            st.sessions
                                                .values()
                                                .find(|s| s.matches_tmux_name(&current_session))
                                                .map(|s| s.id)
                                        };
                                        self.ui_state.shell_toggle_pair = None;
                                        break;
                                    }
                                    crate::tmux::AttachResult::OpenEditor => {
                                        info!(
                                            "OpenEditor requested from session: {}",
                                            current_session
                                        );
                                        // Run the editor for the session's worktree, keep
                                        // the tmux session alive, and then re-attach.
                                        self.open_editor_for_tmux_session(&current_session).await;
                                        crate::tmux::flush_stdin();
                                        consecutive_ends = 0;
                                        continue;
                                    }
                                    crate::tmux::AttachResult::SessionEnded => {
                                        info!("Session ended, attempting fresh restart");
                                        if should_auto_restart_ended(
                                            &current_session,
                                            consecutive_ends,
                                        ) {
                                            consecutive_ends += 1;
                                            match self
                                                .service
                                                .session_manager()
                                                .restart_session_fresh_by_tmux_name(
                                                    &current_session,
                                                )
                                                .await
                                            {
                                                Ok(()) => {
                                                    info!(
                                                        "Auto-restarted session fresh (attempt {})",
                                                        consecutive_ends
                                                    );
                                                    crate::tmux::flush_stdin();
                                                    continue;
                                                }
                                                Err(e) => {
                                                    warn!("Failed to auto-restart session: {}", e);
                                                    self.ui_state.shell_toggle_pair = None;
                                                    break;
                                                }
                                            }
                                        } else {
                                            if consecutive_ends >= 3 {
                                                warn!(
                                                    "Session ended {} consecutive times, \
                                                     giving up",
                                                    consecutive_ends
                                                );
                                            }
                                            self.ui_state.shell_toggle_pair = None;
                                            break;
                                        }
                                    }
                                    result => {
                                        info!("Attach ended: {:?}", result);
                                        self.ui_state.shell_toggle_pair = None;
                                        break;
                                    }
                                }
                            }
                            final_session = Some(current_session);
                        }

                        // Flush stdin again after detach to discard any stale input
                        crate::tmux::flush_stdin();

                        // Restart the input reader after detach. This also
                        // drains the event channel, discarding any
                        // AgentStatesUpdated events queued while attached.
                        info!("Returned from attach, restarting input reader");
                        self.event_loop.restart_input();

                        // Refresh agent state for just the sessions we viewed,
                        // setting their freshly-observed state directly. We
                        // deliberately do NOT clear the whole map: clearing
                        // blanks every spinner in the tree until the next poll
                        // (~3s) and wipes the unread baseline for background
                        // sessions, silently dropping genuine Working→Idle
                        // notifications that occurred while we were attached.
                        // Setting the viewed sessions directly (bypassing the
                        // unread diff) avoids re-flagging them as unread, since
                        // the user was watching them.
                        if !viewed_sessions.is_empty() {
                            let targets: Vec<(SessionId, String, String)> = {
                                let store_state = self.service.store().read().await;
                                store_state
                                    .sessions
                                    .values()
                                    .filter(|s| s.status == SessionStatus::Running)
                                    .filter(|s| {
                                        viewed_sessions.iter().any(|name| s.matches_tmux_name(name))
                                    })
                                    .map(|s| (s.id, s.tmux_session_name.clone(), s.program.clone()))
                                    .collect()
                            };
                            if !targets.is_empty() {
                                let mut detector = AgentStateDetector::new(
                                    self.service.session_manager().tmux.clone(),
                                    Duration::from_millis(0),
                                );
                                let refreshed = detector.detect_all(&targets).await;
                                state::apply_viewed_session_refresh(
                                    &mut self.ui_state.agent_states,
                                    refreshed,
                                );
                                self.refresh_list_items().await;
                            }
                        }

                        // Focus the session the user just left so the tree
                        // lands on it — important after the in-session switcher,
                        // which may have moved them to a different session than
                        // the one they entered.
                        if let Some(name) = final_session {
                            self.focus_session_in_tree(&name).await;
                        }

                        // Alt-r inside the attached session queued its review
                        // diff — open it now so the next TUI frame shows it
                        // (rather than the session list).
                        if let Some(sid) = self.ui_state.pending_open_review.take() {
                            self.ui_state.selected_session_id = Some(sid);
                            self.handle_open_review().await;
                        }
                    }
                    None => {
                        // Save selection before quitting
                        self.save_selection().await;
                        // Flush any queued telemetry before exit so the last
                        // session's events aren't lost to the flush interval.
                        self.service.telemetry().flush().await;
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
