//! GitHub repo listing via the `gh` CLI, for the "add a project" repo picker.
//!
//! One call to `gh api --paginate` fetches every repo the user can reach —
//! their own, ones they collaborate on, and ones belonging to their orgs — and
//! projects each into the shared [`GithubRepo`] wire type.
//!
//! **This module deliberately does not swallow errors, unlike [`crate::git::pr`].**
//! That module maps every failure to `None`/`FetchFailed` because it runs on a
//! background poll where a transient hiccup must not disturb cached UI state.
//! Here the call is user-initiated and its whole output is the screen: a picker
//! that silently shows zero repos when `gh` is missing or unauthenticated is
//! indistinguishable from a user with no repos. So failures propagate, with
//! "gh is not installed" carved out as its own [`GitError::GhUnavailable`]
//! because it is the one case the user can act on directly.

use claude_commander_protocol::github::GithubRepo;
use tokio::process::Command;
use tracing::debug;

use crate::error::{GitError, Result};
use crate::git::is_gh_available;

/// jq projection mapping a REST repo object onto [`GithubRepo`]'s fields.
///
/// Every field is top-level on the REST payload except `owner`, which is an
/// object — hence `.owner.login`. Verified against live output on 2026-08-09
/// (gh 2.96.0); repro with the endpoint below at `per_page=2` and compare the
/// keys to the struct.
const REPO_PROJECTION: &str = "\
.[] | {full_name, owner: .owner.login, name, description, private, fork, \
archived, default_branch, clone_url, ssh_url, pushed_at}";

/// The repos endpoint, sorted most-recently-pushed first for the picker.
///
/// The `affiliation` triple is the *documented default* for this endpoint
/// (GitHub REST docs, `GET /user/repos`: "affiliation — Default:
/// `owner,collaborator,organization_member`"). It is passed explicitly anyway,
/// so the set of repos this picker offers is legible at the call site rather
/// than inherited from a remote default that could change.
///
/// There is no page cap: capping would silently hide repos, which is a worse
/// failure than a slow first call.
const REPOS_ENDPOINT: &str = "/user/repos\
?affiliation=owner,collaborator,organization_member&per_page=100&sort=pushed";

/// List every GitHub repo the authenticated user can clone.
///
/// Returns [`GitError::GhUnavailable`] when `gh` is missing; any other failure
/// (not authenticated, network, rate limit) surfaces as
/// [`GitError::OperationFailed`] carrying gh's own stderr, which is already
/// phrased for a human.
///
/// The availability probe is [`is_gh_available`], the same one the PR poller
/// uses. `CommanderService` caches that answer in a `OnceCell` and callers
/// there should keep gating on the cached value; the check here is a backstop
/// for direct callers and costs one `gh --version` per picker open.
pub async fn list_repos() -> Result<Vec<GithubRepo>> {
    if !is_gh_available().await {
        return Err(GitError::GhUnavailable.into());
    }

    let output = Command::new("gh")
        .args(["api", "--paginate", "--jq", REPO_PROJECTION, REPOS_ENDPOINT])
        .output()
        .await
        .map_err(|e| {
            // gh passed `--version` moments ago, so a spawn failure here means
            // it vanished or became unexecutable mid-flight — still "no gh".
            debug!("gh api spawn failed: {e}");
            GitError::GhUnavailable
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(GitError::OperationFailed(format!(
            "gh api {REPOS_ENDPOINT} failed: {}",
            stderr.trim()
        ))
        .into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let repos = parse_repo_stream(&stdout)?;
    debug!("gh api listed {} repos", repos.len());
    Ok(repos)
}

/// Parse gh's `--paginate --jq` output into repos.
///
/// **The output is a stream of JSON objects, not a JSON array**, so
/// `serde_json::from_str::<Vec<_>>` fails on any real multi-page result. gh
/// applies the `--jq` filter to each page separately and concatenates what
/// comes out: `gh api --help` (gh 2.96.0) states "Each page is a separate JSON
/// array or object. Pass `--slurp` to wrap all pages of JSON arrays or objects
/// into an outer JSON array." — and `--slurp` is not in play here because it
/// wraps *pages*, not the projected objects. Repro: run
/// `gh api --paginate --jq '.[] | {name}' "/repos/cli/cli/labels?per_page=3"`
/// and observe 80-odd bare objects spanning ~28 pages, with no enclosing `[`.
///
/// `serde_json::Deserializer::into_iter` reads exactly that shape. Empty input
/// yields an empty list, not an error — a user with no repos is not a failure.
fn parse_repo_stream(output: &str) -> Result<Vec<GithubRepo>> {
    serde_json::Deserializer::from_str(output)
        .into_iter::<GithubRepo>()
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| GitError::OperationFailed(format!("failed to parse gh repo list: {e}")).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::GitError;

    #[test]
    fn parses_the_concatenated_per_page_object_stream() {
        // Two pages' worth of --jq output, concatenated exactly as gh emits it.
        // This is NOT a JSON array; `from_str::<Vec<_>>` would fail here.
        let out = r#"{"full_name":"me/a","owner":"me","name":"a","description":null,
"private":false,"fork":false,"archived":false,"default_branch":"main",
"clone_url":"https://github.com/me/a.git","ssh_url":"git@github.com:me/a.git",
"pushed_at":"2026-01-02T03:04:05Z"}
{"full_name":"org/b","owner":"org","name":"b","description":"x",
"private":true,"fork":false,"archived":false,"default_branch":"trunk",
"clone_url":"https://github.com/org/b.git","ssh_url":"git@github.com:org/b.git",
"pushed_at":null}"#;
        let repos = parse_repo_stream(out).unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].full_name, "me/a");
        assert!(repos[1].private);
        assert_eq!(repos[1].pushed_at, None); // empty repo
    }

    #[test]
    fn empty_output_is_an_empty_list_not_an_error() {
        assert!(parse_repo_stream("").unwrap().is_empty());
    }

    #[test]
    fn a_json_array_would_not_have_parsed_as_a_stream_of_repos() {
        // Pins the framing the module is built around: if gh ever started
        // emitting one array, this parser would reject it loudly rather than
        // returning an empty list, and this test is where that shows up.
        let arr = r#"[{"full_name":"me/a","owner":"me","name":"a","private":false,
"fork":false,"archived":false,"default_branch":"main",
"clone_url":"c","ssh_url":"s","pushed_at":null}]"#;
        assert!(parse_repo_stream(arr).is_err());
    }

    #[test]
    fn truncated_output_is_an_error_not_a_short_list() {
        // A killed gh (or a mid-stream network failure) must not look like
        // "these are all your repos".
        let truncated = r#"{"full_name":"me/a","owner":"me","name":"a","private":false,
"fork":false,"archived":false,"default_branch":"main","clone_url":"c""#;
        assert!(parse_repo_stream(truncated).is_err());
    }

    #[test]
    fn gh_unavailable_error_displays() {
        assert!(!GitError::GhUnavailable.to_string().is_empty());
    }
}
