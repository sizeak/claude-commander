//! Session lifecycle + query handlers.
//!
//! Thin wrappers over `CommanderService`: `list_sessions`,
//! `find_session`/`find_session_exact`, `get_session_detail`,
//! `get_pane_content`, `create_session`, `kill_session`, `restart_session`,
//! `delete_session`.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use claude_commander_core::api::{CreateSessionOpts, SessionInfo};
use claude_commander_core::cli::SessionLookup;
use serde::Deserialize;
use serde_json::json;

use crate::error::ApiError;
use crate::state::AppState;

use super::{parse_session_id, run_local};

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub include_stopped: bool,
}

/// `GET /sessions?include_stopped=` → `list_sessions`.
pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<SessionInfo>>, ApiError> {
    Ok(Json(state.service.list_sessions(q.include_stopped).await?))
}

#[derive(Debug, Deserialize)]
pub struct FindQuery {
    pub q: String,
    #[serde(default)]
    pub exact: bool,
}

/// `GET /sessions/find?q=&exact=` → `find_session` / `find_session_exact`.
///
/// Loose (default) match: 404 when nothing matches. Exact match: 404 when
/// nothing matches, 409 when the query is ambiguous (several sessions share a
/// title), else the matched session.
pub async fn find(
    State(state): State<AppState>,
    Query(q): Query<FindQuery>,
) -> Result<Response, ApiError> {
    if q.exact {
        match state.service.find_session_exact(&q.q).await? {
            SessionLookup::Found(info) => Ok(Json(info).into_response()),
            SessionLookup::NotFound => Ok(StatusCode::NOT_FOUND.into_response()),
            SessionLookup::Ambiguous(n) => Ok((
                StatusCode::CONFLICT,
                Json(json!({
                    "error": {
                        "kind": "session",
                        "message": format!("{n} sessions match {:?}", q.q),
                    }
                })),
            )
                .into_response()),
        }
    } else {
        match state.service.find_session(&q.q).await? {
            Some(info) => Ok(Json(info).into_response()),
            None => Ok(StatusCode::NOT_FOUND.into_response()),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LinesQuery {
    pub lines: Option<usize>,
}

/// `GET /sessions/{q}/detail?lines=` → `get_session_detail` (404 if None).
pub async fn detail(
    State(state): State<AppState>,
    Path(q): Path<String>,
    Query(lq): Query<LinesQuery>,
) -> Result<Response, ApiError> {
    match state.service.get_session_detail(&q, lq.lines).await? {
        Some(detail) => Ok(Json(detail).into_response()),
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

/// `GET /sessions/{q}/pane?lines=` → `get_pane_content` (404 if None).
pub async fn pane(
    State(state): State<AppState>,
    Path(q): Path<String>,
    Query(lq): Query<LinesQuery>,
) -> Result<Response, ApiError> {
    match state.service.get_pane_content(&q, lq.lines).await? {
        Some(content) => Ok(content.into_response()),
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

/// `POST /sessions` → `create_session` → 201 `{ "id": ... }`.
///
/// The body deserializes straight into [`CreateSessionOpts`] (a shared wire type
/// in `claude-commander-protocol`) — no server-side mirror DTO.
pub async fn create(
    State(state): State<AppState>,
    Json(opts): Json<CreateSessionOpts>,
) -> Result<Response, ApiError> {
    // `create_session` builds a `gix::Repository` (non-`Send`) across an await.
    let id = run_local(move || async move { state.service.create_session(opts).await }).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

/// `POST /sessions/{id}/kill` → `kill_session` → 204.
pub async fn kill(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_session_id(&id)?;
    run_local(move || async move { state.service.kill_session(&id).await }).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /sessions/{id}/restart` → `restart_session` → 204.
pub async fn restart(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_session_id(&id)?;
    run_local(move || async move { state.service.restart_session(&id).await }).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /sessions/{id}` → `delete_session` → 204.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = parse_session_id(&id)?;
    run_local(move || async move { state.service.delete_session(&id).await }).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        routing::{get, post},
    };
    use tempfile::TempDir;

    use crate::handlers::test_support::{get as do_get, test_state};
    use crate::state::AppState;

    fn router(state: AppState) -> Router {
        Router::new()
            .route("/sessions", get(super::list).post(super::create))
            .route("/sessions/find", get(super::find))
            .route("/sessions/{q}/detail", get(super::detail))
            .route("/sessions/{q}/pane", get(super::pane))
            .route("/sessions/{id}/kill", post(super::kill))
            .route("/sessions/{id}/restart", post(super::restart))
            .route("/sessions/{id}", axum::routing::delete(super::delete))
            .with_state(state)
    }

    #[tokio::test]
    async fn list_empty_is_200_empty_array() {
        let dir = TempDir::new().unwrap();
        let (status, body) = do_get(router(test_state(&dir)), "/sessions").await;
        assert_eq!(status, 200);
        // `SessionInfo` is `Serialize`-only (a response DTO), so assert the
        // wire shape directly: an empty state yields an empty JSON array.
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value, serde_json::json!([]));
    }

    #[tokio::test]
    async fn find_loose_unknown_is_404() {
        let dir = TempDir::new().unwrap();
        let (status, _) = do_get(router(test_state(&dir)), "/sessions/find?q=nope").await;
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn find_exact_unknown_is_404() {
        let dir = TempDir::new().unwrap();
        let (status, _) =
            do_get(router(test_state(&dir)), "/sessions/find?q=nope&exact=true").await;
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn detail_unknown_is_404() {
        let dir = TempDir::new().unwrap();
        let (status, _) = do_get(router(test_state(&dir)), "/sessions/nope/detail").await;
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn pane_unknown_is_404() {
        let dir = TempDir::new().unwrap();
        let (status, _) = do_get(router(test_state(&dir)), "/sessions/nope/pane").await;
        assert_eq!(status, 404);
    }

    /// `detail` resolves its `{q}` as a free-form query, so an unmatched value
    /// is a 404 (not a parse error) — even one shaped like a non-UUID.
    #[tokio::test]
    async fn detail_treats_non_uuid_as_query_404() {
        let dir = TempDir::new().unwrap();
        let (status, _) = do_get(router(test_state(&dir)), "/sessions/not-a-uuid/detail").await;
        assert_eq!(status, 404);
    }

    /// A malformed UUID on an *id* route (kill) maps to 400 via
    /// `parse_session_id`, before any service call.
    #[tokio::test]
    async fn kill_invalid_id_is_400() {
        use axum::body::Body;
        use axum::http::Request;
        let dir = TempDir::new().unwrap();
        let req = Request::post("/sessions/not-a-uuid/kill")
            .body(Body::empty())
            .unwrap();
        let (status, _) = crate::handlers::test_support::send(router(test_state(&dir)), req).await;
        assert_eq!(status, 400);
    }
}
