//! Project handlers.
//!
//! Thin wrappers over `CommanderService`: `add_project`, `scan_directory`,
//! `ensure_project`.

use std::path::PathBuf;

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::ApiError;
use crate::handlers::run_local;
use crate::state::AppState;

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

#[cfg(test)]
mod tests {
    use axum::{Router, routing::get};
    use tempfile::TempDir;

    use crate::handlers::test_support::{get as do_get, json, test_state};

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
}
