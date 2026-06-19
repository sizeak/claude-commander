//! Structured unified-diff model and parser for the review/diff view.
//!
//! Unlike [`super::diff`] (which produces only summary stats for the preview
//! pane), this module parses a unified diff into a `file -> hunk -> line`
//! structure with per-line old/new line numbers. The diff view uses that to
//! render gutters, drive line-range selection, and anchor comments.
//!
//! The parser is deliberately tolerant: it skips the metadata lines git emits
//! (`index`, mode, `similarity`, rename headers, `Binary files`) and copes with
//! several files concatenated with blank-line separators, as the review-diff
//! composition does when appending untracked-file patches.

use std::path::Path;
use std::process::Stdio;

use serde::Serialize;
use tokio::process::Command;
use xxhash_rust::xxh3::Xxh3;

use super::diff::untracked_patch_and_count;
use crate::error::{GitError, Result};

/// Compose the review diff for `worktree`: everything from the merge-base of
/// `base` and `HEAD` through the working tree (committed + staged + unstaged),
/// plus untracked files — i.e. the full set of changes a PR against `base`
/// would represent, plus any uncommitted work.
///
/// `base` is a commit-ish (branch name, SHA, or `"HEAD"`). When the merge-base
/// can't be resolved (e.g. a stacked-PR base that only exists remotely), the
/// diff degrades to working-tree-vs-`HEAD` rather than failing the view.
pub async fn compose_review_diff(worktree: &Path, base: &str) -> Result<String> {
    let target = diff_target(worktree, base).await;

    // Force standard `a/`/`b/` prefixes so the parser is independent of the
    // user's `diff.mnemonicPrefix` config (which would emit `i/`/`w/`/`c/`).
    let diff_out = Command::new("git")
        .current_dir(worktree)
        .args(["diff", "--src-prefix=a/", "--dst-prefix=b/", &target])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| GitError::DiffFailed(e.to_string()))?;

    let mut diff = if diff_out.status.success() {
        String::from_utf8_lossy(&diff_out.stdout).to_string()
    } else {
        // `base`/`target` unresolvable — degrade to working-tree-vs-HEAD.
        let head = Command::new("git")
            .current_dir(worktree)
            .args(["diff", "--src-prefix=a/", "--dst-prefix=b/", "HEAD"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| GitError::DiffFailed(e.to_string()))?;
        String::from_utf8_lossy(&head.stdout).to_string()
    };

    let (untracked, _) = untracked_patch_and_count(worktree).await;
    if !untracked.is_empty() {
        if !diff.is_empty() && !diff.ends_with('\n') {
            diff.push('\n');
        }
        diff.push_str(&untracked);
    }

    Ok(diff)
}

/// Resolve the diff base for a PR target branch, preferring the
/// `origin/<branch>` remote-tracking ref when it exists so the review diff
/// reflects the pushed upstream rather than a possibly-stale local branch.
/// Falls back to the bare (local) branch name when no remote-tracking ref is
/// present.
pub async fn prefer_remote_branch(worktree: &Path, branch: &str) -> String {
    let remote = format!("origin/{branch}");
    if ref_exists(worktree, &remote).await {
        remote
    } else {
        branch.to_string()
    }
}

