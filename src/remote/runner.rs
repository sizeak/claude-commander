//! Transport-agnostic remote command runner.
//!
//! [`RemoteRunner`] abstracts "how do we run an `argv` on a remote host?"
//! Two impls live as siblings:
//! - [`OpensshRunner`] — pooled `openssh::Session`, ControlMaster-multiplexed.
//!   Used for `RemoteTransport::Ssh`.
//! - [`GhCodespaceRunner`] — `gh codespace ssh -c <name> --` per command,
//!   one process per call. Slower but works without modifying the user's
//!   `~/.ssh/config`. Used for `RemoteTransport::Codespace`.
//!
//! Both runners capture the login-shell environment once on first use
//! (via `bash -lc 'env -0'`) and replay it as a prefix for every later
//! command. That way Nix / asdf / mise / custom-shim PATH gets set up
//! exactly once per session instead of every command paying the
//! profile-sourcing cost (often 100–500ms).

use std::process::ExitStatus;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use openssh::Session;
use shell_escape::unix::escape;
use tokio::process::Command;
use tokio::sync::OnceCell;
use tracing::{debug, warn};

use crate::error::{GitError, Result};

/// Output from a single remote command run.
pub struct RemoteOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Run an `argv` on a remote host.
#[async_trait]
pub trait RemoteRunner: Send + Sync {
    async fn run(&self, argv: &[&str]) -> Result<RemoteOutput>;
}

/// Variables we explicitly drop from the captured login-shell env when
/// replaying — they're either session-local (PWD, OLDPWD, SHLVL, _) or
/// tied to the SSH/GH session that captured them and would be wrong
/// (or stale) on subsequent invocations.
const SKIP_REPLAY: &[&str] = &["_", "PWD", "OLDPWD", "SHLVL"];

fn should_replay_var(name: &str) -> bool {
    if SKIP_REPLAY.contains(&name) {
        return false;
    }
    if name.starts_with("SSH_") {
        return false;
    }
    if name.starts_with("BASH_FUNC_") {
        return false;
    }
    true
}

/// Parse `env -0` output: NUL-separated `KEY=VALUE` pairs.
fn parse_env_z(bytes: &[u8]) -> Vec<(String, String)> {
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .filter_map(|s| {
            let s = String::from_utf8_lossy(s).into_owned();
            let eq = s.find('=')?;
            Some((s[..eq].to_string(), s[eq + 1..].to_string()))
        })
        .filter(|(k, _)| should_replay_var(k))
        .collect()
}

/// Build an `env K=V K=V ... cmd args` script with proper single-quote
/// escaping. We use `env` (without `-i`) so the new sshd-set vars
/// (SSH_AUTH_SOCK, etc.) come through; captured vars override on overlap.
///
/// We deliberately skip the `--` option terminator after the K=V pairs.
/// Some env implementations (older GNU coreutils, BSD env, busybox) don't
/// recognize `--` and try to exec it as a program, failing with
/// `env: '--': No such file or directory`. POSIX env scans args left-to-right
/// and treats the first arg without `=` as the command name, so the boundary
/// is already unambiguous as long as our captured keys all contain `=`.
fn build_env_replay_script(env: &[(String, String)], argv: &[&str]) -> String {
    let mut s = String::from("env");
    for (k, v) in env {
        s.push(' ');
        s.push_str(k);
        s.push('=');
        s.push_str(&escape(v.as_str().into()));
    }
    for a in argv {
        s.push(' ');
        s.push_str(&escape((*a).into()));
    }
    s
}

/// Whether a stderr from `gh codespace ssh` looks transient enough to retry.
fn is_transient_gh_error(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("deadlineexceeded")
        || s.contains("context deadline exceeded")
        || s.contains("connection reset")
        || s.contains("connection refused")
        || s.contains("i/o timeout")
}

/// Runner that dispatches via a pooled `openssh::Session`.
pub struct OpensshRunner {
    session: Arc<Session>,
    env: OnceCell<Vec<(String, String)>>,
}

impl OpensshRunner {
    pub fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            env: OnceCell::new(),
        }
    }

    /// One-time login-shell env capture. Cached for the lifetime of the
    /// runner; subsequent commands replay it cheaply via `env K=V cmd`.
    pub async fn captured_env(&self) -> Result<&Vec<(String, String)>> {
        self.env
            .get_or_try_init(|| async {
                debug!("openssh: capturing login-shell env");
                let mut cmd = self.session.command("bash");
                cmd.arg("-lc").arg("env -0");
                let output = cmd
                    .output()
                    .await
                    .map_err(|e| GitError::WorktreeError(format!("env capture: {}", e)))?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(
                        GitError::WorktreeError(format!("env capture failed: {}", stderr)).into(),
                    );
                }
                Ok(parse_env_z(&output.stdout))
            })
            .await
    }
}

