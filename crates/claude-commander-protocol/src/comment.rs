//! Review-comment wire types.
//!
//! A [`Comment`] is a note the user attaches to a line range in a session's
//! review diff. The persistence, re-anchoring, and markdown-composition logic
//! live in `claude-commander-core`; only the serialized model crosses the
//! network, so it lives here.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Which side of the diff a line range refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentSide {
    Old,
    New,
}

/// Lifecycle status of a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentStatus {
    /// Anchored to the current diff, ready to apply.
    Staged,
    /// Its snippet could not be located unambiguously in the current diff;
    /// blocks Apply until reviewed or deleted.
    Drifted,
    /// Already sent to the agent.
    Applied,
}

/// A single review comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: Uuid,
    /// Display path of the file (new path, or old path for deletions).
    pub file: String,
    pub side: CommentSide,
    /// Inclusive line range on `side` at capture / last successful anchor time.
    pub line_range: (usize, usize),
    /// Captured line contents (joined with `\n`), used to re-anchor.
    pub snippet: String,
    pub comment: String,
    pub status: CommentStatus,
    pub created_at: DateTime<Utc>,
}

impl Comment {
    /// Create a freshly staged comment with a random id and `created_at`
    /// set to now. `snippet` is normalised to drop any trailing newline.
    pub fn new(
        file: impl Into<String>,
        side: CommentSide,
        line_range: (usize, usize),
        snippet: impl Into<String>,
        comment: impl Into<String>,
    ) -> Self {
        let snippet = snippet.into();
        Self {
            id: Uuid::new_v4(),
            file: file.into(),
            side,
            line_range,
            snippet: snippet.trim_end_matches('\n').to_string(),
            comment: comment.into(),
            status: CommentStatus::Staged,
            created_at: Utc::now(),
        }
    }
}

/// Outcome of applying a session's staged comments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ApplyOutcome {
    /// No staged comments to apply.
    Nothing,
    /// One or more comments are drifted; nothing was sent.
    Blocked { drifted: Vec<Uuid> },
    /// Comments were composed to `path` and the prompt injected.
    Applied { path: PathBuf, count: usize },
    /// The brief was written to `path` but couldn't be delivered (agent stopped
    /// or stayed at a prompt past the hold timeout); the user can re-apply.
    Deferred { path: PathBuf, count: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_round_trips() {
        let c = Comment::new("a.rs", CommentSide::New, (1, 3), "snippet\n", "looks off");
        // Trailing newline is trimmed from the captured snippet.
        assert_eq!(c.snippet, "snippet");
        assert_eq!(c.status, CommentStatus::Staged);
        let wire = serde_json::to_string(&c).unwrap();
        let back: Comment = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, c);
    }
}
