//! Configuration and persistence module
//!
//! Handles:
//! - User configuration (`~/.claude-commander/config.toml`)
//! - Persistent state (`~/.claude-commander/state.json`)
//! - Worktree directory management

pub mod keybindings;
pub mod theme;
mod settings;
pub mod storage;
mod store;

pub use keybindings::{BindableAction, KeyBinding, KeyBindings};
pub use settings::*;
pub use storage::*;
pub use store::StateStore;
pub use theme::{ColorValue, ThemeOverrides};
