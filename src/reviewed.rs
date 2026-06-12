//! Per-file "reviewed" marks for the review-diff view.
//!
//! A mark records that the user has reviewed one file of a session's review
//! diff, keyed by the file's display path and pinned to a hash of the file's
//! hunks at mark time. Marks persist across runs (one JSON file per session)
//! and are invalidated when the file's diff content changes or the file
//! leaves the diff — GitHub "Viewed" semantics.
//!
//! All logic here is pure or filesystem-only so it is testable without a TUI;
//! the presentation layer only renders and dispatches.

use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{ConfigError, Result};
use crate::git::{FileDiff, ParsedDiff, file_diff_hash};
use crate::session::SessionId;

/// A persisted "reviewed" mark for one file of a session's review diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewedMark {
    /// Display path of the file (new path, or old path for deletions).
    pub file: String,
    /// Hex [`file_diff_hash`] of the file's hunks at mark time.
    pub hash: String,
    pub marked_at: DateTime<Utc>,
}

/// Hex rendering of a file's hunk hash, as stored in [`ReviewedMark::hash`].
fn hash_hex(file: &FileDiff) -> String {
    format!("{:016x}", file_diff_hash(file))
}

/// Per-session reviewed-mark store, persisted as one JSON array per session
/// under a directory (typically `<data_dir>/reviewed/`).
pub struct ReviewedStore {
    dir: PathBuf,
}

impl ReviewedStore {
    /// Construct a store rooted at `dir` (created lazily on first save).
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Default store at `<data_dir>/reviewed/`.
    pub fn open_default() -> Result<Self> {
        Ok(Self::new(Config::data_dir()?.join("reviewed")))
    }

    fn path_for(&self, sid: SessionId) -> PathBuf {
        self.dir.join(format!("{}.json", sid.as_uuid()))
    }

    /// Load a session's marks (an absent file yields an empty list).
    pub fn load(&self, sid: SessionId) -> Result<Vec<ReviewedMark>> {
        match fs::read_to_string(self.path_for(sid)) {
            Ok(s) => {
                Ok(serde_json::from_str(&s).map_err(|e| ConfigError::LoadFailed(e.to_string()))?)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(ConfigError::LoadFailed(e.to_string()).into()),
        }
    }

    /// Persist a session's marks via a temp-file + rename (atomic).
    pub fn save(&self, sid: SessionId, marks: &[ReviewedMark]) -> Result<()> {
        fs::create_dir_all(&self.dir).map_err(|e| ConfigError::SaveFailed(e.to_string()))?;
        let json = serde_json::to_string_pretty(marks)
            .map_err(|e| ConfigError::SaveFailed(e.to_string()))?;
        let tmp = self.dir.join(format!(".{}.tmp", sid.as_uuid()));
        fs::write(&tmp, json).map_err(|e| ConfigError::SaveFailed(e.to_string()))?;
        fs::rename(&tmp, self.path_for(sid)).map_err(|e| ConfigError::SaveFailed(e.to_string()))?;
        Ok(())
    }
}

/// Drop marks whose file is absent from `diff` or whose stored hash no longer
/// matches the file's current hunk hash. Returns whether anything was removed.
pub fn prune_invalidated(marks: &mut Vec<ReviewedMark>, diff: &ParsedDiff) -> bool {
    let before = marks.len();
    marks.retain(|m| {
        diff.files
            .iter()
            .any(|f| f.display_path() == m.file && hash_hex(f) == m.hash)
    });
    marks.len() != before
}

/// Toggle the mark for `file`. The hash is computed from the [`FileDiff`] the
/// caller is displaying, so the mark records exactly what the user saw.
/// Returns the new reviewed state.
pub fn toggle(marks: &mut Vec<ReviewedMark>, file: &FileDiff) -> bool {
    let path = file.display_path();
    let before = marks.len();
    marks.retain(|m| m.file != path);
    if marks.len() != before {
        return false;
    }
    marks.push(ReviewedMark {
        file: path.to_string(),
        hash: hash_hex(file),
        marked_at: Utc::now(),
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::parse_unified_diff;
    use tempfile::TempDir;

    /// Minimal one-file diff with the given path and body lines.
    fn diff_for(path: &str, body: &str) -> ParsedDiff {
        parse_unified_diff(&format!(
            "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1,2 +1,2 @@\n{body}"
        ))
    }

    fn mark_for(file: &FileDiff) -> ReviewedMark {
        ReviewedMark {
            file: file.display_path().to_string(),
            hash: hash_hex(file),
            marked_at: Utc::now(),
        }
    }

    // --- store ---

    #[test]
    fn load_missing_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let store = ReviewedStore::new(tmp.path().join("reviewed"));
        assert!(store.load(SessionId::new()).unwrap().is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let tmp = TempDir::new().unwrap();
        let store = ReviewedStore::new(tmp.path().join("reviewed"));
        let sid = SessionId::new();
        let diff = diff_for("src/a.rs", " ctx\n-old\n+new\n");
        let marks = vec![mark_for(&diff.files[0])];

        store.save(sid, &marks).unwrap();
        assert_eq!(store.load(sid).unwrap(), marks);
        // Other sessions are unaffected.
        assert!(store.load(SessionId::new()).unwrap().is_empty());
    }

    // --- toggle ---

    #[test]
    fn toggle_adds_then_removes_mark() {
        let diff = diff_for("src/a.rs", " ctx\n-old\n+new\n");
        let file = &diff.files[0];
        let mut marks = Vec::new();

        assert!(toggle(&mut marks, file));
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].file, "src/a.rs");
        assert_eq!(marks[0].hash, hash_hex(file));

        assert!(!toggle(&mut marks, file));
        assert!(marks.is_empty());
    }

    // --- prune ---

    #[test]
    fn prune_keeps_mark_when_hash_matches() {
        let diff = diff_for("src/a.rs", " ctx\n-old\n+new\n");
        let mut marks = vec![mark_for(&diff.files[0])];

        assert!(!prune_invalidated(&mut marks, &diff));
        assert_eq!(marks.len(), 1);
    }

    #[test]
    fn prune_drops_mark_when_diff_content_changes() {
        let before = diff_for("src/a.rs", " ctx\n-old\n+new\n");
        let after = diff_for("src/a.rs", " ctx\n-old\n+newer\n");
        let mut marks = vec![mark_for(&before.files[0])];

        assert!(prune_invalidated(&mut marks, &after));
        assert!(marks.is_empty());
    }

    #[test]
    fn prune_drops_mark_when_file_leaves_diff() {
        let before = diff_for("src/a.rs", " ctx\n-old\n+new\n");
        let after = diff_for("src/other.rs", " ctx\n-old\n+new\n");
        let mut marks = vec![mark_for(&before.files[0])];

        assert!(prune_invalidated(&mut marks, &after));
        assert!(marks.is_empty());
    }

    #[test]
    fn prune_returns_false_when_nothing_changes() {
        let diff = diff_for("src/a.rs", " ctx\n-old\n+new\n");
        let mut marks: Vec<ReviewedMark> = Vec::new();
        assert!(!prune_invalidated(&mut marks, &diff));
    }
}
