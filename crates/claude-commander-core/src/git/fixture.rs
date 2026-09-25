//! `git` for test fixtures: never signs, whatever the developer's config says.
//!
//! A fixture that runs `git commit` (or `tag`, `merge`, `rebase`, `cherry-pick`,
//! `am` -- anything that writes a commit or an annotated tag) against a temp repo
//! otherwise inherits the developer's global config. With `commit.gpgsign=true`
//! routed through 1Password's `op-ssh-sign`, every such test then hangs for about
//! a minute and fails whenever the vault is locked, and passes when it is not.
//!
//! Test-only by construction: gated on `any(test, feature = "test-support")`, so
//! no release build can reach it. Production commits keep the operator's signing
//! policy; a test that drives a *production* path which commits (cascade's
//! `merge --no-ff`) opts its temp repo out with [`disable_signing_in_repo`]
//! instead, because it cannot put `-c` on a command it does not build.
//!
//! `scripts/verify.sh` runs every lane under a global git config that makes any
//! signing attempt fail at once (`cc_export_poisoned_git_signing`), so a fixture
//! that bypasses these helpers fails loudly there rather than on a locked laptop.

use std::path::Path;

use super::{git_command, git_command_std};

/// The config a fixture forces onto every git it runs, as `key=value` pairs.
pub const FIXTURE_GIT_CONFIG: [&str; 2] = ["commit.gpgsign=false", "tag.gpgsign=false"];

/// [`git_command`] with signing forced off by `-c`, which outranks every config
/// file (global, repo-local, and `GIT_CONFIG_GLOBAL`).
pub fn fixture_git() -> tokio::process::Command {
    let mut cmd = git_command();
    for pair in FIXTURE_GIT_CONFIG {
        cmd.args(["-c", pair]);
    }
    cmd
}

/// [`fixture_git`] for synchronous fixtures.
pub fn fixture_git_std() -> std::process::Command {
    let mut cmd = git_command_std();
    for pair in FIXTURE_GIT_CONFIG {
        cmd.args(["-c", pair]);
    }
    cmd
}

/// Write [`FIXTURE_GIT_CONFIG`] into `repo`'s local config, so git that the
/// *code under test* spawns there (and in its worktrees, which share it) does
/// not sign either. Repo-local config outranks the global file, which is where
/// a developer's signing policy lives.
pub fn disable_signing_in_repo(repo: &Path) {
    for pair in FIXTURE_GIT_CONFIG {
        let (key, value) = pair.split_once('=').expect("key=value");
        let out = git_command_std()
            .current_dir(repo)
            .args(["config", key, value])
            .output()
            .expect("spawn git config");
        assert!(
            out.status.success(),
            "git config {key} {value} failed in {}: {}",
            repo.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A global config that turns signing on and makes the signer fail, like a
    /// developer's `op-ssh-sign` with the vault locked -- minus the minute-long
    /// hang. Returned as env for one command, so the real global is never touched.
    fn poisoned_env(dir: &Path) -> Vec<(&'static str, String)> {
        let global = dir.join("poisoned-gitconfig");
        std::fs::write(
            &global,
            "[commit]\n\tgpgsign = true\n[tag]\n\tgpgsign = true\n\
             [gpg]\n\tformat = ssh\n[gpg \"ssh\"]\n\tprogram = false\n",
        )
        .unwrap();
        let mut env = vec![
            ("GIT_CONFIG_GLOBAL", global.display().to_string()),
            ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ];
        for key in [
            "GIT_AUTHOR_NAME",
            "GIT_AUTHOR_EMAIL",
            "GIT_COMMITTER_NAME",
            "GIT_COMMITTER_EMAIL",
        ] {
            env.push((key, "t".into()));
        }
        env
    }

    fn run(mut cmd: std::process::Command, dir: &Path, args: &[&str]) -> bool {
        cmd.envs(poisoned_env(dir))
            .current_dir(dir)
            .args(args)
            .output()
            .expect("spawn git")
            .status
            .success()
    }

    fn init(dir: &Path) {
        assert!(run(git_command_std(), dir, &["init", "-q"]));
    }

    const COMMIT: &[&str] = &["commit", "-q", "--allow-empty", "-m", "c"];
    const TAG: &[&str] = &["tag", "-a", "-m", "t", "v1"];

    /// The control: under the poison a plain git commit *does* try to sign, so
    /// the other tests below prove something.
    #[test]
    fn a_plain_commit_fails_under_the_poison() {
        let tmp = TempDir::new().unwrap();
        init(tmp.path());
        assert!(!run(git_command_std(), tmp.path(), COMMIT));
    }

    #[test]
    fn fixture_git_neither_signs_commits_nor_tags() {
        let tmp = TempDir::new().unwrap();
        init(tmp.path());
        assert!(run(fixture_git_std(), tmp.path(), COMMIT));
        assert!(run(fixture_git_std(), tmp.path(), TAG));
    }

    #[test]
    fn repo_local_opt_out_covers_git_the_fixture_did_not_build() {
        let tmp = TempDir::new().unwrap();
        init(tmp.path());
        disable_signing_in_repo(tmp.path());
        assert!(run(git_command_std(), tmp.path(), COMMIT));
        assert!(run(git_command_std(), tmp.path(), TAG));
    }

    #[tokio::test]
    async fn async_fixture_git_does_not_sign() {
        let tmp = TempDir::new().unwrap();
        init(tmp.path());
        let status = fixture_git()
            .envs(poisoned_env(tmp.path()))
            .current_dir(tmp.path())
            .args(COMMIT)
            .status()
            .await
            .unwrap();
        assert!(status.success());
    }
}