/// Whether `refname` resolves to a commit in `worktree`.
async fn ref_exists(worktree: &Path, refname: &str) -> bool {
    Command::new("git")
        .current_dir(worktree)
        .args(["rev-parse", "--verify", "--quiet", refname])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The commit-ish the review diff treats as "base": the merge-base of `base`
/// and `HEAD`, falling back to `base` verbatim when it can't be computed. Both
/// the diff composition and binary blob reads resolve the old side through this
/// so they always agree on which commit the "before" image comes from.
async fn diff_target(worktree: &Path, base: &str) -> String {
    merge_base(worktree, base)
        .await
        .unwrap_or_else(|| base.to_string())
}

/// Read the base-side bytes of `path` as the review diff sees them: the blob at
/// [`diff_target`]. Returns an error when the path doesn't exist there (e.g. an
/// added file has no base side). Runs `git show` as a subprocess, so the read
/// happens off the async runtime's worker threads.
pub async fn read_base_blob(worktree: &Path, base: &str, path: &str) -> Result<Vec<u8>> {
    let target = diff_target(worktree, base).await;
    let spec = format!("{target}:{path}");
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["show", &spec])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| GitError::OperationFailed(e.to_string()))?;
    if !out.status.success() {
        return Err(GitError::OperationFailed(format!(
            "git show {spec}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
        .into());
    }
    Ok(out.stdout)
}

/// Read the working-tree (new-side) bytes of `path`. Uses async `tokio::fs` so
/// the read never blocks the executor.
pub async fn read_worktree_file(worktree: &Path, path: &str) -> Result<Vec<u8>> {
    Ok(tokio::fs::read(worktree.join(path))
        .await
        .map_err(|e| GitError::OperationFailed(e.to_string()))?)
}

/// `git cat-file -s <oid>` — the byte size of a blob without reading its
/// contents. `None` if the lookup fails (e.g. an absent/abbreviated oid).
async fn blob_size(worktree: &Path, oid: &str) -> Option<u64> {
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["cat-file", "-s", oid])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Fill in the binary file sizes the parser leaves `None`: the base blob size
/// via `git cat-file -s`, and the new-side size from the working-tree file on
/// disk. Best effort — a size stays `None` if its lookup fails.
pub async fn enrich_binary_sizes(diff: &mut ParsedDiff, worktree: &Path) {
    for f in &mut diff.files {
        let new_path = f.new_path.clone();
        let Some(info) = f.binary.as_mut() else {
            continue;
        };
        if let Some(oid) = info.old_oid.clone() {
            info.old_size = blob_size(worktree, &oid).await;
        }
        if info.new_oid.is_some() {
            // The new side is the working tree, so its size is the file on disk.
            info.new_size = tokio::fs::metadata(worktree.join(&new_path))
                .await
                .ok()
                .map(|m| m.len());
        }
    }
}

/// Resolve `git merge-base <base> HEAD`, returning `None` if it cannot be
/// computed (so the caller can fall back).
async fn merge_base(worktree: &Path, base: &str) -> Option<String> {
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["merge-base", base, "HEAD"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// Origin of a single diff line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LineOrigin {
    Context,
    Addition,
    Deletion,
}

/// A single line within a hunk, with resolved old/new line numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffLine {
    pub origin: LineOrigin,
    /// Line number on the old side (`None` for additions).
    pub old_lineno: Option<usize>,
    /// Line number on the new side (`None` for deletions).
    pub new_lineno: Option<usize>,
    /// Line content without the leading `+`/`-`/space marker.
    pub content: String,
}

/// A contiguous block of changes (one `@@ ... @@` section).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    /// Text following the closing `@@` (the section heading), if any.
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// How a file changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
}

/// What kind of binary a file is, for consumers deciding how to render it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BinaryKind {
    /// A raster image we can render, tagged with its MIME type (e.g.
    /// `image/png`) so a GUI can build a `data:` URL directly.
    Image { mime: String },
    /// Any other binary blob (rendered as a placeholder, not an image).
    Other,
}

/// Metadata for a binary file's diff. The bytes themselves are NOT inlined
/// here — consumers lazy-load them via `CommanderService::fetch_diff_blob`
/// keyed by `(side, path)`. `old_*`/`new_*` are `None` on the side that does
/// not exist (added files have no old side; deleted files have no new side).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BinaryInfo {
    pub kind: BinaryKind,
    /// Base-side blob oid (from the diff `index` line), if present.
    pub old_oid: Option<String>,
    /// Working-tree-side blob oid (from the diff `index` line), if present.
    pub new_oid: Option<String>,
    /// Base-side blob size in bytes, filled in by `open_review` (not the parser).
    pub old_size: Option<u64>,
    /// Working-tree-side size in bytes, filled in by `open_review`.
    pub new_size: Option<u64>,
}

/// All changes to a single file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileDiff {
    pub old_path: String,
    pub new_path: String,
    pub status: FileStatus,
    pub added: usize,
    pub removed: usize,
    pub hunks: Vec<Hunk>,
    /// `Some` when this file is binary (no textual hunks); `None` for text.
    pub binary: Option<BinaryInfo>,
}

impl FileDiff {
    /// Path to show in the file list: the new path, except for deletions where
    /// only the old path is meaningful.
    pub fn display_path(&self) -> &str {
        if self.status == FileStatus::Deleted {
            &self.old_path
        } else {
            &self.new_path
        }
    }
}

/// Stable content hash of a file's diff: xxh3_64 over a canonical byte
/// rendering of its hunks. Changes whenever any hunk header (position) or
/// line changes; deliberately excludes the path, which is the identity key.
pub fn file_diff_hash(file: &FileDiff) -> u64 {
    let mut h = Xxh3::new();
    for hunk in &file.hunks {
        h.update(
            format!(
                "@@ -{},{} +{},{} @@{}\n",
                hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines, hunk.header
            )
            .as_bytes(),
        );
        for line in &hunk.lines {
            let origin = match line.origin {
                LineOrigin::Addition => b'+',
                LineOrigin::Deletion => b'-',
                LineOrigin::Context => b' ',
            };
            h.update(&[origin]);
            h.update(line.content.as_bytes());
            h.update(b"\n");
        }
    }
    h.digest()
}

/// A parsed unified diff: an ordered list of changed files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ParsedDiff {
    pub files: Vec<FileDiff>,
}

