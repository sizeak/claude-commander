//! Local diff-review comments.
//!
//! An comment is a comment the user attaches to a line range in a session's
//! review diff. Comments are *staged* (persisted across restarts) until the
//! user applies them, at which point they are composed into a markdown brief
//! and handed to the agent (see the service layer). The captured `snippet` is
//! stored so a comment can be re-anchored even after the surrounding code
//! drifts; if it can no longer be located unambiguously, the comment is
//! marked [`CommentStatus::Drifted`] and blocks Apply.
//!
//! All logic here is pure or filesystem-only so it is testable without a TUI;
//! the presentation layer only renders and dispatches.

pub mod apply;
pub mod selection;

pub use apply::{ApplyOutcome, SendDecision, decide_send};

use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::Config;
use crate::error::{ConfigError, Result};
use crate::git::{FileDiff, LineOrigin, ParsedDiff};
use crate::session::SessionId;

/// Which side of the diff a line range refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentSide {
    Old,
    New,
}

/// Lifecycle status of an comment.
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

/// Outcome of trying to locate an comment's snippet in a fresh diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorResult {
    Located {
        side: CommentSide,
        line_range: (usize, usize),
    },
    Drifted,
}

/// Per-session comment store, persisted as one JSON array per session under
/// a directory (typically `<data_dir>/comments/`).
pub struct CommentStore {
    dir: PathBuf,
}

impl CommentStore {
    /// Construct a store rooted at `dir` (created lazily on first save).
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Default store at `<data_dir>/comments/`.
    pub fn open_default() -> Result<Self> {
        Ok(Self::new(Config::data_dir()?.join("comments")))
    }

    fn path_for(&self, sid: SessionId) -> PathBuf {
        self.dir.join(format!("{}.json", sid.as_uuid()))
    }

    /// Load a session's comments (an absent file yields an empty list).
    pub fn load(&self, sid: SessionId) -> Result<Vec<Comment>> {
        match fs::read_to_string(self.path_for(sid)) {
            Ok(s) => {
                Ok(serde_json::from_str(&s).map_err(|e| ConfigError::LoadFailed(e.to_string()))?)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(ConfigError::LoadFailed(e.to_string()).into()),
        }
    }

    /// Persist a session's comments via a temp-file + rename (atomic).
    pub fn save(&self, sid: SessionId, anns: &[Comment]) -> Result<()> {
        fs::create_dir_all(&self.dir).map_err(|e| ConfigError::SaveFailed(e.to_string()))?;
        let json = serde_json::to_string_pretty(anns)
            .map_err(|e| ConfigError::SaveFailed(e.to_string()))?;
        let tmp = self.dir.join(format!(".{}.tmp", sid.as_uuid()));
        fs::write(&tmp, json).map_err(|e| ConfigError::SaveFailed(e.to_string()))?;
        fs::rename(&tmp, self.path_for(sid)).map_err(|e| ConfigError::SaveFailed(e.to_string()))?;
        Ok(())
    }

    /// Append an comment to a session.
    pub fn add(&self, sid: SessionId, ann: Comment) -> Result<()> {
        let mut anns = self.load(sid)?;
        anns.push(ann);
        self.save(sid, &anns)
    }

    /// Remove the comment with `id` from a session (no-op if absent).
    pub fn delete(&self, sid: SessionId, id: Uuid) -> Result<()> {
        let mut anns = self.load(sid)?;
        anns.retain(|a| a.id != id);
        self.save(sid, &anns)
    }

    /// Session ids with at least one not-yet-applied comment. Scans the store
    /// directory (a missing directory yields an empty set); unreadable or
    /// malformed files are skipped rather than failing the whole scan.
    pub fn sessions_with_pending(&self) -> Result<std::collections::HashSet<SessionId>> {
        let mut out = std::collections::HashSet::new();
        let entries = match fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(ConfigError::LoadFailed(e.to_string()).into()),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(uuid) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| Uuid::parse_str(s).ok())
            else {
                continue;
            };
            let sid = SessionId::from_uuid(uuid);
            if self
                .load(sid)
                .is_ok_and(|cs| cs.iter().any(|c| c.status != CommentStatus::Applied))
            {
                out.insert(sid);
            }
        }
        Ok(out)
    }

    /// Set the status of one comment.
    pub fn set_status(&self, sid: SessionId, id: Uuid, status: CommentStatus) -> Result<()> {
        let mut anns = self.load(sid)?;
        if let Some(a) = anns.iter_mut().find(|a| a.id == id) {
            a.status = status;
        }
        self.save(sid, &anns)
    }
}

