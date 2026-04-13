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
    pub merged: bool,
}

/// Rich PR metadata returned by `gh pr view`.
#[derive(Debug, Clone)]
pub struct EnrichedPrInfo {
    pub number: u32,
    pub url: String,
    pub title: String,
    pub state: PrState,
    pub is_draft: bool,
    pub labels: Vec<PrLabel>,
    pub checks_status: ChecksStatus,
    pub body: String,
}

/// PR state as reported by the GitHub API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

impl std::fmt::Display for PrState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "Open"),
            Self::Closed => write!(f, "Closed"),
            Self::Merged => write!(f, "Merged"),
        }
    }
}

/// A PR label with name and hex color.
#[derive(Debug, Clone)]
pub struct PrLabel {
    pub name: String,
    pub color: String,
}

/// Aggregate CI/checks status derived from `statusCheckRollup`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChecksStatus {
    Passing,
    Failing,
    Pending,
    None,
}

impl std::fmt::Display for ChecksStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passing => write!(f, "Passing"),
            Self::Failing => write!(f, "Failing"),
            Self::Pending => write!(f, "Pending"),
            Self::None => write!(f, "None"),
        }
    }
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

/// Check whether `branch` has a PR (open or merged) in the repo at `repo_path`.
///
/// Returns `None` on any failure (gh missing, not authed, network error,
/// not a GitHub repo, or no PR). Prefers open PRs over merged ones.
pub async fn check_pr_for_branch(repo_path: &Path, branch: &str) -> Option<PrInfo> {
    // Check open PRs first, then merged if none found
    for state in &["open", "merged"] {
        let output = Command::new("gh")
            .args([
                "pr",
                "list",
                "--head",
                branch,
                "--state",
                state,
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
                "gh pr list --state {} failed for branch {}: {}",
                state,
                branch,
                String::from_utf8_lossy(&output.stderr)
            );
            return None;
        }

        let json = String::from_utf8(output.stdout).ok()?;
        if let Some(mut info) = parse_pr_json(&json) {
            info.merged = *state == "merged";
            return Some(info);
        }
    }

    None
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

    Some(PrInfo {
        number,
        url,
        merged: false,
    })
}

/// Fetch enriched PR details for a specific PR number via `gh pr view`.
///
/// Returns `None` on any failure (gh missing, not authed, network error, etc.).
pub async fn fetch_enriched_pr(repo_path: &Path, pr_number: u32) -> Option<EnrichedPrInfo> {
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            &pr_number.to_string(),
            "--json",
            "title,state,isDraft,labels,statusCheckRollup,body,url",
        ])
        .current_dir(repo_path)
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        debug!(
            "gh pr view {} failed: {}",
            pr_number,
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }

    let json = String::from_utf8(output.stdout).ok()?;
    parse_enriched_pr_json(&json, pr_number)
}

