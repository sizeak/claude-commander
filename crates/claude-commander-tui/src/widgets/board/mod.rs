//! Kanban board widget.
//!
//! Navigation state ([`BoardState`]), pure layout geometry ([`layout`]), and the
//! rendering [`BoardWidget`] for the full-screen board that replaces the session
//! tree list.

pub mod layout;
mod render;
mod state;

pub use layout::BoardRects;
pub use render::{BoardButtonRegion, BoardHitRegion, BoardWidget, CardButton};
pub use state::BoardState;
