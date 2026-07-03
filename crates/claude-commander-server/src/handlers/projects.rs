//! Project handlers.
//!
//! Thin wrappers over `CommanderService`: `add_project`, `scan_directory`,
//! `ensure_project`, `list_projects`, `remove_project`, `list_branches`, and
//! project `preview`.

use std::path::PathBuf;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use claude_commander_core::api::{BranchInfo, PreviewData, PreviewTarget, ProjectInfo};
use claude_commander_core::session::ProjectId;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::error::ApiError;
use crate::handlers::run_local;
use crate::state::AppState;

/// Parse a `{id}` path param into a [`ProjectId`], mapping a malformed UUID to a
/// 400 rather than a 404 (the client sent a syntactically bad id).
fn parse_project_id(raw: &str) -> Result<ProjectId, ApiError> {
    Uuid::parse_str(raw).map(ProjectId::from_uuid).map_err(|e| {
        ApiError(
            claude_commander_core::error::SessionError::InvalidName {
                name: raw.to_string(),
                reason: format!("not a valid project id: {e}"),
            }
            .into(),
        )
    })
}

#[derive(Debug, Deserialize)]
pub struct ProjectPathBody {
    pub path: PathBuf,
}

/// `POST /projects` → `add_project` → 201 `{ "id": ... }`.
pub async fn add(
    State(state): State<AppState>,
    Json(body): Json<ProjectPathBody>,
) -> Result<Response, ApiError> {
    // `add_project` builds a `gix::Repository` (non-`Send`) across an await.
    let id = run_local(move || async move { state.service.add_project(body.path).await }).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

/// `POST /projects/ensure` → `ensure_project` → 201 `{ "id": ... }`.
pub async fn ensure(
    State(state): State<AppState>,
    Json(body): Json<ProjectPathBody>,
) -> Result<Response, ApiError> {
    let id =
        run_local(move || async move { state.service.ensure_project(body.path).await }).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

#[derive(Debug, Deserialize)]
pub struct ScanQuery {
    pub dir: PathBuf,
}

/// Response for `GET /projects/scan`. Mirrors core's `ScanResult`, which is not
/// `Serialize`. (`Deserialize` is for the handler's own round-trip test.)
#[derive(Debug, Serialize, Deserialize)]
pub struct ScanResponse {
    pub added: usize,
    pub skipped: usize,
}

/// `GET /projects/scan?dir=` → `scan_directory`.
pub async fn scan(
    State(state): State<AppState>,
    Query(q): Query<ScanQuery>,
) -> Result<Json<ScanResponse>, ApiError> {
    // `scan_directory` adds discovered repos via `add_project` (non-`Send` gix).
    let result =
        run_local(move || async move { state.service.scan_directory(&q.dir).await }).await?;
    Ok(Json(ScanResponse {
        added: result.added,
        skipped: result.skipped,
    }))
}

/// `GET /projects` → `list_projects`.
pub async fn list(State(state): State<AppState>) -> Json<Vec<ProjectInfo>> {
    Json(state.service.list_projects().await)
}

/// `DELETE /projects/{id}` → `remove_project` → 204.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_project_id(&id)?;
    // `remove_project` now tears down each session's worktree (opens a `gix`
    // repo → non-`Send` across an await), so drive it through `run_local`.
    run_local(move || async move { state.service.remove_project(&id).await }).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct BranchesQuery {
    /// Run `git fetch origin` first so newly-pushed remote branches appear.
    #[serde(default)]
    pub fetch: bool,
}

/// `GET /projects/{id}/branches?fetch=` → `list_branches`.
pub async fn branches(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<BranchesQuery>,
) -> Result<Json<Vec<BranchInfo>>, ApiError> {
    let id = parse_project_id(&id)?;
    // `list_branches` opens a `gix::Repository` (non-`Send`) across an await.
    let branches =
        run_local(move || async move { state.service.list_branches(&id, q.fetch).await }).await?;
    Ok(Json(branches))
}

/// `GET /projects/{id}/preview` → project `preview`.
pub async fn preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PreviewData>, ApiError> {
    let id = parse_project_id(&id)?;
    Ok(Json(
        state.service.preview(PreviewTarget::Project(id)).await?,
    ))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use axum::{Router, routing::get};
    use tempfile::TempDir;

    use crate::handlers::test_support::{get as do_get, json, send, test_state};

    /// Scanning an empty temp dir touches no repos → 200 `{added:0, skipped:0}`.
    /// (`scan_directory` is filesystem-only; it needs no tmux.)
    #[tokio::test]
    async fn scan_empty_dir_is_200_zero_counts() {
        let dir = TempDir::new().unwrap();
        let scan_target = TempDir::new().unwrap();
        let router = Router::new()
            .route("/projects/scan", get(super::scan))
            .with_state(test_state(&dir));
        let (status, body) = do_get(
            router,
            &format!("/projects/scan?dir={}", scan_target.path().display()),
        )
        .await;
        assert_eq!(status, 200);
        let resp: super::ScanResponse = json(&body);
        assert_eq!(resp.added, 0);
        assert_eq!(resp.skipped, 0);
    }

    /// `GET /projects` over empty state is a 200 empty array.
    #[tokio::test]
    async fn list_empty_is_200_empty_array() {
        let dir = TempDir::new().unwrap();
        let router = Router::new()
            .route("/projects", get(super::list))
            .with_state(test_state(&dir));
        let (status, body) = do_get(router, "/projects").await;
        assert_eq!(status, 200);
        let projects: Vec<claude_commander_core::api::ProjectInfo> = json(&body);
        assert!(projects.is_empty());
    }

    /// Deleting an unknown-but-well-formed project id is a 404.
    #[tokio::test]
    async fn delete_unknown_is_404() {
        let dir = TempDir::new().unwrap();
        let router = Router::new()
            .route("/projects/{id}", axum::routing::delete(super::delete))
            .with_state(test_state(&dir));
        let req = Request::delete(format!("/projects/{}", uuid::Uuid::new_v4()))
            .body(Body::empty())
            .unwrap();
        let (status, _) = send(router, req).await;
        assert_eq!(status, 404);
    }

    /// A malformed project id on the delete route is a 400.
    #[tokio::test]
    async fn delete_invalid_id_is_400() {
        let dir = TempDir::new().unwrap();
        let router = Router::new()
            .route("/projects/{id}", axum::routing::delete(super::delete))
            .with_state(test_state(&dir));
        let req = Request::delete("/projects/not-a-uuid")
            .body(Body::empty())
            .unwrap();
        let (status, _) = send(router, req).await;
        assert_eq!(status, 400);
    }

    /// Branch listing for an unknown project id is a 404.
    #[tokio::test]
    async fn branches_unknown_project_is_404() {
        let dir = TempDir::new().unwrap();
        let router = Router::new()
            .route("/projects/{id}/branches", get(super::branches))
            .with_state(test_state(&dir));
        let (status, _) = do_get(
            router,
            &format!("/projects/{}/branches", uuid::Uuid::new_v4()),
        )
        .await;
        assert_eq!(status, 404);
    }
}