impl ParsedDiff {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Parse a unified diff (the output of `git diff`) into a [`ParsedDiff`].
pub fn parse_unified_diff(raw: &str) -> ParsedDiff {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut cur: Option<FileBuilder> = None;

    for line in raw.lines() {
        // New file section.
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(b) = cur.take() {
                files.push(b.finish());
            }
            let mut b = FileBuilder::default();
            // Best-effort fallback paths; `---`/`+++` below override these.
            if let Some((old, new)) = split_diff_git_paths(rest) {
                b.old_path = old;
                b.new_path = new;
            }
            cur = Some(b);
            continue;
        }

        let Some(b) = cur.as_mut() else {
            // Anything before the first `diff --git` (e.g. blank separators).
            continue;
        };

        // Hunk header starts a new hunk.
        if line.starts_with("@@") {
            b.flush_hunk();
            if let Some(h) = parse_hunk_header(line) {
                b.old_lineno = h.old_start;
                b.new_lineno = h.new_start;
                b.cur_hunk = Some(h);
            }
            continue;
        }

        // Body lines, while inside a hunk.
        if b.cur_hunk.is_some() {
            match line.as_bytes().first() {
                Some(b' ') => {
                    let (old, new) = (b.old_lineno, b.new_lineno);
                    b.push_line(DiffLine {
                        origin: LineOrigin::Context,
                        old_lineno: Some(old),
                        new_lineno: Some(new),
                        content: line[1..].to_string(),
                    });
                    b.old_lineno += 1;
                    b.new_lineno += 1;
                }
                Some(b'+') => {
                    let new = b.new_lineno;
                    b.push_line(DiffLine {
                        origin: LineOrigin::Addition,
                        old_lineno: None,
                        new_lineno: Some(new),
                        content: line[1..].to_string(),
                    });
                    b.new_lineno += 1;
                    b.added += 1;
                }
                Some(b'-') => {
                    let old = b.old_lineno;
                    b.push_line(DiffLine {
                        origin: LineOrigin::Deletion,
                        old_lineno: Some(old),
                        new_lineno: None,
                        content: line[1..].to_string(),
                    });
                    b.old_lineno += 1;
                    b.removed += 1;
                }
                // "\ No newline at end of file" is not a content line.
                Some(b'\\') => {}
                // Blank/unexpected line ends the hunk.
                _ => b.flush_hunk(),
            }
            continue;
        }

        // File metadata lines (only relevant before the first hunk).
        if let Some(p) = line.strip_prefix("--- ") {
            b.old_path = parse_header_path(p);
        } else if let Some(p) = line.strip_prefix("+++ ") {
            b.new_path = parse_header_path(p);
        } else if line.starts_with("new file mode") {
            b.added_file = true;
        } else if line.starts_with("deleted file mode") {
            b.deleted_file = true;
        } else if let Some(from) = line.strip_prefix("rename from ") {
            b.renamed = true;
            b.old_path = from.to_string();
        } else if let Some(to) = line.strip_prefix("rename to ") {
            b.renamed = true;
            b.new_path = to.to_string();
        } else if let Some(rest) = line.strip_prefix("index ") {
            // `index <old>..<new>[ <mode>]` — capture both blob oids so binary
            // metadata can carry them. Harmless to record for text files too.
            if let Some((old, new)) = rest.split_once("..") {
                b.old_oid = Some(old.trim().to_string());
                // The new oid may be followed by a file mode; keep just the oid.
                b.new_oid = new.split_whitespace().next().map(str::to_string);
            }
        } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            // git emits "Binary files a/x and b/x differ" (default) or a
            // "GIT binary patch" header (with --binary) — either marks a binary.
            b.binary = true;
        }
        // Everything else (mode, similarity) is ignored.
    }

    if let Some(b) = cur.take() {
        files.push(b.finish());
    }

    ParsedDiff { files }
}

/// Mutable accumulator for one file's diff.
#[derive(Default)]
struct FileBuilder {
    old_path: String,
    new_path: String,
    added_file: bool,
    deleted_file: bool,
    renamed: bool,
    binary: bool,
    old_oid: Option<String>,
    new_oid: Option<String>,
    added: usize,
    removed: usize,
    hunks: Vec<Hunk>,
    cur_hunk: Option<Hunk>,
    old_lineno: usize,
    new_lineno: usize,
}

impl FileBuilder {
    fn push_line(&mut self, line: DiffLine) {
        if let Some(h) = self.cur_hunk.as_mut() {
            h.lines.push(line);
        }
    }

    fn flush_hunk(&mut self) {
        if let Some(h) = self.cur_hunk.take() {
            self.hunks.push(h);
        }
    }

