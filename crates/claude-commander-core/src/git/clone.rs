//! Non-interactive, timeout-bounded repository clone.
//!
//! Two invocations, not one: a [`CloneSource::Github`] slug goes through
//! `gh repo clone` (which resolves the user's configured protocol and carries
//! their GitHub credentials, so private repos work), and anything else through
//! a plain `git clone`.
//!
//! The whole module exists to make one failure mode impossible. This clone runs
//! **unattended**, on a machine that may have no terminal at all, and the job
//! watching it (`CloneJobs`) has no way to answer a question. A `git` that stops
//! to ask for a password, or an `ssh` that stops to ask whether an unknown host
//! key is acceptable, does not fail — it waits, forever, with the job stuck in
//! `Running`. So every prompting route is closed up front, and the run is
//! additionally bounded by a wall-clock timeout with a real kill behind it.
//!
//! Nearly every statement above is a claim about code outside this repo. The
//! receipts are inline, and the ones that could be turned into tests were:
//! `the_noninteractive_recipe_stops_git_prompting` pins the env recipe against
//! a real `git`, and `a_timed_out_run_kills_the_child_and_its_descendants` pins
//! the kill.
//!
//! # Known limitation: `core.sshCommand` is not honoured
//!
//! Setting `GIT_SSH_COMMAND` overrides git's `core.sshCommand` config
//! (`git-config(1)`, `core.sshCommand`: "is overridden when the environment
//! variable is set"). So a user who configures their ssh identity there rather
//! than in `~/.ssh/config` will see an ssh clone fail to authenticate here while
//! the same clone works in their shell. **That is the expected behaviour, not a
//! bug to re-diagnose.**
//!
//! It is accepted rather than fixed because the affected population is narrower
//! than it first looks: only *global* and *system* config can apply to a clone,
//! since the repository being cloned does not exist yet and has no local config
//! to read. Preserving the value would cost a `git config --get core.sshCommand`
//! subprocess on every clone, to buy back a case where the alternative is a fast,
//! legible auth error rather than the indefinite hang this module exists to
//! prevent. An inherited `GIT_SSH_COMMAND` *is* preserved — see [`ssh_command`].

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use claude_commander_protocol::github::{
    CloneSource, redact_credentials, validate_clone_url, validate_repo_slug,
};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tracing::{debug, warn};

use crate::error::{GitError, Result};
use crate::git::is_gh_available;

/// Clone `source` into `dest`, non-interactively, within `timeout`.
///
/// `dest` must be absolute and is passed to git verbatim; git creates it (and
/// any missing parents). On failure a destination *this call created* is
/// removed, so a retry is not blocked by a half-written checkout; a destination
/// that already existed is left untouched.
///
/// The timeout is a parameter rather than a config read so this stays callable
/// from a test with a 300ms budget; `CommanderService` passes
/// `Config::clone_timeout_secs`.
pub async fn run_clone(source: &CloneSource, dest: &Path, timeout: Duration) -> Result<()> {
    // Re-validate at the last stop before argv. Callers validate too, but the
    // hazard being closed here (a source read as a flag) is invisible when it
    // goes wrong, and the check is pure string inspection.
    let program = match source {
        CloneSource::Github { full_name } => {
            validate_repo_slug(full_name).map_err(rejected)?;
            if !is_gh_available().await {
                return Err(GitError::GhUnavailable.into());
            }
            "gh repo clone"
        }
        CloneSource::Url { url } => {
            validate_clone_url(url).map_err(rejected)?;
            "git clone"
        }
    };

    // The destination, never the source: a hand-typed URL may carry
    // `user:token@` userinfo, which has no business in a log file.
    debug!(
        "{program} -> {} (timeout {}s)",
        dest.display(),
        timeout.as_secs()
    );

    let pre_existing = dest.exists();
    let result = run_bounded(clone_command(source, dest), program, timeout).await;

    if result.is_err() && !pre_existing {
        // git removes a destination it created when it fails on its own
        // (verified with git 2.53.0: `git clone -- /nonexistent /tmp/x` leaves
        // no `/tmp/x`), but it cannot do so when we SIGKILL it — SIGKILL is
        // uncatchable, so no cleanup path of its own can run. Hence this.
        if let Err(e) = std::fs::remove_dir_all(dest) {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!("failed to remove partial clone at {}: {e}", dest.display());
            }
        } else {
            debug!("removed partial clone at {}", dest.display());
        }
    }

    result
}

