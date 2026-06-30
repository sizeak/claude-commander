//! Review/diff + comments HTTP API exposed to Flutter via flutter_rust_bridge.
//!
//! Follows the same style as [`crate::api::simple`] (blocking reqwest, bearer
//! auth, `base`/`client`/`ok_or_status` helpers): each `pub fn` becomes an
//! async-callable Dart function, deserializing server JSON into the shared
//! `claude-commander-protocol` types so any drift fails here in Rust.
//!
//! ## Why DTOs instead of returning the protocol types
//!
//! frb renders Rust **data-carrying enums** as Dart `freezed` classes, which
//! needs the build_runner toolchain — we deliberately don't add it (same rule
//! as [`crate::api::terminal::TerminalEvent`]). The protocol review types carry
//! data enums (`ApplyOutcome`, `BinaryKind`) and tuples (`Comment.line_range`),
//! so we deserialize the server payload into the protocol types and then
//! convert into the plain-struct / unit-enum DTOs defined below before handing
//! them to frb.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use claude_commander_protocol::api::{NewComment, ReviewSnapshot};
use claude_commander_protocol::comment::{ApplyOutcome, Comment, CommentSide, CommentStatus};
use claude_commander_protocol::diff::{
    BinaryKind, DiffLine, FileDiff, FileStatus, Hunk, LineOrigin, ParsedDiff,
};
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};

/// Trim a trailing slash so `{base}/path` joins cleanly.
fn base(base_url: &str) -> &str {
    base_url.trim_end_matches('/')
}

fn client() -> Client {
    Client::new()
}

/// Map a response to a `Result`: a 401 becomes a friendly auth error, any other
/// non-2xx surfaces via `error_for_status`, and a 2xx passes through. `what`
/// labels the failing call in the error message.
fn ok_or_status(resp: Response, what: &str) -> Result<Response> {
    if resp.status() == StatusCode::UNAUTHORIZED {
        anyhow::bail!("authentication failed (check your token)");
    }
    resp.error_for_status()
        .with_context(|| format!("{what}: server returned an error status"))
}

// ---------------------------------------------------------------------------
// DTOs — plain structs + unit enums only (no freezed).
// ---------------------------------------------------------------------------

/// Origin of a single diff line (unit-enum mirror of [`LineOrigin`]).
pub enum ReviewLineOrigin {
    Context,
    Addition,
    Deletion,
}

impl From<LineOrigin> for ReviewLineOrigin {
    fn from(o: LineOrigin) -> Self {
        match o {
            LineOrigin::Context => Self::Context,
            LineOrigin::Addition => Self::Addition,
            LineOrigin::Deletion => Self::Deletion,
        }
    }
}

/// How a file changed (unit-enum mirror of [`FileStatus`]).
pub enum ReviewFileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
}

impl From<FileStatus> for ReviewFileStatus {
    fn from(s: FileStatus) -> Self {
        match s {
            FileStatus::Added => Self::Added,
            FileStatus::Deleted => Self::Deleted,
            FileStatus::Modified => Self::Modified,
            FileStatus::Renamed => Self::Renamed,
        }
    }
}

/// Which side of the diff a comment range refers to (unit-enum mirror of
/// [`CommentSide`]).
pub enum ReviewCommentSide {
    Old,
    New,
}

impl From<CommentSide> for ReviewCommentSide {
    fn from(s: CommentSide) -> Self {
        match s {
            CommentSide::Old => Self::Old,
            CommentSide::New => Self::New,
        }
    }
}

/// Lifecycle status of a comment (unit-enum mirror of [`CommentStatus`]).
pub enum ReviewCommentStatus {
    Staged,
    Drifted,
    Applied,
}

impl From<CommentStatus> for ReviewCommentStatus {
    fn from(s: CommentStatus) -> Self {
        match s {
            CommentStatus::Staged => Self::Staged,
            CommentStatus::Drifted => Self::Drifted,
            CommentStatus::Applied => Self::Applied,
        }
    }
}

/// A single diff line with resolved old/new line numbers.
pub struct ReviewLineDto {
    pub origin: ReviewLineOrigin,
    /// `None` for additions.
    pub old_lineno: Option<u32>,
    /// `None` for deletions.
    pub new_lineno: Option<u32>,
    /// Line content without the leading `+`/`-`/space marker.
    pub content: String,
}

impl From<DiffLine> for ReviewLineDto {
    fn from(l: DiffLine) -> Self {
        Self {
            origin: l.origin.into(),
            old_lineno: l.old_lineno.map(|n| n as u32),
            new_lineno: l.new_lineno.map(|n| n as u32),
            content: l.content,
        }
    }
}

