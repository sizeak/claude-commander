//! Helpers around the `gh codespace` CLI.
//!
//! All functions here shell out to `gh`. They surface a `not installed`
//! hint via the error message when `gh` is absent.

use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;
use tokio::time::sleep;
use tracing::{debug, info, instrument, warn};

use crate::error::{GitError, Result};

/// Codespace lifecycle state, as reported by `gh codespace list/view`.
///
/// Common values: `Available`, `Shutdown`, `Starting`, `Provisioning`,
/// `Failed`, `Queued`. We treat anything we don't explicitly recognize as
/// `Other` so future gh additions don't break parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodespaceState {
    Available,
    Shutdown,
    Starting,
    Provisioning,
    Other(String),
}

impl CodespaceState {
    pub fn parse(s: &str) -> Self {
        match s {
            "Available" => Self::Available,
            "Shutdown" => Self::Shutdown,
            "Starting" => Self::Starting,
            "Provisioning" => Self::Provisioning,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Self::Available)
    }
}

/// Subset of `gh codespace list --json` we care about.
#[derive(Debug, Clone)]
pub struct CodespaceInfo {
    pub name: String,
    pub repository: String,
    pub state: CodespaceState,
    pub display_name: Option<String>,
    pub git_status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawCodespace {
    name: String,
    #[serde(default)]
    repository: String,
    #[serde(default)]
    state: String,
    #[serde(rename = "displayName", default)]
    display_name: Option<String>,
    #[serde(rename = "gitStatus", default)]
    git_status: Option<RawGitStatus>,
}

#[derive(Debug, Deserialize)]
struct RawGitStatus {
    #[serde(default)]
    #[serde(rename = "ref")]
    branch: Option<String>,
}

impl From<RawCodespace> for CodespaceInfo {
    fn from(r: RawCodespace) -> Self {
        Self {
            name: r.name,
            repository: r.repository,
            state: CodespaceState::parse(&r.state),
            display_name: r.display_name,
            git_status: r.git_status.and_then(|g| g.branch),
        }
    }
}

/// Run `gh ...` and return stdout on success.
async fn run_gh(args: &[&str]) -> Result<String> {
    debug!("gh {}", args.join(" "));
    let output = Command::new("gh").args(args).output().await.map_err(|e| {
        GitError::WorktreeError(format!(
            "gh not found or not executable ({}). Install GitHub CLI and run `gh auth login`.",
            e
        ))
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(
            GitError::WorktreeError(format!("gh {} failed: {}", args.join(" "), stderr)).into(),
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Public wrapper so callers outside this module can shell `gh` for ad-hoc
/// commands (e.g. polling status during a long-running codespace create).
pub async fn gh(args: &[&str]) -> Result<String> {
    run_gh(args).await
}

/// List the user's codespaces.
#[instrument]
pub async fn gh_codespace_list() -> Result<Vec<CodespaceInfo>> {
    let stdout = run_gh(&[
        "codespace",
        "list",
        "--json",
        "name,repository,state,displayName,gitStatus",
    ])
    .await?;
    let raw: Vec<RawCodespace> = serde_json::from_str(&stdout)
        .map_err(|e| GitError::WorktreeError(format!("parse `gh codespace list` JSON: {}", e)))?;
    Ok(raw.into_iter().map(Into::into).collect())
}

/// Look up a single codespace's current state.
#[instrument]
pub async fn gh_codespace_view(name: &str) -> Result<CodespaceInfo> {
    let stdout = run_gh(&[
        "codespace",
        "view",
        "-c",
        name,
        "--json",
        "name,repository,state,displayName,gitStatus",
    ])
    .await?;
    let raw: RawCodespace = serde_json::from_str(&stdout)
        .map_err(|e| GitError::WorktreeError(format!("parse `gh codespace view` JSON: {}", e)))?;
    Ok(raw.into())
}

/// Whether a `gh codespace ssh` stderr looks like a transient failure that
/// retrying might fix. Common when sshd inside the container is still
/// starting after a rebuild — gh's RPC times out before the remote SSH
/// listener is ready.
fn is_transient_codespace_error(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("deadlineexceeded")
        || s.contains("context deadline exceeded")
        || s.contains("connection reset")
        || s.contains("connection refused")
        || s.contains("i/o timeout")
}

/// Wake a codespace and wait until SSH actually works.
///
/// The CLI has no `gh codespace start` — codespaces auto-resume when you
/// SSH into them — so we trigger the wake by running a trivial
/// `gh codespace ssh -c <name> -- true`. Two reasons this can fail
/// transiently and need retrying:
///
/// 1. The codespace is `Shutdown` and gh's first SSH attempt times out
///    while it's booting (typically 10–60s).
/// 2. The codespace is `Available` per `gh codespace view`, but sshd
///    inside the container is still coming up (especially right after a
///    rebuild). gh's underlying RPC times out and you get
///    `DeadlineExceeded`.
///
/// We retry up to 3 times with exponential backoff (2s, 4s, 8s) before
/// surfacing the error. Non-transient failures (no sshd installed,
/// auth missing, etc.) bail immediately.
#[instrument]
pub async fn gh_codespace_wake(name: &str, _timeout: Duration) -> Result<CodespaceInfo> {
    info!("Waking codespace {} via gh codespace ssh", name);
    let mut delay = Duration::from_secs(2);
    let mut last_err: Option<String> = None;

    for attempt in 0..3 {
        let ssh_result = Command::new("gh")
            .args(["codespace", "ssh", "-c", name, "--", "true"])
            .output()
            .await
            .map_err(|e| GitError::WorktreeError(format!("gh codespace ssh: {}", e)))?;

        if ssh_result.status.success() {
            // The trivial SSH worked → sshd is up and accepting commands.
            // Confirm state before returning so callers get useful metadata.
            return gh_codespace_view(name).await;
        }

        let stderr = String::from_utf8_lossy(&ssh_result.stderr).into_owned();
        if attempt < 2 && is_transient_codespace_error(&stderr) {
            warn!(
                "gh codespace ssh attempt {} hit transient error ({}); retrying in {:?}",
                attempt + 1,
                stderr.trim(),
                delay
            );
            tokio::time::sleep(delay).await;
            delay *= 2;
            last_err = Some(stderr);
            continue;
        }

        // Non-transient or out of retries.
        return Err(GitError::WorktreeError(format!("gh codespace ssh failed: {}", stderr)).into());
    }

    Err(GitError::WorktreeError(format!(
        "gh codespace ssh: gave up after 3 transient failures ({})",
        last_err.unwrap_or_default()
    ))
    .into())
}

/// Create a brand-new codespace from `owner/repo`, optionally pinned to a
/// branch. Polls until it's `Available`. Times out after `timeout`.
#[instrument]
pub async fn gh_codespace_create(
    repo: &str,
    branch: Option<&str>,
    timeout: Duration,
) -> Result<CodespaceInfo> {
    info!("Creating codespace for {} (branch: {:?})", repo, branch);
    let mut args: Vec<&str> = vec!["codespace", "create", "-r", repo];
    if let Some(b) = branch {
        args.push("-b");
        args.push(b);
    }
    let stdout = run_gh(&args).await?;
    // `gh codespace create` prints the new codespace name on its own line.
    let name = stdout
        .lines()
        .last()
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return Err(
            GitError::WorktreeError("gh codespace create returned empty name".to_string()).into(),
        );
    }
    wait_for_state(&name, CodespaceState::Available, timeout).await
}

async fn wait_for_state(
    name: &str,
    target: CodespaceState,
    timeout: Duration,
) -> Result<CodespaceInfo> {
    let started = std::time::Instant::now();
    let mut last_state = CodespaceState::Other("unknown".into());
    while started.elapsed() < timeout {
        match gh_codespace_view(name).await {
            Ok(info) => {
                last_state = info.state.clone();
                if info.state == target {
                    return Ok(info);
                }
                debug!("codespace {} state: {:?}, waiting...", name, info.state);
            }
            Err(e) => {
                debug!("codespace view error (will retry): {}", e);
            }
        }
        sleep(Duration::from_secs(2)).await;
    }
    Err(GitError::WorktreeError(format!(
        "timed out waiting for codespace {} to reach {:?} (last state: {:?})",
        name, target, last_state
    ))
    .into())
}
