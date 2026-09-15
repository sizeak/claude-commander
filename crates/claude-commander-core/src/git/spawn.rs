//! The one way core spawns `git` and `gh`: detached from the operator's
//! terminal, with every prompting route closed.
//!
//! # Why this exists
//!
//! The TUI is a full-screen program that owns its terminal, and it spawns `git`
//! and `gh` constantly — worktree ops, diffs, the hourly auto-pull, PR status.
//! Every one of those children inherits the TUI's **controlling terminal**, and
//! a child that decides to ask a question asks it *there*: OpenSSH's
//! `read_passphrase` opens `/dev/tty` and reads it byte by byte until a newline;
//! git's own credential prompt does the same. Nothing about that read is visible
//! to the TUI, which is drawing an alternate screen over the prompt, and the
//! terminal is in raw mode, so Enter is `\r`, never the `\n` ssh is waiting for.
//!
//! The result is a third reader on the terminal, competing with the TUI (or the
//! attach pump) for every burst: keystrokes vanish into the prompt, the ones
//! that get through arrive with their escape sequences split, mouse reports
//! parse as `<`/`>` and resize the session list, and Ctrl+Q reaches the attach
//! only every other press. It looked like a suspend/resume bug because the
//! triggers were "1Password locked the vault" (screen lock, sleep) and "the
//! 1Password agent asked for confirmation" (its dialog) — both make the agent
//! decline, ssh falls back to an identity file or a host-key confirmation, and
//! prompts. Found live: `ssh aur@aur.archlinux.org` under the auto-pull's
//! `git fetch`, fd 5 → `/dev/tty`, blocked in a tty read for 205 s.
//!
//! Two defences, both required:
//!
//! 1. **The environment recipe** ([`apply_noninteractive_env`]) tells git and
//!    ssh not to prompt at all — `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=""`,
//!    `BatchMode=yes`. This is what `clone.rs` already did for a clone; it now
//!    applies to every spawn. Fast, legible failures instead of hangs.
//! 2. **No controlling terminal** ([`detach_controlling_terminal`], run in the
//!    child between fork and exec). A program that ignores the recipe — a
//!    credential helper, `gh`, some future tool — finds that `open("/dev/tty")`
//!    fails with `ENXIO`, because there is no terminal to open. This is the one
//!    that holds by construction rather than by cooperation.
//!
//! Interactive git — the operator's own shell inside a tmux pane — is not
//! spawned through here and is unaffected.
//!
//! # Why not `setsid`
//!
//! `setsid(2)` would also drop the controlling terminal, but it fails with
//! `EPERM` for a process-group leader, and std makes the child one *before*
//! running `pre_exec` closures whenever `process_group` was set
//! (`library/std/src/sys/process/unix/unix.rs:341` `setpgid`, `:382` the
//! closures) — which [`super::bounded`] does so it can kill a timed-out tree.
//! `TIOCNOTTY` ("give up this controlling terminal", `ioctl_tty(2)`) has no
//! such precondition, and a non-session-leader loses only its own terminal —
//! no `SIGHUP` to anyone. `children_have_no_controlling_terminal` pins it.

use std::ffi::OsStr;
use std::io;
use std::process::Stdio;

/// A `git` command that can never stop to ask the operator anything.
pub fn git_command() -> tokio::process::Command {
    detached("git")
}

/// A `gh` command that can never stop to ask the operator anything.
pub fn gh_command() -> tokio::process::Command {
    detached("gh")
}

/// [`git_command`] for the few synchronous call sites.
pub fn git_command_std() -> std::process::Command {
    detached_std("git")
}

/// Any program, spawned with the full recipe: stdin from `/dev/null`, the
/// no-prompt environment, and no controlling terminal.
pub fn detached(program: impl AsRef<OsStr>) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    cmd.stdin(Stdio::null());
    apply_noninteractive_env(&mut cmd);
    // SAFETY: `detach_controlling_terminal` calls only `open`, `ioctl` and
    // `close`, all async-signal-safe, and touches no memory shared with the
    // parent — the contract `pre_exec` asks for.
    unsafe {
        cmd.pre_exec(detach_controlling_terminal);
    }
    cmd
}

/// [`detached`] for `std::process::Command`.
pub fn detached_std(program: impl AsRef<OsStr>) -> std::process::Command {
    use std::os::unix::process::CommandExt;

    let mut cmd = std::process::Command::new(program);
    cmd.stdin(Stdio::null());
    apply_noninteractive_env(&mut cmd);
    // SAFETY: as in `detached`.
    unsafe {
        cmd.pre_exec(detach_controlling_terminal);
    }
    cmd
}

