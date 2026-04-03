//! AI-generated branch summaries via the Claude CLI.
//!
//! Pipes the diff text into `claude --print` via stdin to generate a brief
//! summary of changes. Uses Haiku by default for token efficiency.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::debug;

/// AI summary state for a session, cached in `AppUiState`.
#[derive(Debug, Clone)]
pub enum AiSummary {
    /// Summary is being generated.
    Loading,
    /// Summary generated successfully.
    Ready {
        text: String,
        /// Hash of the diff text used to generate this summary (for staleness detection).
        diff_hash: u64,
    },
    /// Summary generation failed.
    Error(String),
}

/// Compute the full branch diff: committed changes vs main + uncommitted working changes.
///
/// Runs `git diff <main_branch>...HEAD` (committed) and `git diff HEAD` (uncommitted)
/// and concatenates them. This gives a complete picture of all changes on the branch.
pub async fn compute_branch_diff(worktree_path: &Path, main_branch: &str) -> String {
    let (committed, uncommitted) = tokio::join!(
        // Committed changes vs main branch
        Command::new("git")
            .current_dir(worktree_path)
            .args(["diff", &format!("{main_branch}...HEAD")])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
        // Uncommitted working changes
        Command::new("git")
            .current_dir(worktree_path)
            .args(["diff", "HEAD"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    );

    let mut diff = String::new();

    if let Ok(output) = committed
        && output.status.success()
    {
        diff.push_str(&String::from_utf8_lossy(&output.stdout));
    }

    if let Ok(output) = uncommitted
        && output.status.success()
    {
        let uncommitted_text = String::from_utf8_lossy(&output.stdout);
        if !uncommitted_text.is_empty() && !diff.is_empty() {
            diff.push('\n');
        }
        diff.push_str(&uncommitted_text);
    }

    diff
}

/// Generate a brief summary of changes by piping `diff_text` into the Claude CLI.
///
/// Returns the summary text on success, or an error message on failure.
/// Times out after 60 seconds. Skips the Claude call entirely if the diff is empty.
pub async fn fetch_branch_summary(diff_text: &str, model: &str) -> Result<String, String> {
    if diff_text.trim().is_empty() {
        return Ok("No changes on this branch.".to_string());
    }

    let result = tokio::time::timeout(
        Duration::from_secs(60),
        run_claude_summary(diff_text, model),
    )
    .await;

    match result {
        Ok(inner) => inner,
        Err(_) => Err("timed out".to_string()),
    }
}

async fn run_claude_summary(diff_text: &str, model: &str) -> Result<String, String> {
    let mut child = Command::new("claude")
        .args([
            "--model",
            model,
            "--print",
            "Summarize these changes in 2-3 sentences. Focus on what was changed and why.",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run claude: {e}"))?;

    // Write diff to stdin
    if let Some(mut stdin) = child.stdin.take() {
        // Truncate very large diffs to avoid overwhelming the model
        let input = if diff_text.len() > 100_000 {
            &diff_text[..100_000]
        } else {
            diff_text
        };
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|e| format!("Failed to write to claude stdin: {e}"))?;
        // Drop stdin to signal EOF
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("Failed to wait for claude: {e}"))?;

    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.is_empty() {
            Ok("(no summary generated)".to_string())
        } else {
            debug!("AI summary generated ({} chars)", text.len());
            Ok(text)
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!("claude failed: {}", stderr);
        Err(format!("claude exited with {}", output.status))
    }
}

/// Compute an xxh3 hash of the diff text for staleness detection.
pub fn diff_hash(diff_text: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(diff_text.as_bytes())
}

/// Determine whether an AI summary fetch should be spawned, and if so, whether
/// the UI should show "Loading" or keep displaying the existing cached summary.
///
/// Returns `None` if no fetch is needed (already loading, error, etc.).
/// Returns `Some(cached_hash)` if a fetch should be spawned, where `cached_hash`
/// is `Some(hash)` if there's an existing summary to keep showing, or `None` if
/// the UI should show "Loading".
pub fn should_fetch_summary(current: Option<&AiSummary>) -> Option<Option<u64>> {
    match current {
        None => Some(None),                // No cache → show Loading
        Some(AiSummary::Loading) => None,  // Already in flight
        Some(AiSummary::Error(_)) => None, // Don't retry errors
        Some(AiSummary::Ready { diff_hash, .. }) => Some(Some(*diff_hash)), // Keep visible, check hash in bg
    }
}

/// Check whether the background task should call Claude or skip because the
/// diff hasn't changed since the cached summary was generated.
pub fn should_call_claude(cached_hash: Option<u64>, new_diff_hash: u64) -> bool {
    cached_hash != Some(new_diff_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_hash_deterministic() {
        let text = "+added line\n-removed line\n";
        assert_eq!(diff_hash(text), diff_hash(text));
    }

    #[test]
    fn test_diff_hash_different_for_different_input() {
        assert_ne!(diff_hash("abc"), diff_hash("def"));
    }

    #[tokio::test]
    async fn test_empty_diff_skips_claude() {
        let result = fetch_branch_summary("", "whatever-model").await;
        assert_eq!(result.unwrap(), "No changes on this branch.");
    }

    #[tokio::test]
    async fn test_whitespace_only_diff_skips_claude() {
        let result = fetch_branch_summary("   \n  \n  ", "whatever-model").await;
        assert_eq!(result.unwrap(), "No changes on this branch.");
    }

    #[test]
    fn test_ai_summary_variants() {
        // Ensure all variants are constructible
        let _loading = AiSummary::Loading;
        let _ready = AiSummary::Ready {
            text: "summary".to_string(),
            diff_hash: 42,
        };
        let _error = AiSummary::Error("failed".to_string());
    }

    // ── Cache logic tests ─────────────────────────────────────────

    #[test]
    fn test_should_fetch_when_no_cache() {
        // No existing summary → should fetch, show Loading
        let result = should_fetch_summary(None);
        assert_eq!(result, Some(None));
    }

    #[test]
    fn test_should_not_fetch_when_loading() {
        // Already loading → don't spawn another
        let result = should_fetch_summary(Some(&AiSummary::Loading));
        assert_eq!(result, None);
    }

    #[test]
    fn test_should_not_fetch_when_errored() {
        // Error state → don't retry automatically
        let result = should_fetch_summary(Some(&AiSummary::Error("fail".into())));
        assert_eq!(result, None);
    }

    #[test]
    fn test_should_fetch_when_cached_returns_hash() {
        // Has cached summary → should fetch (to check hash), return cached hash
        let cached = AiSummary::Ready {
            text: "old summary".into(),
            diff_hash: 12345,
        };
        let result = should_fetch_summary(Some(&cached));
        assert_eq!(result, Some(Some(12345)));
    }

    #[test]
    fn test_should_call_claude_when_no_cached_hash() {
        // No previous hash → always call Claude
        assert!(should_call_claude(None, 999));
    }

    #[test]
    fn test_should_call_claude_when_hash_changed() {
        // Hash changed → call Claude
        assert!(should_call_claude(Some(111), 222));
    }

    #[test]
    fn test_should_not_call_claude_when_hash_matches() {
        // Hash unchanged → skip Claude (cache hit)
        assert!(!should_call_claude(Some(42), 42));
    }
}