#[async_trait]
impl RemoteRunner for OpensshRunner {
    async fn run(&self, argv: &[&str]) -> Result<RemoteOutput> {
        if argv.is_empty() {
            return Err(GitError::WorktreeError(
                "RemoteRunner::run called with empty argv".to_string(),
            )
            .into());
        }
        let env = self.captured_env().await?;
        let script = build_env_replay_script(env, argv);
        debug!("openssh exec (env replay): {}", script);
        let mut cmd = self.session.command("bash");
        cmd.arg("-c").arg(&script);
        let output = cmd
            .output()
            .await
            .map_err(|e| GitError::WorktreeError(format!("ssh exec failed: {}", e)))?;
        Ok(RemoteOutput {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

/// Runner that wraps each command in `gh codespace ssh -c <name> --`.
pub struct GhCodespaceRunner {
    codespace: String,
    env: OnceCell<Vec<(String, String)>>,
}

impl GhCodespaceRunner {
    pub fn new(codespace: impl Into<String>) -> Self {
        Self {
            codespace: codespace.into(),
            env: OnceCell::new(),
        }
    }

    /// One-time login-shell env capture. Retries on transient gh RPC
    /// failures (DeadlineExceeded, connection reset, etc.) which often
    /// happen right after a codespace rebuild while sshd inside the
    /// container is still warming up.
    pub async fn captured_env(&self) -> Result<&Vec<(String, String)>> {
        self.env
            .get_or_try_init(|| async {
                debug!(
                    "gh codespace: capturing login-shell env for {}",
                    self.codespace
                );
                let mut delay = Duration::from_secs(2);
                let mut last_stderr = String::new();

                // `gh codespace ssh -- arg1 arg2 …` forwards via ssh, which
                // space-joins remote args without quoting. So we have to pack
                // the bash invocation into a single shell-escaped arg —
                // otherwise the remote shell tokenizes `bash -ilc env -0` into
                // bash COMMAND_STRING="env" with $0="-0", which silently runs
                // `env` instead of `env -0` and produces unparseable output.
                //
                // `-ilc` (interactive login) so .bashrc is sourced too —
                // Nix-based codespaces typically only put their bin dirs on
                // PATH from .bashrc, not from .bash_profile/.profile.
                let env_capture_arg = format!("bash -ilc {}", escape("env -0".into()));
                for attempt in 0..3 {
                    let output = Command::new("gh")
                        .args([
                            "codespace",
                            "ssh",
                            "-c",
                            &self.codespace,
                            "--",
                            &env_capture_arg,
                        ])
                        .output()
                        .await
                        .map_err(|e| GitError::WorktreeError(format!("env capture exec: {}", e)))?;

                    if output.status.success() {
                        return Ok(parse_env_z(&output.stdout));
                    }

                    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                    if attempt < 2 && is_transient_gh_error(&stderr) {
                        warn!(
                            "env capture attempt {} hit transient error ({}); retrying in {:?}",
                            attempt + 1,
                            stderr.trim(),
                            delay
                        );
                        tokio::time::sleep(delay).await;
                        delay *= 2;
                        last_stderr = stderr;
                        continue;
                    }
                    return Err(
                        GitError::WorktreeError(format!("env capture failed: {}", stderr)).into(),
                    );
                }

                Err(GitError::WorktreeError(format!(
                    "env capture: gave up after 3 transient failures ({})",
                    last_stderr
                ))
                .into())
            })
            .await
    }
}

#[async_trait]
impl RemoteRunner for GhCodespaceRunner {
    async fn run(&self, argv: &[&str]) -> Result<RemoteOutput> {
        if argv.is_empty() {
            return Err(GitError::WorktreeError(
                "RemoteRunner::run called with empty argv".to_string(),
            )
            .into());
        }
        let env = self.captured_env().await?;
        let script = build_env_replay_script(env, argv);
        debug!("gh codespace exec (env replay): {}", script);
        // Pack into a single shell-escaped arg so neither gh nor ssh's
        // argv-joining can re-tokenize the script (see `captured_env`).
        let one_arg = format!("bash -c {}", escape(script.as_str().into()));
        let mut cmd = Command::new("gh");
        cmd.args(["codespace", "ssh", "-c", &self.codespace, "--", &one_arg]);
        let output = cmd.output().await.map_err(|e| {
            GitError::WorktreeError(format!(
                "gh codespace ssh -c {} -- {:?} failed: {}",
                self.codespace, one_arg, e
            ))
        })?;
        Ok(RemoteOutput {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}
