//! Session-list view mode (project / sections / stacks).
//!
//! Lives in `config` rather than `tui` because the user's last-selected view
//! is persisted in `AppState`. The TUI re-exports it from `crate::tui::app`
//! for ergonomics at call sites.

use serde::{Deserialize, Serialize};

/// Which list view mode is active.
///
/// Three modes are supported and the user cycles between them with the
/// `ToggleViewMode` key (default `v`):
/// * `ProjectGrouped` — flat tree, sessions indented under their project,
///   stacks indented under their parent. The default.
/// * `SectionGrouped` — sessions bucketed by user-configured sections based
///   on each session's own PR state. Stacks may be split across sections.
/// * `SectionGroupedWithStacks` — same section layout, but stacks are
///   grouped as a unit; the whole stack lands in the section chosen by
///   the newest leaf, and indentation is preserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ViewMode {
    #[default]
    ProjectGrouped,
    SectionGrouped,
    SectionGroupedWithStacks,
}

impl ViewMode {
    /// Returns the next view in the cycle:
    /// Project → Sections → Stacks → Project.
    pub fn next(self) -> Self {
        match self {
            Self::ProjectGrouped => Self::SectionGrouped,
            Self::SectionGrouped => Self::SectionGroupedWithStacks,
            Self::SectionGroupedWithStacks => Self::ProjectGrouped,
        }
    }

    /// Heading label rendered above the session tree so the user can see
    /// which view is active at a glance.
    pub fn heading_label(self) -> &'static str {
        match self {
            Self::ProjectGrouped => " Sessions [Project]:",
            Self::SectionGrouped => " Sessions [Sections]:",
            Self::SectionGroupedWithStacks => " Sessions [Stacks]:",
        }
    }

    /// Whether this view depends on user-configured sections.
    /// Used to decide whether a persisted/computed view is meaningful when
    /// no sections are configured, and to skip section modes in the cycle
    /// when sections are absent.
    pub fn is_section_view(self) -> bool {
        matches!(self, Self::SectionGrouped | Self::SectionGroupedWithStacks)
    }
}
