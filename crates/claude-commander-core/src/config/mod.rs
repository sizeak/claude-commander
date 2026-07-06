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
mod migrations;
pub mod storage;
pub(crate) mod store;
pub mod theme;
mod view_mode;

pub use config::*;
pub use config_store::ConfigStore;

/// Write `contents` to `path` atomically, restricting the file to owner
/// read/write (`0o600`) on Unix. The config file carries remote-server bearer
/// tokens, so it must not be group/world-readable — and a mid-write crash must
/// not leave a truncated/empty token file behind.
///
/// The write goes to a temporary file in the same directory (so the final
/// `rename` is atomic on the same filesystem), then renames over the target.
/// The rename replaces the target's inode, so a reader either sees the old
/// complete file or the new complete file, never a partial one — and a crash
/// before the rename leaves the target untouched. There is no crash window in
/// which the token-bearing content is truncated in place.
///
/// The `0o600` mode is applied to the temp file *before* the sensitive bytes
/// hit disk (the temp is opened `0o600` via `OpenOptions::mode`, then chmod'd
/// down in case a same-pid leftover was looser), so the secret is never
/// world-readable even transiently, and the renamed target inherits `0o600`
/// regardless of any pre-existing target's permissions.
pub(crate) fn write_private_file(
    path: &std::path::Path,
    contents: impl AsRef<[u8]>,
) -> std::io::Result<()> {
    // Temp lives in the same directory as the target so `rename` stays on one
    // filesystem (a cross-device rename would fail). The pid keeps concurrent
    // writers from clobbering each other's temp.
    let tmp_path = path.with_extension(format!("tmp.{}", std::process::id()));

    let write_and_rename = || -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp_path)?;
            // Tighten a pre-existing same-pid temp (open with `.mode()` leaves
            // its perms untouched) before writing the sensitive bytes.
            std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600))?;
            file.write_all(contents.as_ref())?;
            file.sync_all()?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&tmp_path, contents.as_ref())?;
        }
        std::fs::rename(&tmp_path, path)
    };

    write_and_rename().inspect_err(|_| {
        // A failed write/rename must not leave the temp lingering.
        let _ = std::fs::remove_file(&tmp_path);
    })
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

    #[test]
    fn write_is_atomic_rename_not_in_place_truncate() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("secret.toml");
        write_private_file(&path, b"token = \"a\"").unwrap();
        let ino1 = std::fs::metadata(&path).unwrap().ino();

        write_private_file(&path, b"token = \"bb\"").unwrap();
        let ino2 = std::fs::metadata(&path).unwrap().ino();

        // An atomic write renames a fresh temp over the target, changing the
        // inode; an in-place truncate-then-write keeps the same inode and opens
        // a window where the token-bearing file is empty/partial on disk (the
        // crash-corruption this guards against).
        assert_ne!(ino1, ino2, "write must replace the file via atomic rename");
        assert_eq!(std::fs::read(&path).unwrap(), b"token = \"bb\"");
        assert_eq!(mode_of(&path), 0o600, "the renamed file must be 0o600");
    }

    #[test]
    fn write_leaves_no_temp_sibling() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        write_private_file(&path, b"x = 1").unwrap();
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["config.toml".to_string()],
            "a successful atomic write must leave no temp file behind"
        );
    }
}