/// A contiguous block of changes (one `@@ ... @@` section).
pub struct ReviewHunkDto {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// Section heading following the closing `@@`, if any.
    pub header: String,
    pub lines: Vec<ReviewLineDto>,
}

impl From<Hunk> for ReviewHunkDto {
    fn from(h: Hunk) -> Self {
        Self {
            old_start: h.old_start as u32,
            old_lines: h.old_lines as u32,
            new_start: h.new_start as u32,
            new_lines: h.new_lines as u32,
            header: h.header,
            lines: h.lines.into_iter().map(Into::into).collect(),
        }
    }
}

/// All changes to a single file. `BinaryKind`/`BinaryInfo` are flattened onto
/// `is_binary` + `binary_mime` (the blob bytes are fetched lazily — TODO, not
/// wired in this first cut).
pub struct ReviewFileDto {
    /// Path to show in the file list (new path, or old path for deletions).
    pub display_path: String,
    pub old_path: String,
    pub new_path: String,
    pub status: ReviewFileStatus,
    pub added: u32,
    pub removed: u32,
    pub hunks: Vec<ReviewHunkDto>,
    pub is_binary: bool,
    /// `Some(mime)` when the binary is a renderable image; `None` otherwise.
    pub binary_mime: Option<String>,
}

impl From<FileDiff> for ReviewFileDto {
    fn from(f: FileDiff) -> Self {
        let display_path = f.display_path().to_string();
        let (is_binary, binary_mime) = match &f.binary {
            Some(info) => match &info.kind {
                BinaryKind::Image { mime } => (true, Some(mime.clone())),
                BinaryKind::Other => (true, None),
            },
            None => (false, None),
        };
        Self {
            display_path,
            old_path: f.old_path,
            new_path: f.new_path,
            status: f.status.into(),
            added: f.added as u32,
            removed: f.removed as u32,
            hunks: f.hunks.into_iter().map(Into::into).collect(),
            is_binary,
            binary_mime,
        }
    }
}

/// A parsed unified diff: an ordered list of changed files.
pub struct ReviewDiffDto {
    pub files: Vec<ReviewFileDto>,
}

impl From<ParsedDiff> for ReviewDiffDto {
    fn from(d: ParsedDiff) -> Self {
        Self {
            files: d.files.into_iter().map(Into::into).collect(),
        }
    }
}

/// A single review comment. `line_range: (usize, usize)` is split into
/// `line_start`/`line_end`; the id is exposed as a `String`.
pub struct CommentDto {
    pub id: String,
    pub file: String,
    pub side: ReviewCommentSide,
    pub line_start: u32,
    pub line_end: u32,
    pub snippet: String,
    pub comment: String,
    pub status: ReviewCommentStatus,
    pub created_at: DateTime<Utc>,
}

impl From<Comment> for CommentDto {
    fn from(c: Comment) -> Self {
        Self {
            id: c.id.to_string(),
            file: c.file,
            side: c.side.into(),
            line_start: c.line_range.0 as u32,
            line_end: c.line_range.1 as u32,
            snippet: c.snippet,
            comment: c.comment,
            status: c.status.into(),
            created_at: c.created_at,
        }
    }
}

/// Result of opening the review view. `content_hash` is a `u64` server-side;
/// it's exposed as a `String` because a Dart `int` can't hold all `u64` values
/// (and the value is only ever echoed back to `refresh_review`).
pub struct ReviewSnapshotDto {
    pub base: String,
    pub content_hash: String,
    pub files: Vec<ReviewFileDto>,
    pub comments: Vec<CommentDto>,
    /// Display paths of files still marked reviewed.
    pub reviewed: Vec<String>,
}

impl From<ReviewSnapshot> for ReviewSnapshotDto {
    fn from(s: ReviewSnapshot) -> Self {
        Self {
            base: s.base,
            content_hash: s.content_hash.to_string(),
            files: s.diff.files.into_iter().map(Into::into).collect(),
            comments: s.comments.into_iter().map(Into::into).collect(),
            reviewed: s.reviewed,
        }
    }
}

/// Which kind of [`ApplyResult`] this is (unit enum, so frb renders a plain Dart
/// enum rather than a freezed class). Flattens the data-carrying
/// [`ApplyOutcome`].
pub enum ApplyResultKind {
    /// No staged comments to apply.
    Nothing,
    /// One or more comments are drifted; nothing was sent.
    Blocked,
    /// Comments were composed and the prompt injected.
    Applied,
    /// The brief was written but couldn't be delivered; the user can re-apply.
    Deferred,
}

/// Outcome of applying a session's staged comments — a tagged struct (not a
/// data enum), following the [`crate::api::terminal::TerminalEvent`] rule.
pub struct ApplyResult {
    pub kind: ApplyResultKind,
    /// Ids of drifted comments (populated only for [`ApplyResultKind::Blocked`]).
    pub drifted_ids: Vec<String>,
    /// Path the brief was composed to (`Applied`/`Deferred`); `None` otherwise.
    pub path: Option<String>,
    /// Number of comments composed (`Applied`/`Deferred`); `0` otherwise.
    pub count: u32,
}

