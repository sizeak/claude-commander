//! Workspace-surface handlers.
//!
//! Thin wrappers over `CommanderService`: the whole-workspace snapshot the
//! session tree renders from (`workspace_snapshot`), the bulk agent-state poll
//! (`agent_states`), and the new-session dialog options (`create_options`).

use axum::{
    Json,
    extract::{Query, State},
};
use claude_commander_core::api::{AgentStatesSnapshot, CreateOptions, WorkspaceSnapshot};
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /workspace` → `workspace_snapshot`.
pub async fn snapshot(State(state): State<AppState>) -> Result<Json<WorkspaceSnapshot>, ApiError> {
    Ok(Json(state.service.workspace_snapshot().await?))
}

#[derive(Debug, Deserialize)]
pub struct AgentStatesQuery {
    /// Bypass the shared TTL cache and force a fresh pane capture.
    #[serde(default)]
    pub fresh: bool,
}

/// `GET /agent-states?fresh=` → `agent_states`.
pub async fn agent_states(
    State(state): State<AppState>,
    Query(q): Query<AgentStatesQuery>,
) -> Json<AgentStatesSnapshot> {
    Json(state.service.agent_states(q.fresh).await)
}

/// `GET /create-options` → `create_options`.
pub async fn create_options(State(state): State<AppState>) -> Json<CreateOptions> {
    Json(state.service.create_options())
}

#[cfg(test)]
mod tests {
    use axum::{Router, routing::get};
    use tempfile::TempDir;

    use crate::handlers::test_support::{get as do_get, json, test_state};

    #[tokio::test]
    async fn workspace_empty_is_200_with_empty_lists() {
        let dir = TempDir::new().unwrap();
        let router = Router::new()
            .route("/workspace", get(super::snapshot))
            .with_state(test_state(&dir));
        let (status, body) = do_get(router, "/workspace").await;
        assert_eq!(status, 200);
        let snap: claude_commander_core::api::WorkspaceSnapshot = json(&body);
        assert!(snap.projects.is_empty());
        assert!(snap.sessions.is_empty());
        assert!(snap.cascade_paused.is_none());
        assert!(snap.operations.is_empty());
        // Version comes from the core crate's CARGO_PKG_VERSION.
        assert!(!snap.server.version.is_empty());
    }

    #[tokio::test]
    async fn create_options_is_200() {
        let dir = TempDir::new().unwrap();
        let router = Router::new()
            .route("/create-options", get(super::create_options))
            .with_state(test_state(&dir));
        let (status, body) = do_get(router, "/create-options").await;
        assert_eq!(status, 200);
        let opts: claude_commander_core::api::CreateOptions = json(&body);
        // Default config has a default program and no configured sections.
        assert!(!opts.default_program.is_empty());
        assert!(opts.sections.is_empty());
    }

    /// `agent-states` over empty state returns an empty map (no tmux needed
    /// because there are no active sessions to detect).
    #[tokio::test]
    async fn agent_states_empty_is_200() {
        let dir = TempDir::new().unwrap();
        let router = Router::new()
            .route("/agent-states", get(super::agent_states))
            .with_state(test_state(&dir));
        let (status, body) = do_get(router, "/agent-states").await;
        assert_eq!(status, 200);
        let snap: claude_commander_core::api::AgentStatesSnapshot = json(&body);
        assert!(snap.states.is_empty());
    }
}