/// Turn a rejected source into an error that cannot quote a secret back.
///
/// Both validators route through here, and the `Url` arm does so even though its
/// [`CloneRejection`](claude_commander_protocol::github::CloneRejection) variants
/// look harmless today. That is the point: this module redacts where an error
/// string is *built*, rather than trusting each variant to stay harmless. The
/// `Github` arm proves why — `validate_repo_slug` splits on `/`, so a URL pasted
/// into the slug field always fails as `MalformedSlug`, whose `Display` echoes
/// the entire raw input. A credentialed URL went straight into an error message
/// through that path, and the reasoning that missed it was "these variants only
/// echo a scheme or a directory name", which was true of every variant but one.
///
/// Pinned by `a_rejection_message_carries_no_credentials`.
fn rejected(rejection: impl std::fmt::Display) -> GitError {
    GitError::OperationFailed(redact_credentials(&rejection.to_string()))
}

/// Build the clone invocation for `source`, with the non-interactive env applied.
///
/// The `--` before the source is load-bearing on **both** arms, and means the
/// same thing on each: end of options.
///
/// * `git clone -- <source> <dest>` — standard git option terminator.
/// * `gh repo clone -- <slug> <dest>` — gh's help renders this as
///   `gh repo clone <repository> [<directory>] [-- <gitflags>...]`, which reads
///   as if `--` introduced git flags. It does not: gh takes the first positional
///   as the repository either way. Verified with gh 2.96.0 —
///   `gh repo clone -- -version /tmp/x` reports `Could not resolve to a
///   Repository with the name 'sizeak/-version'`, while dropping the `--` gives
///   `unknown shorthand flag: 'v' in -version`. Pinned by
///   `both_arms_terminate_options_before_the_source` for the argv shape.
fn clone_command(source: &CloneSource, dest: &Path) -> Command {
    let mut cmd = match source {
        CloneSource::Github { full_name } => {
            let mut cmd = Command::new("gh");
            cmd.args(["repo", "clone", "--"]).arg(full_name).arg(dest);
            cmd
        }
        CloneSource::Url { url } => {
            let mut cmd = Command::new("git");
            cmd.args(["clone", "--"]).arg(url).arg(dest);
            cmd
        }
    };
    apply_noninteractive_env(&mut cmd);
    cmd
}

/// Close every route by which git (or the ssh it spawns) could stop and ask a
/// question. Applied to the `gh` arm too: `gh repo clone` shells out to
/// `git clone` (its help: "Pass additional `git clone` flags by listing them
/// after `--`"), and a child process inherits this environment.
///
/// * `GIT_TERMINAL_PROMPT=0` — "If this Boolean environment variable is set to
///   false, git will not prompt on the terminal" (`git help environment`,
///   git 2.53.0).
/// * `GIT_ASKPASS=""` — **empty, not unset, and this is the whole point.**
///   `GIT_TERMINAL_PROMPT=0` alone does not stop an askpass *helper* from
///   running, and a desktop machine routinely has one configured, which pops a
///   dialog nobody will ever see. Git consults `GIT_ASKPASS`, then
///   `core.askPass`, then `SSH_ASKPASS`; setting `GIT_ASKPASS` to the empty
///   string short-circuits that chain without nominating a program, leaving the
///   terminal fallback that the line above refuses. Verified with git 2.53.0 by
///   `printf 'protocol=https\nhost=example.invalid\n\n' | git credential fill`
///   under each combination, and pinned by the pair of tests
///   `without_the_recipe_git_runs_an_askpass_helper` /
///   `the_noninteractive_recipe_stops_git_prompting`.
/// * `GIT_SSH_COMMAND` — see [`ssh_command`].
fn apply_noninteractive_env(cmd: &mut Command) {
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env(
            "GIT_SSH_COMMAND",
            ssh_command(std::env::var("GIT_SSH_COMMAND").ok().as_deref()),
        );
}

