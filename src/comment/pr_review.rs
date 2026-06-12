//! Composing a GitHub pull-request review from PR-targeted comments.
//!
//! PR comments are staged locally (like agent comments) and submitted together
//! as a single review via `gh api .../pulls/{n}/reviews`. The payload built
//! here is pure JSON so it is unit-testable without touching the network; the
//! effectful submission (shelling out to `gh`) lives in `CommanderService`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{Comment, CommentSide};

/// Outcome of submitting a session's PR-targeted comments as a GitHub review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum PrReviewOutcome {
    /// No PR-targeted comments to submit.
    Nothing,
    /// The session has no associated pull request to review.
    NoPr,
    /// One or more PR comments are drifted; nothing was submitted.
    Blocked { drifted: Vec<Uuid> },
    /// A review with `count` inline comments was submitted to the PR.
    Submitted { count: usize },
}

/// The overall verdict attached to a submitted review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrVerdict {
    /// A plain review with no approval state.
    Comment,
    /// Approve the pull request.
    Approve,
    /// Request changes on the pull request.
    RequestChanges,
}

impl PrVerdict {
    /// The GitHub `event` string for the reviews API.
    pub fn event(self) -> &'static str {
        match self {
            PrVerdict::Comment => "COMMENT",
            PrVerdict::Approve => "APPROVE",
            PrVerdict::RequestChanges => "REQUEST_CHANGES",
        }
    }

    /// Short human label for the verdict prompt / status messages.
    pub fn label(self) -> &'static str {
        match self {
            PrVerdict::Comment => "Comment",
            PrVerdict::Approve => "Approve",
            PrVerdict::RequestChanges => "Request changes",
        }
    }
}

/// GitHub review-comment side string for a diff side.
fn gh_side(side: CommentSide) -> &'static str {
    match side {
        CommentSide::Old => "LEFT",
        CommentSide::New => "RIGHT",
    }
}

/// Build the JSON body for `POST /repos/{owner}/{repo}/pulls/{n}/reviews`.
///
/// Each comment becomes an inline review comment anchored to its file, side and
/// line range (a multi-line range emits `start_line`/`start_side`; a single
/// line omits them). `body` is the overall review summary — included only when
/// non-empty. Deterministic (no ids or timestamps) so it is snapshot-stable.
pub fn compose_pr_review(verdict: PrVerdict, body: &str, comments: &[Comment]) -> Value {
    let inline: Vec<Value> = comments
        .iter()
        .map(|c| {
            let (lo, hi) = c.line_range;
            let side = gh_side(c.side);
            let mut obj = json!({
                "path": c.file,
                "line": hi,
                "side": side,
                "body": c.comment,
            });
            if lo < hi {
                obj["start_line"] = json!(lo);
                obj["start_side"] = json!(side);
            }
            obj
        })
        .collect();

    let mut payload = json!({
        "event": verdict.event(),
        "comments": inline,
    });
    let trimmed = body.trim();
    if !trimmed.is_empty() {
        payload["body"] = json!(trimmed);
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::super::CommentTarget;
    use super::*;

    fn pr_comment(file: &str, side: CommentSide, range: (usize, usize), text: &str) -> Comment {
        Comment::new(file, side, range, "snip", text).with_target(CommentTarget::Pr)
    }

    #[test]
    fn verdict_event_strings() {
        assert_eq!(PrVerdict::Comment.event(), "COMMENT");
        assert_eq!(PrVerdict::Approve.event(), "APPROVE");
        assert_eq!(PrVerdict::RequestChanges.event(), "REQUEST_CHANGES");
    }

    #[test]
    fn single_line_comment_omits_start_line() {
        let c = pr_comment("src/a.rs", CommentSide::New, (12, 12), "tidy this");
        let payload = compose_pr_review(PrVerdict::Comment, "", &[c]);
        let inline = &payload["comments"][0];
        assert_eq!(inline["path"], "src/a.rs");
        assert_eq!(inline["line"], 12);
        assert_eq!(inline["side"], "RIGHT");
        assert_eq!(inline["body"], "tidy this");
        assert!(inline.get("start_line").is_none());
        assert!(inline.get("start_side").is_none());
    }

    #[test]
    fn multi_line_comment_emits_start_line_and_side() {
        let c = pr_comment("src/a.rs", CommentSide::Old, (4, 9), "drop this block");
        let payload = compose_pr_review(PrVerdict::RequestChanges, "", &[c]);
        let inline = &payload["comments"][0];
        assert_eq!(inline["side"], "LEFT");
        assert_eq!(inline["line"], 9);
        assert_eq!(inline["start_line"], 4);
        assert_eq!(inline["start_side"], "LEFT");
        assert_eq!(payload["event"], "REQUEST_CHANGES");
    }

    #[test]
    fn empty_body_is_omitted_nonempty_trimmed() {
        let c = pr_comment("a.rs", CommentSide::New, (1, 1), "x");
        let no_body = compose_pr_review(PrVerdict::Approve, "   ", std::slice::from_ref(&c));
        assert!(no_body.get("body").is_none());

        let with_body = compose_pr_review(PrVerdict::Approve, "  looks good  ", &[c]);
        assert_eq!(with_body["body"], "looks good");
    }

    #[test]
    fn collects_all_comments_in_order() {
        let a = pr_comment("a.rs", CommentSide::New, (1, 1), "first");
        let b = pr_comment("b.rs", CommentSide::New, (2, 2), "second");
        let payload = compose_pr_review(PrVerdict::Comment, "", &[a, b]);
        let arr = payload["comments"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["body"], "first");
        assert_eq!(arr[1]["body"], "second");
    }
}
