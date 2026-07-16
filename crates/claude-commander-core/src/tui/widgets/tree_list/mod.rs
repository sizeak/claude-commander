//! Hierarchical tree list widget
//!
//! Displays projects and their worktree sessions in an indented list.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, ListItem},
};

use std::collections::{HashMap, HashSet};

use crate::session::{AgentState, ProjectId, SessionId, SessionListItem, SessionStatus};
use crate::tui::theme::Theme;

pub(crate) mod pr_colors;
mod render;
mod state;

#[cfg(test)]
mod tests;

pub use state::TreeListState;

/// Width of the right-aligned session-number field. The rendered prefix is
/// `"{n:>NUMBER_WIDTH$} "` — so the number occupies NUMBER_WIDTH columns and
/// is followed by a single trailing space, giving a 7-column slot.
const NUMBER_WIDTH: usize = 6;
/// Extra indent prepended for stacked-child worktrees (3 display columns),
/// so they sit one level deeper than the stack base they sit under.
const STACK_INDENT: &str = "   ";

/// Braille spinner frames for the Creating status indicator
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Marker shown on a session row that has pending review comments. Matches the
/// review view's comment marker.
const COMMENT_MARKER: char = '*';

/// Marker shown on a session row the user has kept alive (opted out of
/// auto-hibernation) — an anchor: the session stays put and won't hibernate.
const KEEP_ALIVE_MARKER: char = '⚓';

/// Tree list widget for displaying hierarchical sessions
pub struct TreeList<'a> {
    /// Items to display
    items: &'a [SessionListItem],
    /// Theme for styling
    theme: &'a Theme,
    /// Block for borders and title
    block: Option<Block<'a>>,
    /// Style for selected item
    highlight_style: Style,
    /// Tick counter for spinner animation
    tick: u64,
    /// Label names that mark an open PR as awaiting reviewer action.
    review_labels: &'a [String],
    /// When true, render PR labels as colored text on default bg (pre-pill
    /// behavior). When false (default), render as a colored pill block.
    invert_pr_label_color: bool,
    /// When true (default), show the running program as a `(program)`
    /// suffix on session rows when sessions use more than one program.
    show_session_program: bool,
    /// Forces the `(program)` suffix decision instead of deriving it from this
    /// widget's own items. The pinned recents panel renders only its slice, so
    /// it can't see whether the *real* rows use mixed programs; the caller
    /// computes that over the full list and passes it here so recents rows
    /// mirror the real rows' suffix exactly. `None` = derive locally (default).
    show_program_override: Option<bool>,
    /// Projects whose most recent auto-pull was held back, with a short
    /// reason string. A small ⚠ badge is rendered on each blocked row.
    pull_blocked_projects: HashMap<ProjectId, &'a str>,
    /// Sessions with at least one pending review comment. A `*` marker is
    /// rendered on each matching session row.
    comment_sessions: HashSet<SessionId>,
    /// Number + session colour for each worktree, precomputed over the full
    /// list. Recent-session rows look their values up here so they match the
    /// real row's number/colour even though the recents panel renders only its
    /// own slice. Empty for lists with no recents panel.
    recent_display_info: HashMap<SessionId, (usize, Color)>,
}

impl<'a> TreeList<'a> {
    /// Create a new tree list
    pub fn new(items: &'a [SessionListItem], theme: &'a Theme) -> Self {
        Self {
            items,
            theme,
            block: None,
            highlight_style: theme.selection().add_modifier(Modifier::BOLD),
            tick: 0,
            review_labels: &[],
            invert_pr_label_color: false,
            show_session_program: true,
            show_program_override: None,
            pull_blocked_projects: HashMap::new(),
            comment_sessions: HashSet::new(),
            recent_display_info: HashMap::new(),
        }
    }

    /// Mark a set of sessions as having pending review comments. Renders a `*`
    /// marker on each matching session row.
    pub fn comment_sessions(mut self, sessions: HashSet<SessionId>) -> Self {
        self.comment_sessions = sessions;
        self
    }

    /// Whether a session row should display the pending-comment marker.
    pub(crate) fn session_has_comments(&self, id: &SessionId) -> bool {
        self.comment_sessions.contains(id)
    }

    /// Mark a set of projects as having a held-back background pull.
    /// Renders a ⚠ badge on each matching project row.
    pub fn pull_blocked_projects(mut self, blocked: HashMap<ProjectId, &'a str>) -> Self {
        self.pull_blocked_projects = blocked;
        self
    }

    /// Whether a project row should display the FF-blocked badge.
    pub(crate) fn project_is_pull_blocked(&self, id: &ProjectId) -> bool {
        self.pull_blocked_projects.contains_key(id)
    }

    /// When false, never show the `(program)` suffix. When true (default),
    /// show it only if the list has more than one distinct program.
    pub fn show_session_program(mut self, b: bool) -> Self {
        self.show_session_program = b;
        self
    }

    /// Force the `(program)` suffix decision (see [`show_program_override`]).
    /// Used by the pinned recents panel so its rows mirror the real rows.
    ///
    /// [`show_program_override`]: Self::show_program_override
    pub fn show_program_override(mut self, b: bool) -> Self {
        self.show_program_override = Some(b);
        self
    }