/// Runs in the child between `fork` and `exec`: give up the controlling
/// terminal so that nothing this process (or its descendants) runs can open
/// `/dev/tty`. A process that already has none is left alone.
///
/// Only `open`, `ioctl` and `close` — nothing that allocates or locks.
fn detach_controlling_terminal() -> io::Result<()> {
    use nix::libc;

    // O_NOCTTY: the open itself must not *acquire* a terminal on a system
    // where opening one would (we never are the session leader here, so it
    // would not, but say so).
    let fd = unsafe {
        libc::open(
            c"/dev/tty".as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        // ENXIO: no controlling terminal — already what we want.
        return Ok(());
    }
    unsafe {
        libc::ioctl(fd, libc::TIOCNOTTY);
        libc::close(fd);
    }
    Ok(())
}

/// Something with an environment we can set — the two `Command` types.
pub(crate) trait SpawnEnv {
    fn set_env(&mut self, key: &str, value: &str);
}

impl SpawnEnv for tokio::process::Command {
    fn set_env(&mut self, key: &str, value: &str) {
        self.env(key, value);
    }
}

impl SpawnEnv for std::process::Command {
    fn set_env(&mut self, key: &str, value: &str) {
        self.env(key, value);
    }
}

/// Close every route by which git (or the ssh it spawns) could stop and ask a
/// question. `gh` gets it too: `gh repo clone` shells out to `git clone` (its
/// help: "Pass additional `git clone` flags by listing them after `--`"), and a
/// child process inherits this environment.
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
pub(crate) fn apply_noninteractive_env(cmd: &mut impl SpawnEnv) {
    cmd.set_env("GIT_TERMINAL_PROMPT", "0");
    cmd.set_env("GIT_ASKPASS", "");
    cmd.set_env(
        "GIT_SSH_COMMAND",
        &ssh_command(std::env::var("GIT_SSH_COMMAND").ok().as_deref()),
    );
}

/// The ssh command git should use, carrying `BatchMode=yes`.
///
/// `BatchMode=yes`: "If set to yes, user interaction such as password prompts
/// and host key confirmation requests will be disabled." (`ssh_config(5)`,
/// OpenSSH 10.4p1) — the two ways an ssh fetch or clone hangs unattended.
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
/// a fast, legible auth failure instead of a fetch that hangs. `clone.rs`'s
/// module docs weigh this trade in full.
pub(crate) fn ssh_command(inherited: Option<&str>) -> String {
    match inherited {
        Some(cmd) if !cmd.trim().is_empty() => format!("{} -o BatchMode=yes", cmd.trim()),
        _ => "ssh -o BatchMode=yes".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::Duration;

    use tempfile::TempDir;
    use tokio::io::AsyncWriteExt;

    /// `tty_nr` is field 7 of `/proc/<pid>/stat` (`proc_pid_stat(5)`): the
    /// controlling terminal, 0 when there is none. Read from inside the child so
    /// it is the child's own view.
    fn tty_nr_of(stat: &str) -> i64 {
        // The comm field (2) is parenthesised and may contain spaces; split
        // after its closing paren.
        let rest = &stat[stat.rfind(')').expect("stat has a comm field") + 2..];
        rest.split_whitespace()
            .nth(4) // fields 3..: state, ppid, pgrp, session, tty_nr
            .expect("tty_nr field")
            .parse()
            .expect("tty_nr is numeric")
    }

    fn own_tty_nr() -> i64 {
        tty_nr_of(&std::fs::read_to_string("/proc/self/stat").unwrap())
    }

    /// Set by [`detach_holds_under_a_real_controlling_terminal`] on the inner
    /// run it drives, so the inner test can insist it really was given a tty.
    const INNER_RUN: &str = "CC_SPAWN_TEST_INNER";

    /// The property that holds by construction: a child spawned through here
    /// has no controlling terminal, so `open("/dev/tty")` cannot succeed in it
    /// or anything it runs. On its own this is only meaningful when the test
    /// process *has* a terminal (a developer's `cargo test`; a CI runner reads 0
    /// on both sides) — which is why the outer test below re-runs it under a
    /// pty it makes the controlling terminal.
    #[tokio::test]
    async fn children_have_no_controlling_terminal() {
        if std::env::var_os(INNER_RUN).is_some() {
            assert_ne!(own_tty_nr(), 0, "the outer test promised us a terminal");
        }
        let out = detached("cat")
            .arg("/proc/self/stat")
            .output()
            .await
            .unwrap();
        let child = tty_nr_of(&String::from_utf8_lossy(&out.stdout));
        assert_eq!(
            child,
            0,
            "child kept a controlling terminal (ours is {})",
            own_tty_nr()
        );

        // And a grandchild, which is where ssh sits under git.
        let out = detached("sh")
            // A subshell: a failed `exec` redirection is fatal in non-interactive sh.
            .args([
                "-c",
                "( : </dev/tty ) 2>/dev/null && echo TTY || echo NOTTY",
            ])
            .output()
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "NOTTY");
    }

    /// Run `children_have_no_controlling_terminal` in a child that genuinely
    /// has a controlling terminal — a fresh pty made its ctty with `setsid` +
    /// `TIOCSCTTY` — so the "no tty in the grandchild" assertion is exercised
    /// for real on every runner, not just on a developer's terminal.
    #[test]
    fn detach_holds_under_a_real_controlling_terminal() {
        use std::io::Read;
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;

        let pty = nix::pty::openpty(None, None).expect("openpty");
        let slave_fd = pty.slave.as_raw_fd();
        let mut master = std::fs::File::from(pty.master);

        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "git::spawn::tests::children_have_no_controlling_terminal",
            "--test-threads=1",
            "--nocapture",
        ])
        .env(INNER_RUN, "1")
        .stdin(std::process::Stdio::from(pty.slave.try_clone().unwrap()))
        .stdout(std::process::Stdio::from(pty.slave.try_clone().unwrap()))
        .stderr(std::process::Stdio::from(pty.slave));
        // SAFETY: setsid/ioctl only, on descriptors the child owns.
        unsafe {
            cmd.pre_exec(move || {
                if nix::libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                if nix::libc::ioctl(slave_fd, nix::libc::TIOCSCTTY, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn().expect("spawn the inner test run");
        // `Command` keeps the `Stdio` descriptors until it is dropped; drop it so
        // the child holds the only slave fds and the master reads EOF (EIO)
        // once the child exits.
        drop(cmd);
        let mut transcript = String::new();
        let _ = master.read_to_string(&mut transcript);
        let status = child.wait().unwrap();
        assert!(
            status.success(),
            "inner run failed under a controlling terminal:\n{transcript}"
        );
        assert!(
            transcript.contains("test result: ok. 1 passed"),
            "inner run did not execute the test:\n{transcript}"
        );
    }

    #[test]
    fn the_std_variant_detaches_too() {
        let out = detached_std("cat").arg("/proc/self/stat").output().unwrap();
        assert_eq!(tty_nr_of(&String::from_utf8_lossy(&out.stdout)), 0);
    }

    #[tokio::test]
    async fn the_recipe_reaches_the_child_environment() {
        let out = detached("sh")
            .args([
                "-c",
                r#"printf '%s|%s|%s' "$GIT_TERMINAL_PROMPT" "$GIT_ASKPASS" "$GIT_SSH_COMMAND""#,
            ])
            .output()
            .await
            .unwrap();
        let env = String::from_utf8_lossy(&out.stdout);
        let mut parts = env.split('|');
        assert_eq!(parts.next(), Some("0"));
        assert_eq!(parts.next(), Some(""));
        assert!(
            parts.next().unwrap_or("").ends_with("-o BatchMode=yes"),
            "GIT_SSH_COMMAND not carried: {env}"
        );
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
    /// Plain `Command`, not [`detached`]: this probes the *recipe* on its own.
    async fn probe_prompting(tmp: &TempDir, noninteractive: bool) -> (bool, String) {
        let canary = tmp.path().join("askpass.sh");
        let log = tmp.path().join("askpass.log");
        let _ = std::fs::remove_file(&log);
        write_canary(&canary, &log);
        let empty_config = tmp.path().join("empty.gitconfig");
        std::fs::write(&empty_config, "").unwrap();

        let mut cmd = tokio::process::Command::new("git");
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

    #[test]
    fn ssh_command_carries_batch_mode_and_preserves_an_inherited_one() {
        assert_eq!(ssh_command(None), "ssh -o BatchMode=yes");
        assert_eq!(ssh_command(Some("   ")), "ssh -o BatchMode=yes");
        assert_eq!(
            ssh_command(Some("ssh -i /keys/work")),
            "ssh -i /keys/work -o BatchMode=yes"
        );
    }
}