/// Collect the `(lineno, content)` stream for `side` of a file's diff (the
/// lines that exist on that side: context + additions for New, context +
/// deletions for Old).
fn side_lines(file: &FileDiff, side: CommentSide) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    for hunk in &file.hunks {
        for line in &hunk.lines {
            let (keep, lineno) = match side {
                CommentSide::New => (
                    matches!(line.origin, LineOrigin::Context | LineOrigin::Addition),
                    line.new_lineno,
                ),
                CommentSide::Old => (
                    matches!(line.origin, LineOrigin::Context | LineOrigin::Deletion),
                    line.old_lineno,
                ),
            };
            if keep && let Some(n) = lineno {
                out.push((n, line.content.as_str()));
            }
        }
    }
    out
}

/// Try to re-locate an comment's snippet in `diff`. A single contiguous
/// match yields [`AnchorResult::Located`]; zero or multiple matches (ambiguous)
/// both yield [`AnchorResult::Drifted`]. Trailing whitespace is ignored when
/// comparing.
pub fn reanchor(ann: &Comment, diff: &ParsedDiff) -> AnchorResult {
    let Some(file) = diff
        .files
        .iter()
        .find(|f| f.display_path() == ann.file || f.new_path == ann.file || f.old_path == ann.file)
    else {
        return AnchorResult::Drifted;
    };

    let needle: Vec<&str> = ann.snippet.split('\n').collect();
    if needle.is_empty() {
        return AnchorResult::Drifted;
    }
    let hay = side_lines(file, ann.side);
    if hay.len() < needle.len() {
        return AnchorResult::Drifted;
    }

    let norm = str::trim_end;
    let mut found: Option<(usize, usize)> = None;
    for start in 0..=(hay.len() - needle.len()) {
        let matches = (0..needle.len()).all(|i| norm(hay[start + i].1) == norm(needle[i]));
        if matches {
            if found.is_some() {
                // Ambiguous — more than one match.
                return AnchorResult::Drifted;
            }
            found = Some((hay[start].0, hay[start + needle.len() - 1].0));
        }
    }

    match found {
        Some(line_range) => AnchorResult::Located {
            side: ann.side,
            line_range,
        },
        None => AnchorResult::Drifted,
    }
}

/// Re-anchor every non-applied comment against `diff` in place: located ones
/// become [`CommentStatus::Staged`] with an updated range; unlocatable ones
/// become [`CommentStatus::Drifted`]. Applied comments are left untouched.
pub fn reanchor_comments(anns: &mut [Comment], diff: &ParsedDiff) {
    for ann in anns.iter_mut() {
        if ann.status == CommentStatus::Applied {
            continue;
        }
        match reanchor(ann, diff) {
            AnchorResult::Located { line_range, .. } => {
                ann.line_range = line_range;
                ann.status = CommentStatus::Staged;
            }
            AnchorResult::Drifted => ann.status = CommentStatus::Drifted,
        }
    }
}

/// Whether any comment is drifted (and therefore blocks Apply).
pub fn has_blocking_drift(anns: &[Comment]) -> bool {
    anns.iter().any(|a| a.status == CommentStatus::Drifted)
}

