//! GitHub PR detection via `gh` CLI
//!
//! Checks whether a branch has an open pull request using `gh pr list`.
//! All failures are silently swallowed — missing `gh`, auth errors, network
//! issues, or repos without a GitHub remote simply result in `None`.

use std::path::Path;

use tokio::process::Command;
use tracing::debug;

/// Minimal PR metadata returned by `gh pr list`.
#[derive(Debug, Clone)]
pub struct PrInfo {
    pub number: u32,
    pub url: String,
}

/// Returns `true` if the `gh` CLI is installed and runnable.
///
/// Called once at startup to avoid repeated fork/exec on every tick.
pub async fn is_gh_available() -> bool {
    match Command::new("gh").arg("--version").output().await {
        Ok(output) => {
            let ok = output.status.success();
            debug!("gh --version: available={}", ok);
            ok
        }
        Err(e) => {
            debug!("gh not available: {}", e);
            false
        }
    }
}

/// Check whether `branch` has an open PR in the repo at `repo_path`.
///
/// Returns `None` on any failure (gh missing, not authed, network error,
/// not a GitHub repo, or no open PR).
pub async fn check_pr_for_branch(repo_path: &Path, branch: &str) -> Option<PrInfo> {
    let output = Command::new("gh")
        .args([
            "pr",
            "list",
            "--head",
            branch,
            "--json",
            "number,url",
            "--limit",
            "1",
        ])
        .current_dir(repo_path)
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        debug!(
            "gh pr list failed for branch {}: {}",
            branch,
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }

    let json = String::from_utf8(output.stdout).ok()?;
    parse_pr_json(&json)
}

/// Parse the JSON array returned by `gh pr list --json number,url --limit 1`.
///
/// Expected format: `[{"number":123,"url":"https://..."}]` or `[]`.
fn parse_pr_json(json: &str) -> Option<PrInfo> {
    let trimmed = json.trim();
    if trimmed.is_empty() || trimmed == "[]" {
        return None;
    }

    // Minimal JSON parsing without pulling in serde_json for this one call.
    // The output is a single-element array of `{"number":N,"url":"..."}`.
    let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?.trim();
    if inner.is_empty() {
        return None;
    }

    // Extract "number": <digits>
    let number = {
        let idx = inner.find("\"number\"")?;
        let after_key = &inner[idx + "\"number\"".len()..];
        let colon = after_key.find(':')?;
        let after_colon = after_key[colon + 1..].trim_start();
        // Read digits until a non-digit character
        let end = after_colon
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after_colon.len());
        after_colon[..end].parse::<u32>().ok()?
    };

    // Extract "url": "..."
    let url = {
        let idx = inner.find("\"url\"")?;
        let after_key = &inner[idx + "\"url\"".len()..];
        let colon = after_key.find(':')?;
        let after_colon = after_key[colon + 1..].trim_start();
        let quote_start = after_colon.find('"')?;
        let rest = &after_colon[quote_start + 1..];
        let quote_end = rest.find('"')?;
        rest[..quote_end].to_string()
    };

    Some(PrInfo { number, url })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pr_json_valid() {
        let json = r#"[{"number":42,"url":"https://github.com/owner/repo/pull/42"}]"#;
        let info = parse_pr_json(json).unwrap();
        assert_eq!(info.number, 42);
        assert_eq!(info.url, "https://github.com/owner/repo/pull/42");
    }

    #[test]
    fn test_parse_pr_json_empty_array() {
        assert!(parse_pr_json("[]").is_none());
    }

    #[test]
    fn test_parse_pr_json_empty_string() {
        assert!(parse_pr_json("").is_none());
    }

    #[test]
    fn test_parse_pr_json_whitespace() {
        assert!(parse_pr_json("  \n  ").is_none());
    }

    #[test]
    fn test_parse_pr_json_with_whitespace() {
        let json = r#"
        [
          {
            "number": 1234,
            "url": "https://github.com/org/project/pull/1234"
          }
        ]
        "#;
        let info = parse_pr_json(json).unwrap();
        assert_eq!(info.number, 1234);
        assert_eq!(info.url, "https://github.com/org/project/pull/1234");
    }

    #[test]
    fn test_parse_pr_json_url_before_number() {
        let json = r#"[{"url":"https://github.com/a/b/pull/7","number":7}]"#;
        let info = parse_pr_json(json).unwrap();
        assert_eq!(info.number, 7);
        assert_eq!(info.url, "https://github.com/a/b/pull/7");
    }

    #[test]
    fn test_parse_pr_json_garbage() {
        assert!(parse_pr_json("not json at all").is_none());
    }
}
