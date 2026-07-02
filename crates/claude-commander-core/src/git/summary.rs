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
/// Runs `git diff origin/<main_branch>...HEAD` (committed) and `git diff HEAD` (uncommitted)
/// and concatenates them. This gives a complete picture of all changes on the branch.
/// Uses `origin/<main_branch>` rather than local `<main_branch>` because the local ref
/// is often stale — branches typically merge `origin/main`, not local `main`.
pub async fn compute_branch_diff(worktree_path: &Path, main_branch: &str) -> String {
    let (committed, uncommitted) = tokio::join!(
        // Committed changes vs main branch
        Command::new("git")
            .current_dir(worktree_path)
            .args(["diff", &format!("origin/{main_branch}...HEAD")])
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

/// Maximum number of diff bytes piped into the Claude CLI.
const MAX_SUMMARY_INPUT_BYTES: usize = 100_000;

/// Truncate `s` to at most `max_bytes` bytes without splitting a UTF-8
/// character.
fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
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
        let input = truncate_to_char_boundary(diff_text, MAX_SUMMARY_INPUT_BYTES);
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
    fn test_truncate_to_char_boundary_short_input_untouched() {
        assert_eq!(truncate_to_char_boundary("abc", 10), "abc");
        assert_eq!(truncate_to_char_boundary("abc", 3), "abc");
    }

    #[test]
    fn test_truncate_to_char_boundary_ascii_cut() {
        assert_eq!(truncate_to_char_boundary("abcdef", 4), "abcd");
    }

    #[test]
    fn test_truncate_to_char_boundary_does_not_split_multibyte_char() {
        // Regression: byte-index slicing panicked when the truncation limit
        // fell inside a multi-byte character in the diff text.
        // 'é' is 2 bytes, so a limit of 2 lands mid-character.
        assert_eq!(truncate_to_char_boundary("aé", 2), "a");
        // '日' is 3 bytes (1..4); limits 2 and 3 both land inside it.
        assert_eq!(truncate_to_char_boundary("a日b", 2), "a");
        assert_eq!(truncate_to_char_boundary("a日b", 3), "a");
        assert_eq!(truncate_to_char_boundary("a日b", 4), "a日");
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
}