/// Parse the JSON object returned by `gh pr view --json ...`.
fn parse_enriched_pr_json(json: &str, pr_number: u32) -> Option<EnrichedPrInfo> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;

    let title = v["title"].as_str().unwrap_or("").to_string();
    let url = v["url"].as_str().unwrap_or("").to_string();
    let body = v["body"].as_str().unwrap_or("").to_string();
    let is_draft = v["isDraft"].as_bool().unwrap_or(false);

    let state = match v["state"].as_str().unwrap_or("") {
        "OPEN" => PrState::Open,
        "CLOSED" => PrState::Closed,
        "MERGED" => PrState::Merged,
        _ => PrState::Open,
    };

    let labels = v["labels"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|l| {
                    Some(PrLabel {
                        name: l["name"].as_str()?.to_string(),
                        color: l["color"].as_str().unwrap_or("").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let checks_status = parse_checks_rollup(&v["statusCheckRollup"]);

    Some(EnrichedPrInfo {
        number: pr_number,
        url,
        title,
        state,
        is_draft,
        labels,
        checks_status,
        body,
    })
}

/// Derive aggregate checks status from the `statusCheckRollup` array.
///
/// - Any `FAILURE` → `Failing`
/// - Any `null` or `PENDING` (without failures) → `Pending`
/// - All `SUCCESS` or `NEUTRAL` → `Passing`
/// - Empty array → `None`
fn parse_checks_rollup(value: &serde_json::Value) -> ChecksStatus {
    let Some(arr) = value.as_array() else {
        return ChecksStatus::None;
    };
    if arr.is_empty() {
        return ChecksStatus::None;
    }

    let mut has_pending = false;
    for check in arr {
        match check["conclusion"].as_str() {
            Some("FAILURE") => return ChecksStatus::Failing,
            Some("SUCCESS") | Some("NEUTRAL") | Some("SKIPPED") => {}
            // null, "PENDING", or anything else → pending
            _ => has_pending = true,
        }
    }

    if has_pending {
        ChecksStatus::Pending
    } else {
        ChecksStatus::Passing
    }
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
        assert!(!info.merged);
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

    #[test]
    fn test_parse_enriched_pr_open() {
        let json = r#"{
            "title": "Add auth flow",
            "url": "https://github.com/org/repo/pull/42",
            "state": "OPEN",
            "isDraft": false,
            "labels": [
                {"name": "bug", "color": "d73a4a"},
                {"name": "enhancement", "color": "a2eeef"}
            ],
            "statusCheckRollup": [
                {"conclusion": "SUCCESS"},
                {"conclusion": "SUCCESS"}
            ],
            "body": "This PR adds auth."
        }"#;
        let info = parse_enriched_pr_json(json, 42).unwrap();
        assert_eq!(info.number, 42);
        assert_eq!(info.title, "Add auth flow");
        assert_eq!(info.url, "https://github.com/org/repo/pull/42");
        assert_eq!(info.state, PrState::Open);
        assert!(!info.is_draft);
        assert_eq!(info.labels.len(), 2);
        assert_eq!(info.labels[0].name, "bug");
        assert_eq!(info.labels[0].color, "d73a4a");
        assert_eq!(info.checks_status, ChecksStatus::Passing);
        assert_eq!(info.body, "This PR adds auth.");
    }

    #[test]
    fn test_parse_enriched_pr_merged_draft() {
        let json = r#"{
            "title": "Refactor",
            "url": "https://github.com/org/repo/pull/7",
            "state": "MERGED",
            "isDraft": true,
            "labels": [],
            "statusCheckRollup": [],
            "body": ""
        }"#;
        let info = parse_enriched_pr_json(json, 7).unwrap();
        assert_eq!(info.state, PrState::Merged);
        assert!(info.is_draft);
        assert!(info.labels.is_empty());
        assert_eq!(info.checks_status, ChecksStatus::None);
    }

    #[test]
    fn test_parse_enriched_pr_closed() {
        let json = r#"{
            "title": "WIP",
            "url": "",
            "state": "CLOSED",
            "isDraft": false,
            "labels": [],
            "statusCheckRollup": [],
            "body": ""
        }"#;
        let info = parse_enriched_pr_json(json, 1).unwrap();
        assert_eq!(info.state, PrState::Closed);
    }

    #[test]
    fn test_checks_rollup_all_passing() {
        let v: serde_json::Value = serde_json::from_str(
            r#"[{"conclusion":"SUCCESS"},{"conclusion":"NEUTRAL"},{"conclusion":"SKIPPED"}]"#,
        )
        .unwrap();
        assert_eq!(parse_checks_rollup(&v), ChecksStatus::Passing);
    }

    #[test]
    fn test_checks_rollup_one_failure() {
        let v: serde_json::Value =
            serde_json::from_str(r#"[{"conclusion":"SUCCESS"},{"conclusion":"FAILURE"}]"#).unwrap();
        assert_eq!(parse_checks_rollup(&v), ChecksStatus::Failing);
    }

    #[test]
    fn test_checks_rollup_pending() {
        let v: serde_json::Value =
            serde_json::from_str(r#"[{"conclusion":"SUCCESS"},{"conclusion":null}]"#).unwrap();
        assert_eq!(parse_checks_rollup(&v), ChecksStatus::Pending);
    }

    #[test]
    fn test_checks_rollup_empty() {
        let v: serde_json::Value = serde_json::from_str("[]").unwrap();
        assert_eq!(parse_checks_rollup(&v), ChecksStatus::None);
    }

    #[test]
    fn test_checks_rollup_not_array() {
        let v: serde_json::Value = serde_json::from_str("null").unwrap();
        assert_eq!(parse_checks_rollup(&v), ChecksStatus::None);
    }

    #[test]
    fn test_parse_enriched_pr_invalid_json() {
        assert!(parse_enriched_pr_json("not json", 1).is_none());
    }

    #[test]
    fn test_pr_state_display() {
        assert_eq!(PrState::Open.to_string(), "Open");
        assert_eq!(PrState::Closed.to_string(), "Closed");
        assert_eq!(PrState::Merged.to_string(), "Merged");
    }

    #[test]
    fn test_checks_status_display() {
        assert_eq!(ChecksStatus::Passing.to_string(), "Passing");
        assert_eq!(ChecksStatus::Failing.to_string(), "Failing");
        assert_eq!(ChecksStatus::Pending.to_string(), "Pending");
        assert_eq!(ChecksStatus::None.to_string(), "None");
    }
}
