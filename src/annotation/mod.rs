//! Local diff-review annotations.
//!
//! An annotation is a comment the user attaches to a line range in a session's
//! review diff. Annotations are *staged* (persisted across restarts) until the
//! user applies them, at which point they are composed into a markdown brief
//! and handed to the agent (see the service layer). The captured `snippet` is
//! stored so a comment can be re-anchored even after the surrounding code
//! drifts; if it can no longer be located unambiguously, the annotation is
//! marked [`AnnotationStatus::Drifted`] and blocks Apply.
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
pub enum AnnotationSide {
    Old,
    New,
}

/// Lifecycle status of an annotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationStatus {
    /// Anchored to the current diff, ready to apply.
    Staged,
    /// Its snippet could not be located unambiguously in the current diff;
    /// blocks Apply until reviewed or deleted.
    Drifted,
    /// Already sent to the agent.
    Applied,
}

/// A single review annotation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Annotation {
    pub id: Uuid,
    /// Display path of the file (new path, or old path for deletions).
    pub file: String,
    pub side: AnnotationSide,
    /// Inclusive line range on `side` at capture / last successful anchor time.
    pub line_range: (usize, usize),
    /// Captured line contents (joined with `\n`), used to re-anchor.
    pub snippet: String,
    pub comment: String,
    pub status: AnnotationStatus,
    pub created_at: DateTime<Utc>,
}

impl Annotation {
    /// Create a freshly staged annotation with a random id and `created_at`
    /// set to now. `snippet` is normalised to drop any trailing newline.
    pub fn new(
        file: impl Into<String>,
        side: AnnotationSide,
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
            status: AnnotationStatus::Staged,
            created_at: Utc::now(),
        }
    }
}

/// Outcome of trying to locate an annotation's snippet in a fresh diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorResult {
    Located {
        side: AnnotationSide,
        line_range: (usize, usize),
    },
    Drifted,
}

/// Per-session annotation store, persisted as one JSON array per session under
/// a directory (typically `<data_dir>/annotations/`).
pub struct AnnotationStore {
    dir: PathBuf,
}

impl AnnotationStore {
    /// Construct a store rooted at `dir` (created lazily on first save).
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Default store at `<data_dir>/annotations/`.
    pub fn open_default() -> Result<Self> {
        Ok(Self::new(Config::data_dir()?.join("annotations")))
    }

    fn path_for(&self, sid: SessionId) -> PathBuf {
        self.dir.join(format!("{}.json", sid.as_uuid()))
    }

