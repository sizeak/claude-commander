//! Review-diff + comment handlers.
//!
//! Thin wrappers over `CommanderService`: `open_review`,
//! `refresh_review_if_changed`, comment CRUD
//! (`list_comments`/`create_comment`/`delete_comment`), `apply_comments`, and
//! `toggle_file_reviewed`.
//!
//! `toggle_file_reviewed` takes a core [`FileDiff`], which is `Serialize` but
//! **not** `Deserialize` — the client echoes back the file it saw in the
//! review snapshot. We mirror that shape in a `Deserialize` DTO ([`FileDiffDto`]
//! and friends) and convert into the core type, rather than adding a derive to
//! core (the dependency direction is server → core, never the reverse).

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use claude_commander_core::api::{NewComment, ReviewSnapshot};
use claude_commander_core::comment::{Comment, CommentSide};
use claude_commander_core::git::{
    BinaryInfo, BinaryKind, DiffLine, FileDiff, FileStatus, Hunk, LineOrigin,
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

use super::parse_session_id;

/// `GET /sessions/{id}/review` → `open_review`.
pub async fn open(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ReviewSnapshot>, ApiError> {
    let id = parse_session_id(&id)?;
    Ok(Json(state.service.open_review(&id).await?))
}

#[derive(Debug, Deserialize)]
pub struct RefreshQuery {
    #[serde(default)]
    pub prev_hash: u64,
}

/// `GET /sessions/{id}/review/refresh?prev_hash=` → `refresh_review_if_changed`
/// (204 when unchanged, else the fresh snapshot).
pub async fn refresh(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RefreshQuery>,
) -> Result<Response, ApiError> {
    let id = parse_session_id(&id)?;
    match state
        .service
        .refresh_review_if_changed(&id, q.prev_hash)
        .await?
    {
        Some(snapshot) => Ok(Json(snapshot).into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

/// `GET /sessions/{id}/comments` → `list_comments`.
pub async fn list_comments(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<Comment>>, ApiError> {
    let id = parse_session_id(&id)?;
    Ok(Json(state.service.list_comments(&id).await?))
}

/// Request body for `POST /sessions/{id}/comments`. Mirrors [`NewComment`],
/// which is not itself `Deserialize`.
#[derive(Debug, Deserialize)]
pub struct NewCommentBody {
    pub file: String,
    pub side: CommentSide,
    pub line_range: (usize, usize),
    pub snippet: String,
    pub comment: String,
}

impl From<NewCommentBody> for NewComment {
    fn from(b: NewCommentBody) -> Self {
        NewComment {
            file: b.file,
            side: b.side,
            line_range: b.line_range,
            snippet: b.snippet,
            comment: b.comment,
        }
    }
}

/// `POST /sessions/{id}/comments` → `create_comment` → 201 `{ "id": ... }`.
pub async fn create_comment(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<NewCommentBody>,
) -> Result<Response, ApiError> {
    let id = parse_session_id(&id)?;
    let cid = state.service.create_comment(&id, body.into()).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": cid }))).into_response())
}

/// `DELETE /sessions/{id}/comments/{cid}` → `delete_comment` → 204.
pub async fn delete_comment(
    State(state): State<AppState>,
    Path((id, cid)): Path<(String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let id = parse_session_id(&id)?;
    state.service.delete_comment(&id, cid).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /sessions/{id}/comments/apply` → `apply_comments` → `ApplyOutcome`.
pub async fn apply(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = parse_session_id(&id)?;
    let outcome = state.service.apply_comments(&id).await?;
    Ok(Json(outcome).into_response())
}

/// `POST /sessions/{id}/files/reviewed` → `toggle_file_reviewed` →
/// `{ "reviewed": bool }`. The body is the [`FileDiff`] the client is
/// displaying (echoed from the review snapshot).
pub async fn toggle_reviewed(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(file): Json<FileDiffDto>,
) -> Result<Response, ApiError> {
    let id = parse_session_id(&id)?;
    let file: FileDiff = file.into();
    let reviewed = state.service.toggle_file_reviewed(&id, &file).await?;
    Ok(Json(json!({ "reviewed": reviewed })).into_response())
}

// -- `Deserialize` mirrors of the `Serialize`-only core diff types --
//
// These reproduce the exact serialized shape of the core `review_diff` types so
// a client can echo a `FileDiff` back to `POST .../files/reviewed`. Field names
// and enum tags match core's `#[serde]` attributes; `From` conversions rebuild
// the core types (whose fields are all `pub`).

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineOriginDto {
    Context,
    Addition,
    Deletion,
}

impl From<LineOriginDto> for LineOrigin {
    fn from(d: LineOriginDto) -> Self {
        match d {
            LineOriginDto::Context => LineOrigin::Context,
            LineOriginDto::Addition => LineOrigin::Addition,
            LineOriginDto::Deletion => LineOrigin::Deletion,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct DiffLineDto {
    pub origin: LineOriginDto,
    pub old_lineno: Option<usize>,
    pub new_lineno: Option<usize>,
    pub content: String,
}

impl From<DiffLineDto> for DiffLine {
    fn from(d: DiffLineDto) -> Self {
        DiffLine {
            origin: d.origin.into(),
            old_lineno: d.old_lineno,
            new_lineno: d.new_lineno,
            content: d.content,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct HunkDto {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub header: String,
    pub lines: Vec<DiffLineDto>,
}

impl From<HunkDto> for Hunk {
    fn from(d: HunkDto) -> Self {
        Hunk {
            old_start: d.old_start,
            old_lines: d.old_lines,
            new_start: d.new_start,
            new_lines: d.new_lines,
            header: d.header,
            lines: d.lines.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatusDto {
    Added,
    Deleted,
    Modified,
    Renamed,
}

impl From<FileStatusDto> for FileStatus {
    fn from(d: FileStatusDto) -> Self {
        match d {
            FileStatusDto::Added => FileStatus::Added,
            FileStatusDto::Deleted => FileStatus::Deleted,
            FileStatusDto::Modified => FileStatus::Modified,
            FileStatusDto::Renamed => FileStatus::Renamed,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BinaryKindDto {
    Image { mime: String },
    Other,
}

impl From<BinaryKindDto> for BinaryKind {
    fn from(d: BinaryKindDto) -> Self {
        match d {
            BinaryKindDto::Image { mime } => BinaryKind::Image { mime },
            BinaryKindDto::Other => BinaryKind::Other,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct BinaryInfoDto {
    pub kind: BinaryKindDto,
    pub old_oid: Option<String>,
    pub new_oid: Option<String>,
    pub old_size: Option<u64>,
    pub new_size: Option<u64>,
}

impl From<BinaryInfoDto> for BinaryInfo {
    fn from(d: BinaryInfoDto) -> Self {
        BinaryInfo {
            kind: d.kind.into(),
            old_oid: d.old_oid,
            new_oid: d.new_oid,
            old_size: d.old_size,
            new_size: d.new_size,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct FileDiffDto {
    pub old_path: String,
    pub new_path: String,
    pub status: FileStatusDto,
    pub added: usize,
    pub removed: usize,
    pub hunks: Vec<HunkDto>,
    pub binary: Option<BinaryInfoDto>,
}

impl From<FileDiffDto> for FileDiff {
    fn from(d: FileDiffDto) -> Self {
        FileDiff {
            old_path: d.old_path,
            new_path: d.new_path,
            status: d.status.into(),
            added: d.added,
            removed: d.removed,
            hunks: d.hunks.into_iter().map(Into::into).collect(),
            binary: d.binary.map(Into::into),
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::{Router, routing::get};
    use claude_commander_core::comment::Comment;
    use claude_commander_core::git::FileDiff;
    use tempfile::TempDir;

    use crate::handlers::test_support::{get as do_get, json, test_state};

    /// `list_comments` on an unseen session returns an empty list (the store
    /// treats an absent file as empty), so the route is 200 with `[]`.
    #[tokio::test]
    async fn list_comments_empty_is_200_empty_array() {
        let dir = TempDir::new().unwrap();
        let id = uuid::Uuid::new_v4();
        let router = Router::new()
            .route("/sessions/{id}/comments", get(super::list_comments))
            .with_state(test_state(&dir));
        let (status, body) = do_get(router, &format!("/sessions/{id}/comments")).await;
        assert_eq!(status, 200);
        let comments: Vec<Comment> = json(&body);
        assert!(comments.is_empty());
    }

    /// A malformed session id on an id-route maps to 400, not 404.
    #[tokio::test]
    async fn comments_bad_id_is_400() {
        let dir = TempDir::new().unwrap();
        let router = Router::new()
            .route("/sessions/{id}/comments", get(super::list_comments))
            .with_state(test_state(&dir));
        let (status, _) = do_get(router, "/sessions/not-a-uuid/comments").await;
        assert_eq!(status, 400);
    }

    /// The `FileDiffDto` round-trips a serialized core `FileDiff` faithfully,
    /// so a client can echo back what `open_review` sent it.
    #[test]
    fn file_diff_dto_roundtrips_serialized_filediff() {
        use claude_commander_core::git::{DiffLine, FileStatus, Hunk, LineOrigin};
        let original = FileDiff {
            old_path: "a.rs".into(),
            new_path: "a.rs".into(),
            status: FileStatus::Modified,
            added: 1,
            removed: 0,
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: 0,
                new_start: 1,
                new_lines: 1,
                header: "fn x".into(),
                lines: vec![DiffLine {
                    origin: LineOrigin::Addition,
                    old_lineno: None,
                    new_lineno: Some(1),
                    content: "let y = 2;".into(),
                }],
            }],
            binary: None,
        };
        let wire = serde_json::to_vec(&original).unwrap();
        let dto: super::FileDiffDto = serde_json::from_slice(&wire).unwrap();
        let rebuilt: FileDiff = dto.into();
        assert_eq!(rebuilt, original);
    }

    /// A binary image file (the nested, doubly-`kind`-tagged `BinaryInfo` ->
    /// `BinaryKind::Image` path) must round-trip through the DTO. This exercises
    /// the `#[serde(tag = "kind")]` enum that the plain text case skips.
    #[test]
    fn file_diff_dto_roundtrips_binary_image() {
        use claude_commander_core::git::{BinaryInfo, BinaryKind, FileStatus};
        let original = FileDiff {
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
                old_oid: Some("aaaa".into()),
                new_oid: Some("bbbb".into()),
                old_size: Some(128),
                new_size: Some(256),
            }),
        };
        let wire = serde_json::to_vec(&original).unwrap();
        let dto: super::FileDiffDto = serde_json::from_slice(&wire).unwrap();
        let rebuilt: FileDiff = dto.into();
        assert_eq!(rebuilt, original);
    }

    /// A deleted file with `Deletion` lines, and a renamed file, must both
    /// round-trip — covering the `FileStatus::Deleted`/`Renamed` and
    /// `LineOrigin::Deletion` variants the modified+addition case skips.
    #[test]
    fn file_diff_dto_roundtrips_deleted_and_renamed() {
        use claude_commander_core::git::{DiffLine, FileStatus, Hunk, LineOrigin};

        let deleted = FileDiff {
            old_path: "gone.rs".into(),
            new_path: "gone.rs".into(),
            status: FileStatus::Deleted,
            added: 0,
            removed: 1,
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: 1,
                new_start: 0,
                new_lines: 0,
                header: "".into(),
                lines: vec![DiffLine {
                    origin: LineOrigin::Deletion,
                    old_lineno: Some(1),
                    new_lineno: None,
                    content: "let removed = true;".into(),
                }],
            }],
            binary: None,
        };

        let renamed = FileDiff {
            old_path: "old_name.rs".into(),
            new_path: "new_name.rs".into(),
            status: FileStatus::Renamed,
            added: 0,
            removed: 0,
            hunks: vec![],
            binary: None,
        };

        for original in [deleted, renamed] {
            let wire = serde_json::to_vec(&original).unwrap();
            let dto: super::FileDiffDto = serde_json::from_slice(&wire).unwrap();
            let rebuilt: FileDiff = dto.into();
            assert_eq!(rebuilt, original);
        }
    }
}