impl From<ApplyOutcome> for ApplyResult {
    fn from(o: ApplyOutcome) -> Self {
        match o {
            ApplyOutcome::Nothing => Self {
                kind: ApplyResultKind::Nothing,
                drifted_ids: Vec::new(),
                path: None,
                count: 0,
            },
            ApplyOutcome::Blocked { drifted } => Self {
                kind: ApplyResultKind::Blocked,
                drifted_ids: drifted.into_iter().map(|id| id.to_string()).collect(),
                path: None,
                count: 0,
            },
            ApplyOutcome::Applied { path, count } => Self {
                kind: ApplyResultKind::Applied,
                drifted_ids: Vec::new(),
                path: Some(path.to_string_lossy().into_owned()),
                count: count as u32,
            },
            ApplyOutcome::Deferred { path, count } => Self {
                kind: ApplyResultKind::Deferred,
                drifted_ids: Vec::new(),
                path: Some(path.to_string_lossy().into_owned()),
                count: count as u32,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// cdylib functions.
// ---------------------------------------------------------------------------

/// `GET {base_url}/api/sessions/{session_id}/review` → the review snapshot
/// (parsed diff + re-anchored comments + reviewed marks), converted to DTOs.
pub fn open_review(
    base_url: String,
    token: String,
    session_id: String,
) -> Result<ReviewSnapshotDto> {
    let resp = client()
        .get(format!(
            "{}/api/sessions/{}/review",
            base(&base_url),
            session_id
        ))
        .bearer_auth(token)
        .send()
        .context("open_review request failed")?;
    let snapshot = ok_or_status(resp, "open_review")?
        .json::<ReviewSnapshot>()
        .context("response did not match the ReviewSnapshot contract")?;
    Ok(snapshot.into())
}

/// `GET {base_url}/api/sessions/{session_id}/review/refresh?prev_hash=` → a
/// fresh snapshot, or `None` (204) when the diff is unchanged. `prev_hash` is
/// the `content_hash` string from a prior snapshot, parsed back to a `u64`.
pub fn refresh_review(
    base_url: String,
    token: String,
    session_id: String,
    prev_hash: String,
) -> Result<Option<ReviewSnapshotDto>> {
    let prev_hash: u64 = prev_hash
        .parse()
        .context("prev_hash was not a valid content hash")?;
    let resp = client()
        .get(format!(
            "{}/api/sessions/{}/review/refresh?prev_hash={}",
            base(&base_url),
            session_id,
            prev_hash
        ))
        .bearer_auth(token)
        .send()
        .context("refresh_review request failed")?;
    if resp.status() == StatusCode::NO_CONTENT {
        return Ok(None);
    }
    let snapshot = ok_or_status(resp, "refresh_review")?
        .json::<ReviewSnapshot>()
        .context("response did not match the ReviewSnapshot contract")?;
    Ok(Some(snapshot.into()))
}

/// `GET {base_url}/api/sessions/{session_id}/comments` → the session's comments.
pub fn list_comments(
    base_url: String,
    token: String,
    session_id: String,
) -> Result<Vec<CommentDto>> {
    let resp = client()
        .get(format!(
            "{}/api/sessions/{}/comments",
            base(&base_url),
            session_id
        ))
        .bearer_auth(token)
        .send()
        .context("list_comments request failed")?;
    let comments = ok_or_status(resp, "list_comments")?
        .json::<Vec<Comment>>()
        .context("response did not match the Comment contract")?;
    Ok(comments.into_iter().map(Into::into).collect())
}

/// `POST {base_url}/api/sessions/{session_id}/comments` (body = `NewComment`)
/// → 201 `{ "id": ... }`, returning the new comment id. `side` is `"old"` or
/// `"new"`.
#[allow(clippy::too_many_arguments)]
pub fn create_comment(
    base_url: String,
    token: String,
    session_id: String,
    file: String,
    side: String,
    line_start: u32,
    line_end: u32,
    snippet: String,
    comment: String,
) -> Result<String> {
    let side: CommentSide = match side.as_str() {
        "old" => CommentSide::Old,
        "new" => CommentSide::New,
        other => anyhow::bail!("invalid comment side {other:?} (expected \"old\" or \"new\")"),
    };
    let body = NewComment {
        file,
        side,
        line_range: (line_start as usize, line_end as usize),
        snippet,
        comment,
    };
    let resp = client()
        .post(format!(
            "{}/api/sessions/{}/comments",
            base(&base_url),
            session_id
        ))
        .bearer_auth(token)
        .json(&body)
        .send()
        .context("create_comment request failed")?;
    let value: serde_json::Value = ok_or_status(resp, "create_comment")?
        .json()
        .context("could not read create_comment response body")?;
    value
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .context("create_comment response was missing the new comment id")
}

/// `DELETE {base_url}/api/sessions/{session_id}/comments/{comment_id}` (204).
pub fn delete_comment(
    base_url: String,
    token: String,
    session_id: String,
    comment_id: String,
) -> Result<()> {
    let resp = client()
        .delete(format!(
            "{}/api/sessions/{}/comments/{}",
            base(&base_url),
            session_id,
            comment_id
        ))
        .bearer_auth(token)
        .send()
        .context("delete_comment request failed")?;
    ok_or_status(resp, "delete_comment")?;
    Ok(())
}

/// `POST {base_url}/api/sessions/{session_id}/comments/apply` → `ApplyOutcome`,
/// converted to the flattened [`ApplyResult`] DTO.
pub fn apply_comments(base_url: String, token: String, session_id: String) -> Result<ApplyResult> {
    let resp = client()
        .post(format!(
            "{}/api/sessions/{}/comments/apply",
            base(&base_url),
            session_id
        ))
        .bearer_auth(token)
        .send()
        .context("apply_comments request failed")?;
    let outcome = ok_or_status(resp, "apply_comments")?
        .json::<ApplyOutcome>()
        .context("response did not match the ApplyOutcome contract")?;
    Ok(outcome.into())
}

// TODO: the binary-blob endpoint (`GET /sessions/{id}/blob`) and
// `toggle_file_reviewed` (`POST /sessions/{id}/files/reviewed`) are not wired in
// this first cut. Binary files render as a placeholder; reviewed marks are
// read-only (shown but not togglable from the client yet).

#[cfg(test)]
mod tests {
    use super::*;
    use claude_commander_protocol::comment::Comment;
    use std::path::PathBuf;

    #[test]
    fn base_trims_trailing_slash() {
        assert_eq!(base("http://host:1234/"), "http://host:1234");
        assert_eq!(base("http://host:1234"), "http://host:1234");
    }

    #[test]
    fn comment_dto_splits_line_range_and_stringifies_id() {
        let c = Comment::new("a.rs", CommentSide::New, (3, 7), "snip", "note");
        let id = c.id.to_string();
        let dto: CommentDto = c.into();
        assert_eq!(dto.line_start, 3);
        assert_eq!(dto.line_end, 7);
        assert_eq!(dto.id, id);
        assert!(matches!(dto.side, ReviewCommentSide::New));
        assert!(matches!(dto.status, ReviewCommentStatus::Staged));
    }

    #[test]
    fn apply_result_flattens_outcome_variants() {
        let nothing: ApplyResult = ApplyOutcome::Nothing.into();
        assert!(matches!(nothing.kind, ApplyResultKind::Nothing));

        let id = uuid::Uuid::new_v4();
        let blocked: ApplyResult = ApplyOutcome::Blocked { drifted: vec![id] }.into();
        assert!(matches!(blocked.kind, ApplyResultKind::Blocked));
        assert_eq!(blocked.drifted_ids, vec![id.to_string()]);

        let applied: ApplyResult = ApplyOutcome::Applied {
            path: PathBuf::from("/tmp/brief.md"),
            count: 4,
        }
        .into();
        assert!(matches!(applied.kind, ApplyResultKind::Applied));
        assert_eq!(applied.count, 4);
        assert_eq!(applied.path.as_deref(), Some("/tmp/brief.md"));
    }

    #[test]
    fn snapshot_dto_exposes_content_hash_as_string() {
        let snap = ReviewSnapshot {
            base: "main".into(),
            diff: ParsedDiff::default(),
            comments: vec![],
            reviewed: vec![],
            content_hash: u64::MAX,
        };
        let dto: ReviewSnapshotDto = snap.into();
        assert_eq!(dto.content_hash, u64::MAX.to_string());
    }

    #[test]
    fn binary_image_file_flattens_to_mime() {
        use claude_commander_protocol::diff::{BinaryInfo, BinaryKind};
        let f = FileDiff {
            old_path: "logo.png".into(),
            new_path: "logo.png".into(),
            status: FileStatus::Modified,
            added: 0,
            removed: 0,
            hunks: vec![],
            binary: Some(BinaryInfo {
                kind: BinaryKind::Image {
                    mime: "image/png".into(),
                },
                old_oid: None,
                new_oid: None,
                old_size: None,
                new_size: None,
            }),
        };
        let dto: ReviewFileDto = f.into();
        assert!(dto.is_binary);
        assert_eq!(dto.binary_mime.as_deref(), Some("image/png"));
    }
}
