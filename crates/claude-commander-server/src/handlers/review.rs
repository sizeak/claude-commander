//! Review-diff + comment handlers.
//!
//! Thin wrappers over `CommanderService`: `open_review`,
//! `refresh_review_if_changed`, comment CRUD
//! (`list_comments`/`create_comment`/`delete_comment`), `apply_comments`, and
//! `toggle_file_reviewed`.
//!
//! `toggle_file_reviewed` takes a [`FileDiff`] — the client echoes back the
//! file it saw in the review snapshot. `FileDiff` lives in the shared
//! `claude-commander-protocol` crate and derives `Deserialize`, so the body
//! deserializes directly with no hand-written mirror DTO.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use claude_commander_core::api::{NewComment, ReviewSnapshot};
use claude_commander_core::comment::Comment;
use claude_commander_core::git::FileDiff;
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

/// `POST /sessions/{id}/comments` → `create_comment` → 201 `{ "id": ... }`.
///
/// The body deserializes straight into [`NewComment`] (a shared wire type in
/// `claude-commander-protocol`) — no server-side mirror DTO.
pub async fn create_comment(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<NewComment>,
) -> Result<Response, ApiError> {
    let id = parse_session_id(&id)?;
    let cid = state.service.create_comment(&id, body).await?;
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
    Json(file): Json<FileDiff>,
) -> Result<Response, ApiError> {
    let id = parse_session_id(&id)?;
    let reviewed = state.service.toggle_file_reviewed(&id, &file).await?;
    Ok(Json(json!({ "reviewed": reviewed })).into_response())
}

#[cfg(test)]
mod tests {
    use axum::{Router, routing::get};
    use claude_commander_core::comment::Comment;
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

    // The `FileDiff` wire round-trip (the contract a client relies on to echo a
    // file back to `POST .../files/reviewed`) is covered in the
    // `claude-commander-protocol` crate, which now owns the type and its
    // `Serialize + Deserialize` derives — no server-side mirror to test.
}