    fn finish(mut self) -> FileDiff {
        self.flush_hunk();
        let status = if self.renamed {
            FileStatus::Renamed
        } else if self.added_file || self.old_path == "/dev/null" {
            FileStatus::Added
        } else if self.deleted_file || self.new_path == "/dev/null" {
            FileStatus::Deleted
        } else {
            FileStatus::Modified
        };
        let binary = if self.binary {
            let display = if status == FileStatus::Deleted {
                self.old_path.as_str()
            } else {
                self.new_path.as_str()
            };
            let kind = match image_mime_for_path(display) {
                Some(mime) => BinaryKind::Image {
                    mime: mime.to_string(),
                },
                None => BinaryKind::Other,
            };
            Some(BinaryInfo {
                kind,
                old_oid: zero_oid_to_none(self.old_oid.take()),
                new_oid: zero_oid_to_none(self.new_oid.take()),
                old_size: None,
                new_size: None,
            })
        } else {
            None
        };
        FileDiff {
            old_path: self.old_path,
            new_path: self.new_path,
            status,
            added: self.added,
            removed: self.removed,
            hunks: self.hunks,
            binary,
        }
    }
}

/// Map an all-zero blob oid (git's "absent" sentinel for the missing side of an
/// add or delete) to `None`; pass any real oid through unchanged.
fn zero_oid_to_none(oid: Option<String>) -> Option<String> {
    oid.filter(|o| !o.is_empty() && o.bytes().any(|c| c != b'0'))
}

/// MIME type for a path's extension when it names a raster image we can render,
/// else `None` (treated as a generic binary). SVG is intentionally excluded:
/// git diffs it as text, and rasterizing it would need an extra dependency.
fn image_mime_for_path(path: &str) -> Option<&'static str> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "tif" | "tiff" => "image/tiff",
        _ => return None,
    })
}

/// Parse a `---`/`+++` header path, stripping the `a/`/`b/` prefix and handling
/// `/dev/null`.
fn parse_header_path(p: &str) -> String {
    if p == "/dev/null" {
        return "/dev/null".to_string();
    }
    p.strip_prefix("a/")
        .or_else(|| p.strip_prefix("b/"))
        .unwrap_or(p)
        .to_string()
}

/// Best-effort split of `a/old b/new` from a `diff --git` line. Only used as a
/// fallback when `---`/`+++`/rename headers are absent (e.g. mode-only change).
fn split_diff_git_paths(rest: &str) -> Option<(String, String)> {
    let a = rest.trim().strip_prefix("a/")?;
    let idx = a.find(" b/")?;
    Some((a[..idx].to_string(), a[idx + 3..].to_string()))
}

/// Parse a hunk header `@@ -old_start,old_lines +new_start,new_lines @@ heading`.
fn parse_hunk_header(line: &str) -> Option<Hunk> {
    let after = line.strip_prefix("@@ ")?;
    let close = after.find(" @@")?;
    let ranges = &after[..close];
    let header = after[close + 3..].trim_start().to_string();

    let mut parts = ranges.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let (old_start, old_lines) = parse_range(old)?;
    let (new_start, new_lines) = parse_range(new)?;

    Some(Hunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        header,
        lines: Vec::new(),
    })
}

