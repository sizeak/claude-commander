//! Session view mode (project / sections / stacks / board).
//!
//! Lives in `config` rather than `tui` because the user's last-selected view
//! is persisted (in `tui.json`). The TUI re-exports it from `crate::tui::app`
//! for ergonomics at call sites.

use serde::{Deserialize, Serialize};

/// Which view is active. Cycled with the `ToggleViewMode` key (default `v`):
/// Project → Sections → Section Stacks → Board → (repeat).
///
/// The first three are single-pane **list** views (a tree of sections →
/// projects → sessions); `Board` is the full-screen kanban board. There is no
/// right pane in any view — session detail is the `i` Info modal.
/// * `ProjectGrouped` — flat tree, sessions indented under their project,
///   stacks indented under their parent.
/// * `SectionGrouped` — sessions bucketed by user-configured sections based
///   on each session's own PR state. Stacks may be split across sections.
/// * `SectionStacks` — same section layout, but stacks are grouped as a unit;
///   the whole stack lands in the section chosen by the newest leaf, and
///   indentation is preserved. This is the default (a section-less setup falls
///   back to `ProjectGrouped`).
/// * `Board` — full-screen kanban board (sections as columns, sessions as
///   cards).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ViewMode {
    ProjectGrouped,
    SectionGrouped,
    // Accept the pre-rename variant name so `tui.json`/`state.json` written by
    // an earlier build (when this was `SectionGroupedWithStacks`) still parse.
    #[serde(alias = "SectionGroupedWithStacks")]
    #[default]
    SectionStacks,
    /// Full-screen kanban board (sections as columns, sessions as cards).
    Board,
}

impl ViewMode {
    /// The next view in the cycle: Project → Sections → Stacks → Board → Project.
    pub fn next(self) -> Self {
        match self {
            Self::ProjectGrouped => Self::SectionGrouped,
            Self::SectionGrouped => Self::SectionStacks,
            Self::SectionStacks => Self::Board,
            Self::Board => Self::ProjectGrouped,
        }
    }

    /// Heading label rendered above the session tree (list views only; the
    /// board draws its own top bar).
    pub fn heading_label(self) -> &'static str {
        match self {
            Self::ProjectGrouped => " Sessions [Project]:",
            Self::SectionGrouped => " Sessions [Sections]:",
            Self::SectionStacks => " Sessions [Section Stacks]:",
            Self::Board => " Board:",
        }
    }

    /// Whether this view depends on user-configured sections.
    pub fn is_section_view(self) -> bool {
        matches!(self, Self::SectionGrouped | Self::SectionStacks)
    }

    /// Whether this is the kanban board (as opposed to a list view).
    pub fn is_board(self) -> bool {
        matches!(self, Self::Board)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_cycles_project_sections_stacks_board() {
        assert_eq!(ViewMode::ProjectGrouped.next(), ViewMode::SectionGrouped);
        assert_eq!(ViewMode::SectionGrouped.next(), ViewMode::SectionStacks);
        assert_eq!(ViewMode::SectionStacks.next(), ViewMode::Board);
        assert_eq!(ViewMode::Board.next(), ViewMode::ProjectGrouped);
    }

    #[test]
    fn section_stacks_alias_deserializes() {
        // Prefs written by an older build used `SectionGroupedWithStacks`.
        let v: ViewMode = serde_json::from_str("\"SectionGroupedWithStacks\"").unwrap();
        assert_eq!(v, ViewMode::SectionStacks);
    }
}