    /// When true, render PR labels as colored text on default bg
    /// (pre-pill behavior). Default false renders them as a colored pill.
    pub fn invert_pr_label_color(mut self, b: bool) -> Self {
        self.invert_pr_label_color = b;
        self
    }

    /// Configure the labels that flag an open PR as awaiting reviewer action.
    pub fn review_labels(mut self, labels: &'a [String]) -> Self {
        self.review_labels = labels;
        self
    }

    /// Set the tick counter for spinner animation
    pub fn tick(mut self, tick: u64) -> Self {
        self.tick = tick;
        self
    }

    /// Set the highlight style
    pub fn highlight_style(mut self, style: Style) -> Self {
        self.highlight_style = style;
        self
    }

    /// Pick the single status glyph and colour for a worktree row.
    ///
    /// Priority (first wins):
    /// 1. Creating / Merging / Pushing → animated spinner
    /// 2. CascadePaused        → `⏸` with warning accent
    /// 3. Agent `Working`      → animated spinner
    /// 4. Agent `WaitingForInput` → `?` glyph
    /// 5. `unread`             → `◆` diamond
    /// 6. Running (idle/unknown, no unread) → `●` filled circle
    /// 7. Stopped              → `○` open circle
    fn session_status_glyph(
        &self,
        status: SessionStatus,
        agent_state: Option<AgentState>,
        unread: bool,
    ) -> Option<(String, Color)> {
        if matches!(
            status,
            SessionStatus::Creating | SessionStatus::Merging | SessionStatus::Pushing
        ) {
            let step = self.tick as usize / 3;
            let frame = SPINNER_FRAMES[step % SPINNER_FRAMES.len()];
            return Some((frame.to_string(), self.theme.status_creating));
        }
        if status == SessionStatus::CascadePaused {
            return Some(("⏸".to_string(), self.theme.agent_waiting));
        }
        if status == SessionStatus::Running {
            match agent_state {
                Some(AgentState::Working) => {
                    let step = self.tick as usize / 3;
                    let frame = SPINNER_FRAMES[step % SPINNER_FRAMES.len()];
                    let color = self.theme.agent_working.color_for_tick(step as u64);
                    return Some((frame.to_string(), color));
                }
                Some(AgentState::WaitingForInput) => {
                    return Some(("?".to_string(), self.theme.agent_waiting));
                }
                _ => {}
            }
            if unread {
                return Some(("◆".to_string(), self.theme.unread_indicator));
            }
            return Some(("●".to_string(), self.theme.status_running));
        }
        // Stopped
        Some(("○".to_string(), self.theme.status_stopped))
    }

    /// Supply the precomputed number/colour map used to render recent-session
    /// rows (see [`worktree_display_info`]). The recents panel renders only its
    /// own slice of the list, so it can't derive numbers from the worktree rows
    /// itself — the caller computes the map over the *full* list and passes it
    /// in here.
    pub fn recent_display_info(mut self, info: HashMap<SessionId, (usize, Color)>) -> Self {
        self.recent_display_info = info;
        self
    }

    /// Check whether this widget's sessions use more than one distinct program.
    fn has_mixed_programs(&self) -> bool {
        list_has_mixed_programs(self.items)
    }
}

/// Whether the session rows in `items` use more than one distinct program.
/// Drives the `(program)` suffix. Exposed so a caller (the recents panel) can
/// compute the decision over the *full* list and mirror it onto a slice.
pub fn list_has_mixed_programs(items: &[SessionListItem]) -> bool {
    let mut first = None;
    for item in items {
        let program = match item {
            SessionListItem::Worktree { program, .. }
            | SessionListItem::RecentSession { program, .. } => program,
            _ => continue,
        };
        match first {
            None => first = Some(program_name(program)),
            Some(p) if p != program_name(program) => return true,
            _ => {}
        }
    }
    false
}

/// The base program name, excluding any arguments (e.g. "claude --mode auto" -> "claude").
pub(super) fn program_name(program: &str) -> &str {
    program.split_whitespace().next().unwrap_or(program)
}

/// Precompute each worktree's displayed number and session colour by walking
/// the list once, mirroring the counters in [`TreeList::to_list_items`]. The
/// recents panel looks its rows up here so each shows the exact same number and
/// colour the session has in its real position below. `RecentsHeader` /
/// `RecentSession` rows are skipped, so they never perturb the numbering.
pub fn worktree_display_info(
    items: &[SessionListItem],
    theme: &Theme,
) -> HashMap<SessionId, (usize, Color)> {
    let mut map = HashMap::new();
    let mut project_index: usize = 0;
    let mut current_session_color = theme.project_color(0).1;
    let mut worktree_number: usize = 0;
    for item in items {
        match item {
            SessionListItem::Project { .. } => {
                current_session_color = theme.project_color(project_index).1;
                project_index += 1;
            }
            SessionListItem::Worktree { id, .. } => {
                worktree_number += 1;
                map.insert(*id, (worktree_number, current_session_color));
            }
            _ => {}
        }
    }
    map
}
