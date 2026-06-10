//! Nix flake integration: wrap launch commands in `nix develop` so sessions
//! run inside the project's dev shell when the project is a flake.

use std::ffi::OsStr;
use std::sync::OnceLock;

use super::*;

/// Wrap a shell command string so it runs inside the Nix dev shell of the
/// pane's working directory (`nix develop`'s flake ref defaults to `.`).
///
/// The command is a full shell string (it may already contain quoting), not
/// an argv list, so it must go through `sh -c` as a single argument. `exec`
/// replaces that `sh` with the program, avoiding an extra process layer.
pub(super) fn wrap_in_nix_develop(cmd: &str) -> String {
    let escaped = lifecycle::shell_escape_single_quote(&format!("exec {cmd}"));
    format!("nix develop --command sh -c '{escaped}'")
}

/// Whether a `nix` executable exists in any directory of the given PATH value.
fn nix_in_path(path_var: Option<&OsStr>) -> bool {
    path_var.is_some_and(|paths| std::env::split_paths(paths).any(|dir| dir.join("nix").is_file()))
}

/// Cached check for `nix` on the current process PATH. Cached because every
/// session launch and restart consults it and PATH doesn't change mid-run.
fn nix_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| nix_in_path(std::env::var_os("PATH").as_deref()))
}

/// Whether a launch in `dir` should run inside `nix develop`: the config
/// option is on, the directory has a `flake.nix` at its root, and nix is
/// installed.
fn should_use_nix_develop(enabled: bool, dir: &Path, nix_available: bool) -> bool {
    enabled && nix_available && dir.join("flake.nix").is_file()
}

impl SessionManager {
    /// Wrap `cmd` in `nix develop --command` when `dir` is a flake project
    /// (see `should_use_nix_develop`); otherwise return it unchanged. Applied
    /// as the outermost layer of every launch command, after Claude-specific
    /// flag injection.
    pub(super) fn maybe_wrap_nix_develop(&self, cmd: &str, dir: &Path) -> String {
        if should_use_nix_develop(self.config_store.read().nix_develop, dir, nix_available()) {
            info!("Launching inside nix develop shell at {}", dir.display());
            wrap_in_nix_develop(cmd)
        } else {
            cmd.to_string()
        }
    }
}

#[cfg(test)]
mod nix_tests {
    use super::*;

    #[test]
    fn wrap_simple_command() {
        assert_eq!(
            wrap_in_nix_develop("claude"),
            "nix develop --command sh -c 'exec claude'"
        );
    }

    #[test]
    fn wrap_escapes_single_quotes_in_command() {
        assert_eq!(
            wrap_in_nix_develop("claude -n 'my session' --resume"),
            "nix develop --command sh -c 'exec claude -n '\\''my session'\\'' --resume'"
        );
    }

    #[test]
    fn should_use_requires_flake_file() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(!should_use_nix_develop(true, dir.path(), true));

        std::fs::write(dir.path().join("flake.nix"), "{}").unwrap();
        assert!(should_use_nix_develop(true, dir.path(), true));
    }

    #[test]
    fn should_use_respects_config_and_nix_availability() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("flake.nix"), "{}").unwrap();

        assert!(!should_use_nix_develop(false, dir.path(), true));
        assert!(!should_use_nix_develop(true, dir.path(), false));
    }

    #[test]
    fn should_use_ignores_flake_directory() {
        // A directory named flake.nix is not a flake
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("flake.nix")).unwrap();
        assert!(!should_use_nix_develop(true, dir.path(), true));
    }

    #[test]
    fn nix_in_path_finds_executable() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("nix"), "").unwrap();
        let path_var = std::env::join_paths([dir.path()]).unwrap();
        assert!(nix_in_path(Some(path_var.as_os_str())));
    }

    #[test]
    fn nix_in_path_misses_when_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        let path_var = std::env::join_paths([dir.path()]).unwrap();
        assert!(!nix_in_path(Some(path_var.as_os_str())));
        assert!(!nix_in_path(None));
    }
}
