//! Cascade-merge + push-stack handlers.
//!
//! Thin wrappers over `CommanderService`: `cascade_merge`, `cascade_resume`,
//! `cascade_abandon`, and `push_stack`. The cascade/push methods run the git
//! work (which builds a non-`Send` `gix::Repository`), so they go through
//! `run_local`; each returns an `OperationStatus` recorded in the service's
//! ledger, surfaced with `202 Accepted`.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use claude_commander_core::api::OperationStatus;

use crate::error::ApiError;
use crate::state::AppState;

use super::{parse_session_id, run_local};

/// `POST /sessions/{id}/cascade` → `cascade_merge` → 202 + `OperationStatus`.
pub async fn cascade(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = parse_session_id(&id)?;
    let status: OperationStatus =
        run_local(move || async move { state.service.cascade_merge(&id).await }).await?;
    Ok((StatusCode::ACCEPTED, Json(status)).into_response())
}

/// `POST /cascade/resume` → `cascade_resume` → 202 + `OperationStatus`.
pub async fn resume(State(state): State<AppState>) -> Result<Response, ApiError> {
    let status: OperationStatus =
        run_local(move || async move { state.service.cascade_resume().await }).await?;
    Ok((StatusCode::ACCEPTED, Json(status)).into_response())
}

/// `POST /cascade/abandon` → `cascade_abandon` → 204.
pub async fn abandon(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    state.service.cascade_abandon().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /sessions/{id}/push-stack` → `push_stack` → 202 + `OperationStatus`.
pub async fn push_stack(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id = parse_session_id(&id)?;
    let status: OperationStatus =
        run_local(move || async move { state.service.push_stack(&id).await }).await?;
    Ok((StatusCode::ACCEPTED, Json(status)).into_response())
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use axum::{Router, routing::post};
    use tempfile::TempDir;

    use crate::handlers::test_support::{send, test_state};
    use crate::state::AppState;

    fn router(state: AppState) -> Router {
        Router::new()
            .route("/sessions/{id}/cascade", post(super::cascade))
            .route("/sessions/{id}/push-stack", post(super::push_stack))
            .route("/cascade/resume", post(super::resume))
            .route("/cascade/abandon", post(super::abandon))
            .with_state(state)
    }

    /// A malformed session id on the cascade route is rejected as 400 before any
    /// git work runs.
    #[tokio::test]
    async fn cascade_invalid_id_is_400() {
        let dir = TempDir::new().unwrap();
        let req = Request::post("/sessions/not-a-uuid/cascade")
            .body(Body::empty())
            .unwrap();
        let (status, _) = send(router(test_state(&dir)), req).await;
        assert_eq!(status, 400);
    }

    /// Resuming with no cascade in progress is recorded as a failed operation
    /// and returned with 202 (the ledger carries the failure detail).
    #[tokio::test]
    async fn resume_without_cascade_records_failed_operation() {
        let dir = TempDir::new().unwrap();
        let req = Request::post("/cascade/resume")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(router(test_state(&dir)), req).await;
        assert_eq!(status, 202);
        let op: claude_commander_core::api::OperationStatus =
            serde_json::from_slice(&body).unwrap();
        assert!(matches!(
            op.outcome,
            claude_commander_core::api::OperationOutcome::Failed { .. }
        ));
    }

    /// Abandoning with no cascade in progress surfaces the core error (500).
    #[tokio::test]
    async fn abandon_without_cascade_errors() {
        let dir = TempDir::new().unwrap();
        let req = Request::post("/cascade/abandon")
            .body(Body::empty())
            .unwrap();
        let (status, _) = send(router(test_state(&dir)), req).await;
        assert_ne!(status, 204);
    }
}