/// The ssh command git should use, carrying `BatchMode=yes`.
///
/// `BatchMode=yes`: "If set to yes, user interaction such as password prompts
/// and host key confirmation requests will be disabled." (`ssh_config(5)`,
/// OpenSSH 10.4p1) — the two ways an ssh clone hangs unattended.
///
/// `GIT_SSH_COMMAND` is documented for "git fetch and git push"
/// (`git help environment`), which is what a clone runs underneath; verified
/// with git 2.53.0 by pointing it at a logging stub and observing
/// `git clone ssh://…` invoke it with `-o BatchMode=yes -o
/// SendEnv=GIT_PROTOCOL git@host git-upload-pack '/o/r.git'`.
///
/// An inherited `GIT_SSH_COMMAND` is preserved and appended to rather than
/// replaced, so a wrapper or `-i <key>` the operator configured still applies.
/// Our option lands first, and ssh takes "the first obtained value" for each
/// parameter (`ssh_config(5)`) — so an inherited command that explicitly sets
/// `BatchMode=no` would win. That is a deliberate escape hatch, not an
/// oversight.
///
/// **Known limitation:** git's `core.sshCommand` config is *not* preserved,
/// because `GIT_SSH_COMMAND` "takes precedence over" it (`git-config(1)`,
/// `core.sshCommand`: "is overridden when the environment variable is set").
/// A user whose identity setup lives there rather than in `~/.ssh/config` gets
/// a fast, legible auth failure instead of a clone that hangs.
fn ssh_command(inherited: Option<&str>) -> String {
    match inherited {
        Some(cmd) if !cmd.trim().is_empty() => format!("{} -o BatchMode=yes", cmd.trim()),
        _ => "ssh -o BatchMode=yes".to_string(),
    }
}

/// Run `cmd` to completion within `timeout`, killing it and everything it
/// spawned if it overruns.
///
/// `program` names the command for the error message ("git clone", …).
async fn run_bounded(mut cmd: Command, program: &str, timeout: Duration) -> Result<()> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        // A new process group, so the timeout can take the whole tree down:
        // `gh repo clone` spawns `git clone`, which spawns `ssh` or
        // `git-remote-https`. Killing only the process we spawned would leave a
        // live git writing into the destination that the failure path is about
        // to delete. `process_group(0)` is setpgid(0, 0), so the child's pgid
        // equals its own pid.
        .process_group(0);

    let mut child = cmd
        .spawn()
        .map_err(|e| GitError::OperationFailed(format!("failed to run {program}: {e}")))?;
    // Read once, before the wait: `Child::id` returns `None` after the child
    // has been reaped.
    let pgid = child.id().map(|id| Pid::from_raw(id as i32));

    // Drain stderr concurrently. Waiting on the child while its stderr pipe
    // fills would deadlock the pair.
    let mut pipe = child.stderr.take();
    let drain = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(pipe) = pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buf).await;
        }
        buf
    });

    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => {
            status.map_err(|e| GitError::OperationFailed(format!("{program} failed: {e}")))?
        }
        Err(_) => {
            warn!("{program} exceeded {}s; killing", timeout.as_secs());
            kill_tree(&mut child, pgid).await;
            return Err(GitError::CloneTimedOut {
                secs: timeout.as_secs(),
            }
            .into());
        }
    };

    if !status.success() {
        let stderr = drain.await.unwrap_or_default();
        // git suppresses its progress meter when stderr is not a terminal
        // ("Progress status is reported on the standard error stream by default
        // when it is attached to a terminal" — `git help clone`, `--progress`),
        // so what is left here is the actual diagnosis, short enough to pass on
        // whole.
        //
        // Redacted as the error is *built*, not merely kept out of the log: git
        // echoes the source in most failure messages, and a hand-typed URL may
        // carry `user:token@` userinfo. From here the string becomes a
        // `CloneStatus::Failed` message, crosses the wire and is rendered in a
        // UI, so the secret has to be gone at the one point all of those share.
        let stderr = String::from_utf8_lossy(&stderr);
        return Err(GitError::OperationFailed(format!(
            "{program} failed: {}",
            redact_credentials(stderr.trim())
        ))
        .into());
    }

    Ok(())
}