/// Markdown fence language hint for a path's extension (empty when unknown).
fn lang_for(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("rs") => "rust",
        Some("py") => "python",
        Some("js") | Some("jsx") => "javascript",
        Some("ts") | Some("tsx") => "typescript",
        Some("go") => "go",
        Some("toml") => "toml",
        Some("json") => "json",
        Some("md") => "markdown",
        Some("sh") | Some("bash") => "bash",
        Some("yml") | Some("yaml") => "yaml",
        _ => "",
    }
}

/// Compose a markdown brief from a set of comments, suitable for handing to
/// the agent. Deterministic (no ids or timestamps) so it is snapshot-stable.
pub fn compose_markdown(title: &str, anns: &[Comment]) -> String {
    let mut out = format!("# Review comments: {title}\n");
    for a in anns {
        let (lo, hi) = a.line_range;
        let loc = if lo == hi {
            lo.to_string()
        } else {
            format!("{lo}-{hi}")
        };
        out.push_str(&format!("\n## {}:{}\n", a.file, loc));
        out.push_str(&format!("```{}\n{}\n```\n", lang_for(&a.file), a.snippet));
        out.push_str(&format!("{}\n", a.comment));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::parse_unified_diff;
    use tempfile::TempDir;

    fn ann(file: &str, side: CommentSide, range: (usize, usize), snippet: &str) -> Comment {
        Comment::new(file, side, range, snippet, "do the thing")
    }

    // --- store ---

    #[test]
    fn load_missing_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let store = CommentStore::new(tmp.path().join("comments"));
        assert!(store.load(SessionId::new()).unwrap().is_empty());
    }

    #[test]
    fn add_then_load_round_trips() {
        let tmp = TempDir::new().unwrap();
        let store = CommentStore::new(tmp.path().join("comments"));
        let sid = SessionId::new();
        let a = ann("src/foo.rs", CommentSide::New, (10, 12), "let x = 1;");
        store.add(sid, a.clone()).unwrap();

        let loaded = store.load(sid).unwrap();
        assert_eq!(loaded, vec![a]);
    }

    #[test]
    fn delete_removes_only_target() {
        let tmp = TempDir::new().unwrap();
        let store = CommentStore::new(tmp.path().join("comments"));
        let sid = SessionId::new();
        let a = ann("a.rs", CommentSide::New, (1, 1), "a");
        let b = ann("b.rs", CommentSide::New, (2, 2), "b");
        store.add(sid, a.clone()).unwrap();
        store.add(sid, b.clone()).unwrap();

        store.delete(sid, a.id).unwrap();
        let loaded = store.load(sid).unwrap();
        assert_eq!(loaded, vec![b]);
    }

    #[test]
    fn set_status_updates_target() {
        let tmp = TempDir::new().unwrap();
        let store = CommentStore::new(tmp.path().join("comments"));
        let sid = SessionId::new();
        let a = ann("a.rs", CommentSide::New, (1, 1), "a");
        store.add(sid, a.clone()).unwrap();

        store.set_status(sid, a.id, CommentStatus::Applied).unwrap();
        assert_eq!(store.load(sid).unwrap()[0].status, CommentStatus::Applied);
    }

    #[test]
    fn sessions_with_pending_lists_only_unapplied() {
        let tmp = TempDir::new().unwrap();
        let store = CommentStore::new(tmp.path().join("comments"));

        // Empty (missing dir) → empty set.
        assert!(store.sessions_with_pending().unwrap().is_empty());

        let staged = SessionId::new();
        store
            .add(staged, ann("a.rs", CommentSide::New, (1, 1), "a"))
            .unwrap();

        // A session whose only comment has been applied is not pending.
        let applied = SessionId::new();
        let a = ann("b.rs", CommentSide::New, (1, 1), "b");
        store.add(applied, a.clone()).unwrap();
        store
            .set_status(applied, a.id, CommentStatus::Applied)
            .unwrap();

        let pending = store.sessions_with_pending().unwrap();
        assert!(pending.contains(&staged));
        assert!(!pending.contains(&applied));
    }

    // --- reanchor ---

    fn diff_with_inserted_line() -> ParsedDiff {
        parse_unified_diff(
            "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,3 @@
 fn main() {
+    let y = 3;
 }
",
        )
    }

    #[test]
    fn reanchor_single_match_relocates_range() {
        let diff = diff_with_inserted_line();
        // Stored range is stale (99); the snippet actually sits at new line 2.
        let a = ann("a.rs", CommentSide::New, (99, 99), "    let y = 3;");
        assert_eq!(
            reanchor(&a, &diff),
            AnchorResult::Located {
                side: CommentSide::New,
                line_range: (2, 2),
            }
        );
    }

    #[test]
    fn reanchor_multiline_snippet() {
        let diff = diff_with_inserted_line();
        let a = ann(
            "a.rs",
            CommentSide::New,
            (1, 2),
            "fn main() {\n    let y = 3;",
        );
        assert_eq!(
            reanchor(&a, &diff),
            AnchorResult::Located {
                side: CommentSide::New,
                line_range: (1, 2),
            }
        );
    }

    #[test]
    fn reanchor_no_match_is_drifted() {
        let diff = diff_with_inserted_line();
        let a = ann("a.rs", CommentSide::New, (2, 2), "    let z = 9;");
        assert_eq!(reanchor(&a, &diff), AnchorResult::Drifted);
    }

    #[test]
    fn reanchor_ambiguous_match_is_drifted() {
        let diff = parse_unified_diff(
            "\
diff --git a/d.rs b/d.rs
--- /dev/null
+++ b/d.rs
@@ -0,0 +1,2 @@
+dup
+dup
",
        );
        let a = ann("d.rs", CommentSide::New, (1, 1), "dup");
        assert_eq!(reanchor(&a, &diff), AnchorResult::Drifted);
    }

    #[test]
    fn reanchor_missing_file_is_drifted() {
        let diff = diff_with_inserted_line();
        let a = ann("other.rs", CommentSide::New, (1, 1), "fn main() {");
        assert_eq!(reanchor(&a, &diff), AnchorResult::Drifted);
    }

    #[test]
    fn reanchor_comments_updates_status_and_skips_applied() {
        let diff = diff_with_inserted_line();
        let mut located = ann("a.rs", CommentSide::New, (99, 99), "    let y = 3;");
        let mut gone = ann("a.rs", CommentSide::New, (1, 1), "    let z = 9;");
        let mut applied = ann("a.rs", CommentSide::New, (1, 1), "    let z = 9;");
        applied.status = CommentStatus::Applied;

        let mut all = vec![located.clone(), gone.clone(), applied.clone()];
        reanchor_comments(&mut all, &diff);

        located = all[0].clone();
        gone = all[1].clone();
        applied = all[2].clone();
        assert_eq!(located.status, CommentStatus::Staged);
        assert_eq!(located.line_range, (2, 2));
        assert_eq!(gone.status, CommentStatus::Drifted);
        // Applied comments are not re-evaluated.
        assert_eq!(applied.status, CommentStatus::Applied);
    }

    #[test]
    fn has_blocking_drift_detects_drifted() {
        let mut a = ann("a.rs", CommentSide::New, (1, 1), "x");
        assert!(!has_blocking_drift(std::slice::from_ref(&a)));
        a.status = CommentStatus::Drifted;
        assert!(has_blocking_drift(std::slice::from_ref(&a)));
    }

    // --- markdown ---

    #[test]
    fn compose_markdown_is_deterministic() {
        let mut a = ann(
            "src/foo.rs",
            CommentSide::New,
            (10, 12),
            "let x = 1;\nlet y = 2;",
        );
        a.comment = "extract a helper".to_string();
        let mut b = ann("README.md", CommentSide::New, (3, 3), "# Title");
        b.comment = "fix the heading".to_string();

        let md = compose_markdown("my session", &[a, b]);
        let expected = "\
# Review comments: my session

## src/foo.rs:10-12
```rust
let x = 1;
let y = 2;
```
extract a helper

## README.md:3
```markdown
# Title
```
fix the heading
";
        assert_eq!(md, expected);
    }
}
