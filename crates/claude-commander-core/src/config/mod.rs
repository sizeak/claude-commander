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
pub(crate) mod store;
pub mod theme;
mod view_mode;

pub use config::*;
pub use config_store::ConfigStore;

/// Write `contents` to `path`, restricting the file to owner read/write
/// (`0o600`) on Unix. The config file carries remote-server bearer tokens, so
/// it must not be group/world-readable. A plain write on non-Unix platforms.
pub(crate) fn write_private_file(
    path: &std::path::Path,
    contents: impl AsRef<[u8]>,
) -> std::io::Result<()> {
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
pub use keybindings::{BindableAction, KeyBinding, KeyBindings};
pub use storage::*;
pub use store::StateStore;
pub use theme::{ColorValue, ThemeOverrides};
pub use view_mode::ViewMode;