/// SIGKILL the child's process group, then reap the child itself.
///
/// The group kill covers every descendant that did not put itself into a group
/// of its own; neither `git` nor `ssh` is known to do so, but that is an
/// assumption, not a verified claim. `Child::kill` is still called afterwards:
/// it waits, which is what actually reaps the child rather than leaving a
/// zombie.
async fn kill_tree(child: &mut Child, pgid: Option<Pid>) {
    if let Some(pgid) = pgid
        && let Err(e) = killpg(pgid, Signal::SIGKILL)
    {
        // ESRCH just means it exited between the timeout firing and this call.
        debug!("killpg({pgid}) failed: {e}");
    }
    if let Err(e) = child.kill().await {
        debug!("failed to reap timed-out child: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    use claude_commander_protocol::github::CloneSource;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    use tempfile::TempDir;
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;

    /// Run a git command to completion, asserting it succeeded.
    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("git invocation failed to spawn");
        assert!(out.status.success(), "git {args:?} failed: {out:?}");
    }

    /// Seed a bare repo with one commit on `main`, network-free.
    ///
    /// Modelled on `git::auto_pull`'s `setup_origin_and_local`, and like it the
    /// returned path is a **plain local path**, not a `file://` URL — which is
    /// what a user pasting a path into the clone box would give us.
    fn seed_bare_repo(tmp: &TempDir) -> PathBuf {
        let remote = tmp.path().join("remote.git");
        let seed = tmp.path().join("seed");
        git(tmp.path(), &["init", "--bare", "-b", "main", "remote.git"]);
        git(tmp.path(), &["init", "-b", "main", "seed"]);
        git(&seed, &["config", "user.email", "t@t"]);
        git(&seed, &["config", "user.name", "t"]);
        git(&seed, &["config", "commit.gpgsign", "false"]);
        std::fs::write(seed.join("README"), "v1\n").unwrap();
        git(&seed, &["add", "README"]);
        git(&seed, &["commit", "-m", "initial"]);
        git(
            &seed,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&seed, &["push", "origin", "main"]);
        remote
    }

    #[tokio::test]
    async fn clones_from_a_local_bare_repo_without_network() {
        let tmp = TempDir::new().unwrap();
        let remote = seed_bare_repo(&tmp);
        let dest = tmp.path().join("out");

        run_clone(
            &CloneSource::Url {
                url: remote.to_string_lossy().into_owned(),
            },
            &dest,
            Duration::from_secs(60),
        )
        .await
        .unwrap();

        assert!(dest.join(".git").is_dir());
        assert_eq!(
            std::fs::read_to_string(dest.join("README")).unwrap(),
            "v1\n"
        );
    }

    #[tokio::test]
    async fn a_bad_source_fails_rather_than_hanging() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("out");

        let err = run_clone(
            &CloneSource::Url {
                url: tmp
                    .path()
                    .join("does-not-exist")
                    .to_string_lossy()
                    .into_owned(),
            },
            &dest,
            Duration::from_secs(60),
        )
        .await
        .unwrap_err();

        // git's own message, which is already phrased for a human.
        assert!(
            err.to_string().contains("does not exist"),
            "unhelpful error: {err}"
        );
        assert!(!dest.exists(), "a failed clone left a destination behind");
    }

    /// The cleanup removes only what this function created. A destination that
    /// was already there — someone else's checkout — must survive a failure,
    /// because `run_clone` cannot know it is disposable.
    #[tokio::test]
    async fn a_failed_clone_leaves_a_pre_existing_destination_alone() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("occupied");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(dest.join("precious"), "mine\n").unwrap();

        let err = run_clone(
            &CloneSource::Url {
                url: tmp
                    .path()
                    .join("does-not-exist")
                    .to_string_lossy()
                    .into_owned(),
            },
            &dest,
            Duration::from_secs(60),
        )
        .await
        .unwrap_err();

        assert!(!err.to_string().is_empty());
        assert_eq!(
            std::fs::read_to_string(dest.join("precious")).unwrap(),
            "mine\n",
            "cleanup deleted a destination it did not create"
        );
    }

    /// The timeout must actually *kill*, and kill the whole tree: `gh repo
    /// clone` spawns `git clone`, which spawns `ssh`/`git-remote-https`. Killing
    /// only the direct child would leave a live git writing into the very
    /// directory the failure path then tries to remove.
    ///
    /// Driven through the `run_bounded` seam with `sh` rather than `git`,
    /// because a clone that finished fast would prove nothing about either
    /// property.
    #[tokio::test]
    async fn a_timed_out_run_kills_the_child_and_its_descendants() {
        let tmp = TempDir::new().unwrap();
        let pidfile = tmp.path().join("grandchild.pid");

        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(format!("sleep 300 & echo $! > {}; wait", pidfile.display()));

        let started = Instant::now();
        let err = run_bounded(cmd, "sh", Duration::from_millis(300))
            .await
            .unwrap_err();
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(10),
            "run_bounded waited {elapsed:?} on a 300ms timeout"
        );
        assert!(
            err.to_string().contains("timed out"),
            "not reported as a timeout: {err}"
        );

        let grandchild: i32 = std::fs::read_to_string(&pidfile)
            .expect("sh never recorded its grandchild pid")
            .trim()
            .parse()
            .unwrap();

        // Poll: the kill is asynchronous, and the reparented `sleep` has to be
        // reaped by init before its pid stops resolving.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if kill(Pid::from_raw(grandchild), None).is_err() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "grandchild {grandchild} survived the timeout kill"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Write an executable script that records the fact it ran.
    fn write_canary(path: &Path, log: &Path) {
        std::fs::write(
            path,
            format!("#!/bin/sh\necho called >> {}\necho dummy\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Ask git for a credential it has no way to obtain, and report whether the
    /// askpass canary ran and what git said.
    ///
    /// `git credential fill` reaches git's *entire* prompting chain (askpass
    /// helper, then terminal) without touching the network, which is what makes
    /// this property testable at all. The user's own config is neutralised via
    /// `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` so a real credential helper on
    /// the developer's machine can neither answer the request nor pop a dialog.
    async fn probe_prompting(tmp: &TempDir, noninteractive: bool) -> (bool, String) {
        let canary = tmp.path().join("askpass.sh");
        let log = tmp.path().join("askpass.log");
        let _ = std::fs::remove_file(&log);
        write_canary(&canary, &log);
        let empty_config = tmp.path().join("empty.gitconfig");
        std::fs::write(&empty_config, "").unwrap();

        let mut cmd = Command::new("git");
        cmd.args(["credential", "fill"])
            .current_dir(tmp.path())
            .env("GIT_CONFIG_GLOBAL", &empty_config)
            .env("GIT_CONFIG_SYSTEM", &empty_config)
            // The canary stands in for whatever askpass helper the machine has:
            // git falls back to SSH_ASKPASS when GIT_ASKPASS is unset.
            .env("SSH_ASKPASS", &canary)
            .env_remove("GIT_ASKPASS")
            .env_remove("GIT_TERMINAL_PROMPT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if noninteractive {
            apply_noninteractive_env(&mut cmd);
        }

        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"protocol=https\nhost=example.invalid\n\n")
            .await
            .unwrap();
        let out = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
            .await
            .expect("git blocked on a prompt")
            .unwrap();

        (
            log.exists(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// The control arm: without the env recipe, git happily runs an askpass
    /// helper. Without this half, the test below would pass against a git that
    /// never prompts for unrelated reasons.
    #[tokio::test]
    async fn without_the_recipe_git_runs_an_askpass_helper() {
        let tmp = TempDir::new().unwrap();
        let (called, _stderr) = probe_prompting(&tmp, false).await;
        assert!(called, "askpass helper was not consulted at all");
    }

    /// The env recipe closes *both* prompting routes: the askpass helper is
    /// never consulted, and the terminal fallback is refused.
    #[tokio::test]
    async fn the_noninteractive_recipe_stops_git_prompting() {
        let tmp = TempDir::new().unwrap();
        let (called, stderr) = probe_prompting(&tmp, true).await;
        assert!(!called, "an askpass helper still ran: {stderr}");
        assert!(
            stderr.contains("terminal prompts disabled"),
            "git did not refuse the terminal prompt: {stderr}"
        );
    }

    /// A failing git echoes the source back, and a hand-typed source may carry
    /// `user:token@`. That string becomes `CloneStatus::Failed { message }`,
    /// crosses the wire and is rendered in a UI, so the secret must not survive
    /// contact with the error type — "remember not to log this" does not
    /// survive four more tasks.
    ///
    /// Driven through `run_bounded` with a stub, because a genuine credentialed
    /// clone would need the network. The stub's wording is *modelled on* git's
    /// HTTPS auth failure from memory, not captured from a run — what this test
    /// pins is the redaction, not git's phrasing. Only the shape it depends on
    /// is verified: git echoes the source URL in failure messages (git 2.53.0,
    /// `git clone -- /nonexistent /tmp/x` → `fatal: repository
    /// '/nonexistent' does not exist`).
    #[tokio::test]
    async fn a_failure_message_carries_no_credentials() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(
            "echo \"fatal: Authentication failed for \
             'https://sizeak:ghp_s3cr3tt0ken@github.com/o/r.git/'\" >&2; exit 128",
        );

        let err = run_bounded(cmd, "git clone", Duration::from_secs(60))
            .await
            .unwrap_err()
            .to_string();

        assert!(!err.contains("ghp_s3cr3tt0ken"), "token leaked: {err}");
        assert!(err.contains("https://***@github.com/o/r.git/"), "{err}");
        // Still a usable diagnosis.
        assert!(err.contains("Authentication failed"), "{err}");
    }

    #[test]
    fn ssh_command_carries_batch_mode_and_preserves_an_inherited_one() {
        assert_eq!(ssh_command(None), "ssh -o BatchMode=yes");
        assert_eq!(ssh_command(Some("   ")), "ssh -o BatchMode=yes");
        assert_eq!(
            ssh_command(Some("ssh -i /keys/work")),
            "ssh -i /keys/work -o BatchMode=yes"
        );
    }

    /// The `--` is load-bearing on both arms and lives right before the source.
    #[test]
    fn both_arms_terminate_options_before_the_source() {
        let dest = Path::new("/projects/repo");

        let gh = clone_command(
            &CloneSource::Github {
                full_name: "o/r".to_string(),
            },
            dest,
        );
        let args: Vec<_> = gh.as_std().get_args().collect();
        assert_eq!(gh.as_std().get_program(), "gh");
        assert_eq!(args, ["repo", "clone", "--", "o/r", "/projects/repo"]);

        let git = clone_command(
            &CloneSource::Url {
                url: "https://example.com/o/r.git".to_string(),
            },
            dest,
        );
        let args: Vec<_> = git.as_std().get_args().collect();
        assert_eq!(git.as_std().get_program(), "git");
        assert_eq!(
            args,
            [
                "clone",
                "--",
                "https://example.com/o/r.git",
                "/projects/repo"
            ]
        );
    }

    /// The validators run again here, on the last stop before argv. A caller
    /// that forgot to validate must not be able to hand git a flag.
    #[tokio::test]
    async fn a_flag_shaped_source_is_refused_before_any_process_starts() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("out");

        for source in [
            CloneSource::Url {
                url: "--upload-pack=evil".to_string(),
            },
            CloneSource::Github {
                full_name: "-flag/name".to_string(),
            },
        ] {
            let err = run_clone(&source, &dest, Duration::from_secs(60))
                .await
                .unwrap_err();
            assert!(!err.to_string().is_empty(), "source: {source:?}");
            assert!(!dest.exists());
        }
    }

    /// Rejecting a source must not quote the secret back.
    ///
    /// The `Github` arm is the sharp one: `validate_repo_slug` splits on `/`, so
    /// a URL pasted into the slug field always fails with
    /// `CloneRejection::MalformedSlug`, whose `Display` echoes the **whole raw
    /// input** — the entire credentialed URL. That rejection message is a `400`
    /// body and a `CloneStatus::Failed` message, so the same four-hop journey
    /// applies to it as to git's stderr.
    #[tokio::test]
    async fn a_rejection_message_carries_no_credentials() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("out");
        const SECRET: &str = "ghp_s3cr3tt0ken";

        // `echoes_source` records whether the rejection quotes the input back at
        // all. Only `MalformedSlug` does — no `validate_clone_url` rejection
        // carries more than a scheme or a directory name, which is why the `Url`
        // arm is here as a regression guard rather than as a reproduction.
        for (source, echoes_source) in [
            (
                // A URL pasted where a slug belongs — the leak path.
                CloneSource::Github {
                    full_name: format!("https://sizeak:{SECRET}@github.com/o/r"),
                },
                true,
            ),
            (
                CloneSource::Url {
                    url: format!("ftp://sizeak:{SECRET}@example.com/o/r"),
                },
                false,
            ),
        ] {
            let err = run_clone(&source, &dest, Duration::from_secs(60))
                .await
                .unwrap_err()
                .to_string();

            assert!(!err.contains(SECRET), "token leaked: {err}");
            assert!(!err.contains("sizeak:"), "userinfo leaked: {err}");
            if echoes_source {
                // Redacted, not swallowed: the user still has to be able to see
                // which source was refused.
                assert!(err.contains("***@github.com/o/r"), "unusable: {err}");
            }
            assert!(!dest.exists());
        }
    }
}
