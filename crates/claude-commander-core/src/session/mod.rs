//! Session management module
//!
//! Provides the hierarchical session model:
//! - `Project` - A git repository (parent)
//! - `WorktreeSession` - A worktree session within a project (child)
//! - `SessionManager` - Coordinates session lifecycle

pub mod board;
mod branch_reconcile;
mod manager;
pub mod section;
mod types;

pub use board::{
    Board, BoardBackendInput, BoardCard, BoardColumn, BoardPos, BoardProjectEntry, BoardServer,
    build_board,
};
pub use branch_reconcile::decide_branch_reconcile;
pub use manager::*;
pub use section::{
    IN_PROGRESS, RenderedSection, SectionAssignment, SectionConfig, apply_assignment,
    assign_section, build_sections, clear_override_and_reassign, default_board_sections,
    effective_sections, place_created_session, rename_section, section_name_available,
};
pub use types::*;
