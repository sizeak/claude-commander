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
///
/// The mode is applied *before* the sensitive bytes hit disk so there is no
/// window where the token-bearing content is group/world-readable: a newly
/// created file is opened `0o600` (`OpenOptions::mode` only takes effect on
/// creation), and a pre-existing file (whose mode `.mode()` would ignore) is
/// chmod'd down before it is truncated and rewritten.
pub(crate) fn write_private_file(
    path: &std::path::Path,
    contents: impl AsRef<[u8]>,
) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        // Tighten a pre-existing file (open with `.mode()` leaves its perms
        // untouched) before writing the sensitive bytes.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        file.write_all(contents.as_ref())?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
    }
}
pub use keybindings::{BindableAction, KeyBinding, KeyBindings};
pub use storage::*;
pub use store::StateStore;
pub use theme::{ColorValue, ThemeOverrides};
pub use view_mode::ViewMode;

#[cfg(all(test, unix))]
mod write_private_file_tests {
    use super::write_private_file;
    use std::os::unix::fs::PermissionsExt;

    fn mode_of(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn fresh_file_is_owner_only() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("secret.toml");
        write_private_file(&path, b"token = \"abc\"").unwrap();
        assert_eq!(
            mode_of(&path),
            0o600,
            "a freshly written file must be 0o600"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"token = \"abc\"");
    }

    #[test]
    fn preexisting_permissive_file_is_tightened() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("secret.toml");
        // Simulate a file a prior run (or the user) left group/world-readable.
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();

        write_private_file(&path, b"token = \"new\"").unwrap();
        assert_eq!(
            mode_of(&path),
            0o600,
            "an over-permissive pre-existing file must be chmod'd down on write"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"token = \"new\"");
    }
}