/// Parse a `start,count` range; `count` defaults to 1 when omitted (`@@ -5 +5 @@`).
fn parse_range(s: &str) -> Option<(usize, usize)> {
    let mut it = s.split(',');
    let start: usize = it.next()?.parse().ok()?;
    let lines: usize = match it.next() {
        Some(n) => n.parse().ok()?,
        None => 1,
    };
    Some((start, lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_empty_to_empty() {
        let d = parse_unified_diff("");
        assert!(d.is_empty());
        assert_eq!(d, ParsedDiff::default());
    }

    #[test]
    fn tracks_old_and_new_line_numbers_across_two_hunks() {
        let raw = "\
diff --git a/src/foo.rs b/src/foo.rs
index 1111111..2222222 100644
--- a/src/foo.rs
+++ b/src/foo.rs
@@ -1,3 +1,4 @@
 fn main() {
-    println!(\"old\");
+    println!(\"new\");
+    println!(\"added\");
 }
@@ -10,2 +11,2 @@ fn other() {
 let x = 1;
-let y = 2;
+let y = 3;
";
        let d = parse_unified_diff(raw);
        assert_eq!(d.files.len(), 1);
        let f = &d.files[0];
        assert_eq!(f.status, FileStatus::Modified);
        assert_eq!(f.old_path, "src/foo.rs");
        assert_eq!(f.new_path, "src/foo.rs");
        assert_eq!(f.added, 3);
        assert_eq!(f.removed, 2);
        assert_eq!(f.hunks.len(), 2);

        let h0 = &f.hunks[0];
        assert_eq!((h0.old_start, h0.new_start), (1, 1));
        // " fn main() {" context, old 1 / new 1
        assert_eq!(h0.lines[0].origin, LineOrigin::Context);
        assert_eq!(h0.lines[0].old_lineno, Some(1));
        assert_eq!(h0.lines[0].new_lineno, Some(1));
        assert_eq!(h0.lines[0].content, "fn main() {");
        // "-    println!(\"old\");" deletion, old 2 / new None
        assert_eq!(h0.lines[1].origin, LineOrigin::Deletion);
        assert_eq!(h0.lines[1].old_lineno, Some(2));
        assert_eq!(h0.lines[1].new_lineno, None);
        // "+    println!(\"new\");" addition, old None / new 2
        assert_eq!(h0.lines[2].origin, LineOrigin::Addition);
        assert_eq!(h0.lines[2].old_lineno, None);
        assert_eq!(h0.lines[2].new_lineno, Some(2));
        // "+    println!(\"added\");" addition, new 3
        assert_eq!(h0.lines[3].new_lineno, Some(3));
        // " }" context, old 3 / new 4
        assert_eq!(h0.lines[4].origin, LineOrigin::Context);
        assert_eq!(h0.lines[4].old_lineno, Some(3));
        assert_eq!(h0.lines[4].new_lineno, Some(4));

        let h1 = &f.hunks[1];
        assert_eq!((h1.old_start, h1.new_start), (10, 11));
        assert_eq!(h1.header, "fn other() {");
        assert_eq!(h1.lines[0].old_lineno, Some(10));
        assert_eq!(h1.lines[0].new_lineno, Some(11));
        assert_eq!(h1.lines[1].origin, LineOrigin::Deletion);
        assert_eq!(h1.lines[1].old_lineno, Some(11));
        assert_eq!(h1.lines[2].origin, LineOrigin::Addition);
        assert_eq!(h1.lines[2].new_lineno, Some(12));
    }

    #[test]
    fn parses_multiple_files() {
        let raw = "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1 +1 @@
-a
+A
diff --git a/b.txt b/b.txt
--- a/b.txt
+++ b/b.txt
@@ -1 +1 @@
-b
+B
";
        let d = parse_unified_diff(raw);
        assert_eq!(d.files.len(), 2);
        assert_eq!(d.files[0].new_path, "a.txt");
        assert_eq!(d.files[1].new_path, "b.txt");
        // count omitted in "@@ -1 +1 @@" defaults to 1 line each side
        assert_eq!(d.files[0].hunks[0].old_lines, 1);
        assert_eq!(d.files[0].hunks[0].new_lines, 1);
    }

    #[test]
    fn detects_added_file_via_dev_null() {
        let raw = "\
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..1111111
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+hello
+world
";
        let d = parse_unified_diff(raw);
        let f = &d.files[0];
        assert_eq!(f.status, FileStatus::Added);
        assert_eq!(f.old_path, "/dev/null");
        assert_eq!(f.new_path, "new.txt");
        assert_eq!(f.display_path(), "new.txt");
        assert_eq!(f.added, 2);
        assert_eq!(f.removed, 0);
    }

    #[test]
    fn detects_deleted_file() {
        let raw = "\
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 1111111..0000000
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
";
        let d = parse_unified_diff(raw);
        let f = &d.files[0];
        assert_eq!(f.status, FileStatus::Deleted);
        assert_eq!(f.new_path, "/dev/null");
        // deletions display the old path
        assert_eq!(f.display_path(), "gone.txt");
        assert_eq!(f.removed, 1);
    }

    #[test]
    fn detects_rename_without_hunks() {
        let raw = "\
diff --git a/old.txt b/new.txt
similarity index 100%
rename from old.txt
rename to new.txt
";
        let d = parse_unified_diff(raw);
        let f = &d.files[0];
        assert_eq!(f.status, FileStatus::Renamed);
        assert_eq!(f.old_path, "old.txt");
        assert_eq!(f.new_path, "new.txt");
        assert!(f.hunks.is_empty());
    }

    #[test]
    fn modified_file_display_path_is_new_path() {
        let raw = "\
diff --git a/keep.txt b/keep.txt
--- a/keep.txt
+++ b/keep.txt
@@ -1 +1 @@
-x
+y
";
        let f = &parse_unified_diff(raw).files[0];
        assert_eq!(f.status, FileStatus::Modified);
        assert_eq!(f.display_path(), "keep.txt");
    }

    #[test]
    fn ignores_no_newline_marker() {
        let raw = "\
diff --git a/f b/f
--- a/f
+++ b/f
@@ -1 +1 @@
-old
\\ No newline at end of file
+new
\\ No newline at end of file
";
        let f = &parse_unified_diff(raw).files[0];
        assert_eq!(f.added, 1);
        assert_eq!(f.removed, 1);
        // the marker lines must not appear as content
        assert_eq!(f.hunks[0].lines.len(), 2);
    }

    // --- binary files ---

    #[test]
    fn text_file_is_not_binary() {
        let raw = one_file_diff("src/a.rs", "-1,2 +1,2", " ctx\n-old\n+new\n");
        let f = &parse_unified_diff(&raw).files[0];
        assert_eq!(f.binary, None);
    }

    #[test]
    fn parses_binary_modified_image() {
        let raw = "\
diff --git a/assets/logo.png b/assets/logo.png
index 1111111..2222222 100644
Binary files a/assets/logo.png and b/assets/logo.png differ
";
        let f = &parse_unified_diff(raw).files[0];
        assert_eq!(f.status, FileStatus::Modified);
        assert_eq!(f.display_path(), "assets/logo.png");
        let b = f.binary.as_ref().expect("binary info present");
        assert_eq!(
            b.kind,
            BinaryKind::Image {
                mime: "image/png".to_string()
            }
        );
        assert_eq!(b.old_oid.as_deref(), Some("1111111"));
        assert_eq!(b.new_oid.as_deref(), Some("2222222"));
        // Sizes are enriched by open_review, not the parser.
        assert_eq!(b.old_size, None);
        assert_eq!(b.new_size, None);
        // Binary files carry no textual hunks.
        assert!(f.hunks.is_empty());
    }

    #[test]
    fn parses_binary_added_image_with_no_old_side() {
        let raw = "\
diff --git a/new.jpg b/new.jpg
new file mode 100644
index 0000000..3333333
Binary files /dev/null and b/new.jpg differ
";
        let f = &parse_unified_diff(raw).files[0];
        assert_eq!(f.status, FileStatus::Added);
        let b = f.binary.as_ref().expect("binary info present");
        assert_eq!(
            b.kind,
            BinaryKind::Image {
                mime: "image/jpeg".to_string()
            }
        );
        // All-zero base oid is the "absent side" sentinel -> None.
        assert_eq!(b.old_oid, None);
        assert_eq!(b.new_oid.as_deref(), Some("3333333"));
    }

    #[test]
    fn parses_binary_deleted_image_with_no_new_side() {
        let raw = "\
diff --git a/old.gif b/old.gif
deleted file mode 100644
index 4444444..0000000
Binary files a/old.gif and /dev/null differ
";
        let f = &parse_unified_diff(raw).files[0];
        assert_eq!(f.status, FileStatus::Deleted);
        // Deletions resolve kind from the old (display) path.
        assert_eq!(f.display_path(), "old.gif");
        let b = f.binary.as_ref().expect("binary info present");
        assert_eq!(
            b.kind,
            BinaryKind::Image {
                mime: "image/gif".to_string()
            }
        );
        assert_eq!(b.old_oid.as_deref(), Some("4444444"));
        assert_eq!(b.new_oid, None);
    }

    #[test]
    fn non_image_binary_is_kind_other() {
        let raw = "\
diff --git a/data.bin b/data.bin
index 5555555..6666666 100644
Binary files a/data.bin and b/data.bin differ
";
        let f = &parse_unified_diff(raw).files[0];
        let b = f.binary.as_ref().expect("binary info present");
        assert_eq!(b.kind, BinaryKind::Other);
    }

    // --- composition (real git repos via TempDir) ---

    use std::fs;
    use tempfile::TempDir;

    async fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(dir)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .expect("git command runs");
        assert!(status.success(), "git {args:?} failed");
    }

    async fn git_capture(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .expect("git command runs");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    async fn init_repo() -> TempDir {
        let tmp = TempDir::new().expect("tempdir");
        let p = tmp.path();
        git(p, &["init", "-q"]).await;
        git(p, &["config", "user.email", "test@example.com"]).await;
        git(p, &["config", "user.name", "Test"]).await;
        tmp
    }

    #[tokio::test]
    async fn includes_committed_change_against_base() {
        let tmp = init_repo().await;
        let p = tmp.path();

        fs::write(p.join("file.txt"), "v1\n").unwrap();
        git(p, &["add", "."]).await;
        git(p, &["commit", "-q", "-m", "A"]).await;
        let base = git_capture(p, &["rev-parse", "HEAD"]).await;

        // A committed change on top of the base. `git diff HEAD` would be empty.
        fs::write(p.join("file.txt"), "v2\n").unwrap();
        git(p, &["add", "."]).await;
        git(p, &["commit", "-q", "-m", "B"]).await;

        let diff = compose_review_diff(p, &base).await.unwrap();
        assert!(diff.contains("-v1"), "committed deletion missing:\n{diff}");
        assert!(diff.contains("+v2"), "committed addition missing:\n{diff}");

        // Sanity: the committed change is invisible to a plain working-tree diff.
        let head = git_capture(p, &["diff", "HEAD"]).await;
        assert!(head.is_empty(), "working tree should be clean: {head}");

        // And it round-trips through the parser.
        let parsed = parse_unified_diff(&diff);
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].display_path(), "file.txt");
    }

    #[tokio::test]
    async fn binary_image_change_survives_composition_and_parsing() {
        let tmp = init_repo().await;
        let p = tmp.path();

        // Bytes with embedded NULs so git classifies the file as binary.
        let v1: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x00v1\x00";
        let v2: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x00v2-longer\x00";
        fs::write(p.join("logo.png"), v1).unwrap();
        git(p, &["add", "."]).await;
        git(p, &["commit", "-q", "-m", "A"]).await;
        let base = git_capture(p, &["rev-parse", "HEAD"]).await;

        fs::write(p.join("logo.png"), v2).unwrap();

        let diff = compose_review_diff(p, &base).await.unwrap();
        let parsed = parse_unified_diff(&diff);
        let f = parsed
            .files
            .iter()
            .find(|f| f.display_path() == "logo.png")
            .expect("binary file present in parsed diff");
        let info = f.binary.as_ref().expect("binary info present");
        assert_eq!(
            info.kind,
            BinaryKind::Image {
                mime: "image/png".to_string()
            }
        );
        assert!(f.hunks.is_empty(), "binary file has no textual hunks");
    }

    #[tokio::test]
    async fn read_base_blob_returns_committed_bytes_not_worktree() {
        let tmp = init_repo().await;
        let p = tmp.path();
        let v1: &[u8] = b"\x00base-bytes\x00";
        let v2: &[u8] = b"\x00worktree-bytes-changed\x00";
        fs::write(p.join("img.png"), v1).unwrap();
        git(p, &["add", "."]).await;
        git(p, &["commit", "-q", "-m", "A"]).await;
        let base = git_capture(p, &["rev-parse", "HEAD"]).await;
        fs::write(p.join("img.png"), v2).unwrap();

        // Old side reads the committed blob; new side reads the working tree.
        let old = read_base_blob(p, &base, "img.png").await.unwrap();
        assert_eq!(old, v1);
        let new = read_worktree_file(p, "img.png").await.unwrap();
        assert_eq!(new, v2);
    }

    #[tokio::test]
    async fn read_base_blob_errors_for_file_absent_at_base() {
        let tmp = init_repo().await;
        let p = tmp.path();
        fs::write(p.join("seed.txt"), "x\n").unwrap();
        git(p, &["add", "."]).await;
        git(p, &["commit", "-q", "-m", "A"]).await;
        let base = git_capture(p, &["rev-parse", "HEAD"]).await;

        // An added file has no base side -> error rather than bogus bytes.
        fs::write(p.join("added.png"), b"\x00new\x00").unwrap();
        assert!(read_base_blob(p, &base, "added.png").await.is_err());
    }

    #[tokio::test]
    async fn enrich_binary_sizes_fills_old_and_new() {
        let tmp = init_repo().await;
        let p = tmp.path();
        let v1: &[u8] = b"\x00abc\x00"; // 5 bytes
        let v2: &[u8] = b"\x00abcdefgh\x00"; // 10 bytes
        fs::write(p.join("logo.png"), v1).unwrap();
        git(p, &["add", "."]).await;
        git(p, &["commit", "-q", "-m", "A"]).await;
        let base = git_capture(p, &["rev-parse", "HEAD"]).await;
        fs::write(p.join("logo.png"), v2).unwrap();

        let raw = compose_review_diff(p, &base).await.unwrap();
        let mut diff = parse_unified_diff(&raw);
        enrich_binary_sizes(&mut diff, p).await;
        let f = diff
            .files
            .iter()
            .find(|f| f.display_path() == "logo.png")
            .unwrap();
        let b = f.binary.as_ref().unwrap();
        assert_eq!(b.old_size, Some(v1.len() as u64));
        assert_eq!(b.new_size, Some(v2.len() as u64));
    }

    #[tokio::test]
    async fn includes_unstaged_and_untracked_against_base() {
        let tmp = init_repo().await;
        let p = tmp.path();

        fs::write(p.join("file.txt"), "v1\n").unwrap();
        git(p, &["add", "."]).await;
        git(p, &["commit", "-q", "-m", "A"]).await;
        let base = git_capture(p, &["rev-parse", "HEAD"]).await;

        // Unstaged modification + untracked file (neither committed).
        fs::write(p.join("file.txt"), "v3\n").unwrap();
        fs::write(p.join("untracked.txt"), "u\n").unwrap();

        let diff = compose_review_diff(p, &base).await.unwrap();
        assert!(diff.contains("+v3"), "unstaged change missing:\n{diff}");
        assert!(
            diff.contains("untracked.txt") && diff.contains("+u"),
            "untracked file missing:\n{diff}"
        );
    }

    #[tokio::test]
    async fn empty_when_clean_against_head() {
        let tmp = init_repo().await;
        let p = tmp.path();
        fs::write(p.join("file.txt"), "x\n").unwrap();
        git(p, &["add", "."]).await;
        git(p, &["commit", "-q", "-m", "A"]).await;

        let diff = compose_review_diff(p, "HEAD").await.unwrap();
        assert!(diff.is_empty(), "expected empty diff, got:\n{diff}");
        assert!(parse_unified_diff(&diff).is_empty());
    }

    #[tokio::test]
    async fn degrades_to_head_when_base_unresolvable() {
        let tmp = init_repo().await;
        let p = tmp.path();
        fs::write(p.join("file.txt"), "v1\n").unwrap();
        git(p, &["add", "."]).await;
        git(p, &["commit", "-q", "-m", "A"]).await;

        // Unstaged change present; base is a branch that does not exist.
        fs::write(p.join("file.txt"), "v2\n").unwrap();

        let diff = compose_review_diff(p, "no-such-branch").await.unwrap();
        // Falls back to working-tree-vs-HEAD, which still shows the change.
        assert!(
            diff.contains("+v2"),
            "fallback diff missing change:\n{diff}"
        );
    }

    #[tokio::test]
    async fn prefers_origin_branch_when_remote_ref_exists() {
        let tmp = init_repo().await;
        let p = tmp.path();
        fs::write(p.join("file.txt"), "v1\n").unwrap();
        git(p, &["add", "."]).await;
        git(p, &["commit", "-q", "-m", "A"]).await;
        let sha = git_capture(p, &["rev-parse", "HEAD"]).await;

        // Simulate a pushed upstream by creating the remote-tracking ref
        // directly (no real remote needed).
        git(p, &["update-ref", "refs/remotes/origin/main", &sha]).await;

        assert_eq!(prefer_remote_branch(p, "main").await, "origin/main");
    }

    #[tokio::test]
    async fn falls_back_to_local_branch_when_no_remote_ref() {
        let tmp = init_repo().await;
        let p = tmp.path();
        fs::write(p.join("file.txt"), "v1\n").unwrap();
        git(p, &["add", "."]).await;
        git(p, &["commit", "-q", "-m", "A"]).await;

        // No `origin/main` remote-tracking ref → bare (local) branch name.
        assert_eq!(prefer_remote_branch(p, "main").await, "main");
    }

    /// Minimal one-file diff with the given path, hunk ranges, and body lines.
    fn one_file_diff(path: &str, ranges: &str, body: &str) -> String {
        format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ {ranges} @@\n{body}")
    }

    #[test]
    fn file_diff_hash_is_deterministic() {
        let raw = one_file_diff("src/a.rs", "-1,2 +1,2", " ctx\n-old\n+new\n");
        let a = parse_unified_diff(&raw);
        let b = parse_unified_diff(&raw);
        assert_eq!(file_diff_hash(&a.files[0]), file_diff_hash(&b.files[0]));
    }

    #[test]
    fn file_diff_hash_changes_on_line_content_change() {
        let a = parse_unified_diff(&one_file_diff(
            "src/a.rs",
            "-1,2 +1,2",
            " ctx\n-old\n+new\n",
        ));
        let b = parse_unified_diff(&one_file_diff(
            "src/a.rs",
            "-1,2 +1,2",
            " ctx\n-old\n+newer\n",
        ));
        assert_ne!(file_diff_hash(&a.files[0]), file_diff_hash(&b.files[0]));
    }

    #[test]
    fn file_diff_hash_changes_on_added_line() {
        let a = parse_unified_diff(&one_file_diff(
            "src/a.rs",
            "-1,2 +1,2",
            " ctx\n-old\n+new\n",
        ));
        let b = parse_unified_diff(&one_file_diff(
            "src/a.rs",
            "-1,2 +1,3",
            " ctx\n-old\n+new\n+extra\n",
        ));
        assert_ne!(file_diff_hash(&a.files[0]), file_diff_hash(&b.files[0]));
    }

    #[test]
    fn file_diff_hash_changes_when_hunk_positions_shift() {
        // Identical lines, but the hunk has moved within the file.
        let a = parse_unified_diff(&one_file_diff(
            "src/a.rs",
            "-1,2 +1,2",
            " ctx\n-old\n+new\n",
        ));
        let b = parse_unified_diff(&one_file_diff(
            "src/a.rs",
            "-10,2 +10,2",
            " ctx\n-old\n+new\n",
        ));
        assert_ne!(file_diff_hash(&a.files[0]), file_diff_hash(&b.files[0]));
    }

    #[test]
    fn file_diff_hash_ignores_path() {
        // Identity is the display path (the map key), not the hash, so two
        // files with identical hunks hash the same.
        let a = parse_unified_diff(&one_file_diff(
            "src/a.rs",
            "-1,2 +1,2",
            " ctx\n-old\n+new\n",
        ));
        let b = parse_unified_diff(&one_file_diff(
            "src/b.rs",
            "-1,2 +1,2",
            " ctx\n-old\n+new\n",
        ));
        assert_eq!(file_diff_hash(&a.files[0]), file_diff_hash(&b.files[0]));
    }
}
