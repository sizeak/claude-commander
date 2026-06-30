//! Configuration and persistence module
//!
//! Handles:
//! - User configuration (`~/.claude-commander/config.toml`)
//! - Persistent state (`~/.claude-commander/state.json`)
//! - Worktree directory management

// The config-struct file is deliberately named `config.rs` to match
// `config.toml` (the project standardises on "config", not "settings").
// `config::config` is intentional; suppress the module-inception lint.
#[allow(clippy::module_inception)]
mod config;
mod config_store;
pub mod keybindings;
pub mod storage;
mod store;
pub mod theme;
mod view_mode;

pub use config::*;
pub use config_store::ConfigStore;
pub use keybindings::{BindableAction, KeyBinding, KeyBindings};
pub use storage::*;
pub use store::StateStore;
pub use theme::{ColorValue, ThemeOverrides};
pub use view_mode::ViewMode;