    /// Load a session's annotations (an absent file yields an empty list).
    pub fn load(&self, sid: SessionId) -> Result<Vec<Annotation>> {
        match fs::read_to_string(self.path_for(sid)) {
            Ok(s) => {
                Ok(serde_json::from_str(&s).map_err(|e| ConfigError::LoadFailed(e.to_string()))?)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(ConfigError::LoadFailed(e.to_string()).into()),
        }
    }

    /// Persist a session's annotations via a temp-file + rename (atomic).
    pub fn save(&self, sid: SessionId, anns: &[Annotation]) -> Result<()> {
        fs::create_dir_all(&self.dir).map_err(|e| ConfigError::SaveFailed(e.to_string()))?;
        let json = serde_json::to_string_pretty(anns)
            .map_err(|e| ConfigError::SaveFailed(e.to_string()))?;
        let tmp = self.dir.join(format!(".{}.tmp", sid.as_uuid()));
        fs::write(&tmp, json).map_err(|e| ConfigError::SaveFailed(e.to_string()))?;
        fs::rename(&tmp, self.path_for(sid)).map_err(|e| ConfigError::SaveFailed(e.to_string()))?;
        Ok(())
    }

    /// Append an annotation to a session.
    pub fn add(&self, sid: SessionId, ann: Annotation) -> Result<()> {
        let mut anns = self.load(sid)?;
        anns.push(ann);
        self.save(sid, &anns)
    }

    /// Remove the annotation with `id` from a session (no-op if absent).
    pub fn delete(&self, sid: SessionId, id: Uuid) -> Result<()> {
        let mut anns = self.load(sid)?;
        anns.retain(|a| a.id != id);
        self.save(sid, &anns)
    }

    /// Set the status of one annotation.
    pub fn set_status(&self, sid: SessionId, id: Uuid, status: AnnotationStatus) -> Result<()> {
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
fn side_lines(file: &FileDiff, side: AnnotationSide) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    for hunk in &file.hunks {
        for line in &hunk.lines {
            let (keep, lineno) = match side {
                AnnotationSide::New => (
                    matches!(line.origin, LineOrigin::Context | LineOrigin::Addition),
                    line.new_lineno,
                ),
                AnnotationSide::Old => (
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

/// Try to re-locate an annotation's snippet in `diff`. A single contiguous
/// match yields [`AnchorResult::Located`]; zero or multiple matches (ambiguous)
/// both yield [`AnchorResult::Drifted`]. Trailing whitespace is ignored when
/// comparing.
pub fn reanchor(ann: &Annotation, diff: &ParsedDiff) -> AnchorResult {
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

/// Re-anchor every non-applied annotation against `diff` in place: located ones
/// become [`AnnotationStatus::Staged`] with an updated range; unlocatable ones
/// become [`AnnotationStatus::Drifted`]. Applied annotations are left untouched.
pub fn reanchor_annotations(anns: &mut [Annotation], diff: &ParsedDiff) {
    for ann in anns.iter_mut() {
        if ann.status == AnnotationStatus::Applied {
            continue;
        }
        match reanchor(ann, diff) {
            AnchorResult::Located { line_range, .. } => {
                ann.line_range = line_range;
                ann.status = AnnotationStatus::Staged;
            }
            AnchorResult::Drifted => ann.status = AnnotationStatus::Drifted,
        }
    }
}

/// Whether any annotation is drifted (and therefore blocks Apply).
pub fn has_blocking_drift(anns: &[Annotation]) -> bool {
    anns.iter().any(|a| a.status == AnnotationStatus::Drifted)
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

/// Compose a markdown brief from a set of annotations, suitable for handing to
/// the agent. Deterministic (no ids or timestamps) so it is snapshot-stable.
pub fn compose_markdown(title: &str, anns: &[Annotation]) -> String {
    let mut out = format!("# Review annotations: {title}\n");
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

    fn ann(file: &str, side: AnnotationSide, range: (usize, usize), snippet: &str) -> Annotation {
        Annotation::new(file, side, range, snippet, "do the thing")
    }

    // --- store ---

    #[test]
    fn load_missing_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let store = AnnotationStore::new(tmp.path().join("annotations"));
        assert!(store.load(SessionId::new()).unwrap().is_empty());
    }

    #[test]
    fn add_then_load_round_trips() {
        let tmp = TempDir::new().unwrap();
        let store = AnnotationStore::new(tmp.path().join("annotations"));
        let sid = SessionId::new();
        let a = ann("src/foo.rs", AnnotationSide::New, (10, 12), "let x = 1;");
        store.add(sid, a.clone()).unwrap();

        let loaded = store.load(sid).unwrap();
        assert_eq!(loaded, vec![a]);
    }

    #[test]
    fn delete_removes_only_target() {
        let tmp = TempDir::new().unwrap();
        let store = AnnotationStore::new(tmp.path().join("annotations"));
        let sid = SessionId::new();
        let a = ann("a.rs", AnnotationSide::New, (1, 1), "a");
        let b = ann("b.rs", AnnotationSide::New, (2, 2), "b");
        store.add(sid, a.clone()).unwrap();
        store.add(sid, b.clone()).unwrap();

        store.delete(sid, a.id).unwrap();
        let loaded = store.load(sid).unwrap();
        assert_eq!(loaded, vec![b]);
    }

    #[test]
    fn set_status_updates_target() {
        let tmp = TempDir::new().unwrap();
        let store = AnnotationStore::new(tmp.path().join("annotations"));
        let sid = SessionId::new();
        let a = ann("a.rs", AnnotationSide::New, (1, 1), "a");
        store.add(sid, a.clone()).unwrap();

        store
            .set_status(sid, a.id, AnnotationStatus::Applied)
            .unwrap();
        assert_eq!(
            store.load(sid).unwrap()[0].status,
            AnnotationStatus::Applied
        );
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
        let a = ann("a.rs", AnnotationSide::New, (99, 99), "    let y = 3;");
        assert_eq!(
            reanchor(&a, &diff),
            AnchorResult::Located {
                side: AnnotationSide::New,
                line_range: (2, 2),
            }
        );
    }

    #[test]
    fn reanchor_multiline_snippet() {
        let diff = diff_with_inserted_line();
        let a = ann(
            "a.rs",
            AnnotationSide::New,
            (1, 2),
            "fn main() {\n    let y = 3;",
        );
        assert_eq!(
            reanchor(&a, &diff),
            AnchorResult::Located {
                side: AnnotationSide::New,
                line_range: (1, 2),
            }
        );
    }

    #[test]
    fn reanchor_no_match_is_drifted() {
        let diff = diff_with_inserted_line();
        let a = ann("a.rs", AnnotationSide::New, (2, 2), "    let z = 9;");
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
        let a = ann("d.rs", AnnotationSide::New, (1, 1), "dup");
        assert_eq!(reanchor(&a, &diff), AnchorResult::Drifted);
    }

    #[test]
    fn reanchor_missing_file_is_drifted() {
        let diff = diff_with_inserted_line();
        let a = ann("other.rs", AnnotationSide::New, (1, 1), "fn main() {");
        assert_eq!(reanchor(&a, &diff), AnchorResult::Drifted);
    }

    #[test]
    fn reanchor_annotations_updates_status_and_skips_applied() {
        let diff = diff_with_inserted_line();
        let mut located = ann("a.rs", AnnotationSide::New, (99, 99), "    let y = 3;");
        let mut gone = ann("a.rs", AnnotationSide::New, (1, 1), "    let z = 9;");
        let mut applied = ann("a.rs", AnnotationSide::New, (1, 1), "    let z = 9;");
        applied.status = AnnotationStatus::Applied;

        let mut all = vec![located.clone(), gone.clone(), applied.clone()];
        reanchor_annotations(&mut all, &diff);

        located = all[0].clone();
        gone = all[1].clone();
        applied = all[2].clone();
        assert_eq!(located.status, AnnotationStatus::Staged);
        assert_eq!(located.line_range, (2, 2));
        assert_eq!(gone.status, AnnotationStatus::Drifted);
        // Applied annotations are not re-evaluated.
        assert_eq!(applied.status, AnnotationStatus::Applied);
    }

    #[test]
    fn has_blocking_drift_detects_drifted() {
        let mut a = ann("a.rs", AnnotationSide::New, (1, 1), "x");
        assert!(!has_blocking_drift(std::slice::from_ref(&a)));
        a.status = AnnotationStatus::Drifted;
        assert!(has_blocking_drift(std::slice::from_ref(&a)));
    }

    // --- markdown ---

    #[test]
    fn compose_markdown_is_deterministic() {
        let mut a = ann(
            "src/foo.rs",
            AnnotationSide::New,
            (10, 12),
            "let x = 1;\nlet y = 2;",
        );
        a.comment = "extract a helper".to_string();
        let mut b = ann("README.md", AnnotationSide::New, (3, 3), "# Title");
        b.comment = "fix the heading".to_string();

        let md = compose_markdown("my session", &[a, b]);
        let expected = "\
# Review annotations: my session

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
