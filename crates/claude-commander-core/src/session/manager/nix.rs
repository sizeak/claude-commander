//! Nix flake integration: wrap launch commands in `nix develop` so sessions
//! run inside the project's dev shell when the project is a flake.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::sync::{Mutex, OnceLock};

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

/// Whether to spawn a background `nix develop` pre-warm for `dir`: the launch
/// would use nix develop (see `should_use_nix_develop`) *and* we haven't already
/// attempted a warm for this directory this process. Records the directory in
/// `seen` on the first `true`, so each repo is warm-attempted at most once per
/// process — rapidly creating several sessions in one project only warms once.
/// Note this dedups on *attempt*, not success: a failed warm won't be retried.
fn should_prewarm(
    enabled: bool,
    dir: &Path,
    nix_available: bool,
    seen: &Mutex<HashSet<PathBuf>>,
) -> bool {
    if !should_use_nix_develop(enabled, dir, nix_available) {
        return false;
    }
    // insert() returns true when the value was newly inserted (not yet warmed).
    seen.lock().unwrap().insert(dir.to_path_buf())
}

/// Process-global set of repo paths already pre-warmed this run.
fn prewarmed_repos() -> &'static Mutex<HashSet<PathBuf>> {
    static PREWARMED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    PREWARMED.get_or_init(|| Mutex::new(HashSet::new()))
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

    /// Kick off a background `nix develop --command true` in `repo_path` so the
    /// project's dev shell is evaluated and its dependencies fetched/built into
    /// `/nix/store` *before* a freshly-created session's pane runs `nix develop`
    /// itself. The new worktree checks out the same commit, so the toolchain
    /// store paths its dev shell needs are typically already built by this warm
    /// (the store is content-addressed and shared) — the pane's own `nix
    /// develop` then reuses the warm store instead of building from cold. The
    /// win is bounded by the overlap window (git fetch + worktree add): a
    /// genuinely cold, long build isn't eliminated, just started earlier.
    ///
    /// Fire-and-forget and best-effort: no-op when the project isn't a nix
    /// flake (or `nix_develop` is off / nix is absent), and warms each repo at
    /// most once per process (see `should_prewarm`).
    pub(super) fn prewarm_nix_shell(&self, repo_path: &Path) {
        let enabled = self.config_store.read().nix_develop;
        if !should_prewarm(enabled, repo_path, nix_available(), prewarmed_repos()) {
            return;
        }
        info!("Pre-warming nix dev shell at {}", repo_path.display());
        let dir = repo_path.to_path_buf();
        tokio::spawn(async move {
            let result = tokio::process::Command::new("nix")
                .current_dir(&dir)
                .args(["develop", "--command", "true"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .await;
            match result {
                Ok(output) if output.status.success() => {
                    debug!("Pre-warmed nix dev shell at {}", dir.display());
                }
                Ok(output) => {
                    warn!(
                        "nix develop pre-warm at {} exited with {}: {}",
                        dir.display(),
                        output.status,
                        String::from_utf8_lossy(&output.stderr).trim()
                    );
                }
                Err(e) => warn!(
                    "Failed to spawn nix develop pre-warm at {}: {}",
                    dir.display(),
                    e
                ),
            }
        });
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
    fn prewarm_warms_flake_dir_once() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("flake.nix"), "{}").unwrap();
        let seen = Mutex::new(HashSet::new());

        // First call warms; second call for the same dir is deduped.
        assert!(should_prewarm(true, dir.path(), true, &seen));
        assert!(!should_prewarm(true, dir.path(), true, &seen));
    }

    #[test]
    fn prewarm_respects_config_flake_and_nix() {
        let seen = Mutex::new(HashSet::new());

        // Not a flake.
        let plain = tempfile::TempDir::new().unwrap();
        assert!(!should_prewarm(true, plain.path(), true, &seen));

        let flake = tempfile::TempDir::new().unwrap();
        std::fs::write(flake.path().join("flake.nix"), "{}").unwrap();
        // Config disabled, or nix unavailable — no warm, and nothing recorded.
        assert!(!should_prewarm(false, flake.path(), true, &seen));
        assert!(!should_prewarm(true, flake.path(), false, &seen));
    }

    #[test]
    fn prewarm_tracks_dirs_independently() {
        let a = tempfile::TempDir::new().unwrap();
        let b = tempfile::TempDir::new().unwrap();
        std::fs::write(a.path().join("flake.nix"), "{}").unwrap();
        std::fs::write(b.path().join("flake.nix"), "{}").unwrap();
        let seen = Mutex::new(HashSet::new());

        assert!(should_prewarm(true, a.path(), true, &seen));
        // A different flake dir still warms once of its own.
        assert!(should_prewarm(true, b.path(), true, &seen));
        assert!(!should_prewarm(true, a.path(), true, &seen));
        assert!(!should_prewarm(true, b.path(), true, &seen));
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
