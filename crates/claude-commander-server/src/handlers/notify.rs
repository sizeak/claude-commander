//! Notify route: `POST /api/notify`.
//!
//! Relays a worker session's message to chat. The server owns the only chat
//! clients (workers never hold platform credentials); a worker running
//! `claude-commander notify` POSTs here and the [`ChatHub`] delivers it — into
//! the thread the session came from, or, for a session with no chat origin, as
//! a labelled direct message on the default backend.
//!
//! Status codes: 404 for an unknown session; 503 when this server can't
//! deliver at all (no backend running, the session's backend isn't running,
//! nobody to message); 502 when the platform rejects the delivery; 204 on
//! success. The `notify` telemetry feature is recorded in the
//! `CommanderService` method (the domain layer), not here.
//!
//! [`ChatHub`]: claude_commander_chat::ChatHub

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use claude_commander_protocol::api::NotifyRequest;

use crate::error::{ApiError, error_response};
use crate::state::AppState;

/// `POST /api/notify` → deliver `message` about `session` to chat.
pub async fn notify(
    State(state): State<AppState>,
    Json(req): Json<NotifyRequest>,
) -> Result<Response, ApiError> {
    let Some(target) = state.service.notify_target(&req.session).await? else {
        return Ok(error_response(
            StatusCode::NOT_FOUND,
            "session",
            format!("no session matches {:?}", req.session),
        ));
    };
    match state.chat.notify(&target, &req.message).await {
        Ok(_) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) if e.is_delivery_failure() => Ok(error_response(
            StatusCode::BAD_GATEWAY,
            "chat",
            e.to_string(),
        )),
        Err(e) => Ok(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "chat",
            e.to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::Request;
    use axum::{Router, routing::post};
    use claude_commander_chat::testing::{Call, FakeBackend};
    use claude_commander_chat::{ChatBackend, ChatHub};
    use claude_commander_core::api::CommanderService;
    use claude_commander_core::config::storage::AppState as CoreState;
    use claude_commander_core::config::{Config, ConfigStore, StateStore};
    use claude_commander_core::session::{ChatOrigin, Project, WorktreeSession};
    use claude_commander_core::telemetry::FrontendInfo;
    use tempfile::TempDir;

    use crate::auth::AuthConfig;
    use crate::handlers::test_support::send;
    use crate::state::AppState;

    /// A server with two sessions — `"from-chat"` (origin on the `fake`
    /// backend's `C9`/`555.5`) and `"local"` (no origin) — and `backends` as its
    /// chat hub.
    fn seeded_state(dir: &TempDir, backends: Vec<Arc<FakeBackend>>) -> AppState {
        let mut config = Config::default();
        config.telemetry.enabled = false;

        let mut core = CoreState::default();
        let mut project = Project::new("p", std::path::PathBuf::from("/tmp/p"), "main");
        let pid = project.id;

        let mut with_origin =
            WorktreeSession::new(pid, "from-chat", "fb", std::path::PathBuf::new(), "claude");
        with_origin.origin = Some(ChatOrigin {
            backend: "fake".to_string(),
            channel: "C9".to_string(),
            thread: "555.5".to_string(),
            permalink: None,
        });
        let without_origin =
            WorktreeSession::new(pid, "local", "lb", std::path::PathBuf::new(), "claude");
        project.add_worktree(with_origin.id);
        project.add_worktree(without_origin.id);
        core.projects.insert(pid, project);
        core.sessions.insert(with_origin.id, with_origin);
        core.sessions.insert(without_origin.id, without_origin);

        let config_store = Arc::new(ConfigStore::with_path(
            config,
            dir.path().join("config.toml"),
        ));
        let state_path = dir.path().join("state.json");
        std::fs::write(&state_path, serde_json::to_string(&core).unwrap()).unwrap();
        let store = Arc::new(StateStore::with_path(core, state_path));
        let service =
            CommanderService::new(config_store, store, FrontendInfo::new("test", "0.0.0"));
        let hub = ChatHub::new(
            backends
                .into_iter()
                .map(|b| b as Arc<dyn ChatBackend>)
                .collect(),
            None,
        );
        AppState::new(service, AuthConfig::Disabled).with_chat(Arc::new(hub))
    }

    fn router(state: AppState) -> Router {
        Router::new()
            .route("/notify", post(super::notify))
            .with_state(state)
    }

    fn notify_req(session: &str, message: &str) -> Request<Body> {
        Request::post("/notify")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "session": session, "message": message }).to_string(),
            ))
            .unwrap()
    }

    #[tokio::test]
    async fn origin_session_posts_into_its_thread() {
        let dir = TempDir::new().unwrap();
        let fake = Arc::new(FakeBackend::new("fake"));
        let app = router(seeded_state(&dir, vec![fake.clone()]));
        let (status, _) = send(app, notify_req("from-chat", "done")).await;
        assert_eq!(status, 204);
        assert_eq!(
            fake.calls(),
            vec![Call::Reply {
                channel: "C9".into(),
                thread: "555.5".into(),
                text: "rendered(done)".into(),
            }]
        );
    }

    #[tokio::test]
    async fn originless_session_dms_the_first_allowlisted_user() {
        let dir = TempDir::new().unwrap();
        let fake = Arc::new(FakeBackend::new("fake"));
        let app = router(seeded_state(&dir, vec![fake.clone()]));
        let (status, _) = send(app, notify_req("local", "ping")).await;
        assert_eq!(status, 204);
        let calls = fake.calls();
        assert!(
            matches!(&calls[..], [Call::Direct { user, text }]
                if user == "U1" && text.contains("**local") && text.contains("ping")),
            "{calls:?}"
        );
    }

    #[tokio::test]
    async fn unknown_session_is_404() {
        let dir = TempDir::new().unwrap();
        let app = router(seeded_state(&dir, vec![Arc::new(FakeBackend::new("fake"))]));
        let (status, _) = send(app, notify_req("nope", "x")).await;
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn no_chat_backend_is_503() {
        let dir = TempDir::new().unwrap();
        let app = router(seeded_state(&dir, Vec::new()));
        let (status, _) = send(app, notify_req("from-chat", "x")).await;
        assert_eq!(status, 503);
    }

    #[tokio::test]
    async fn an_origin_backend_that_is_not_running_is_503() {
        let dir = TempDir::new().unwrap();
        let app = router(seeded_state(
            &dir,
            vec![Arc::new(FakeBackend::new("other"))],
        ));
        let (status, _) = send(app, notify_req("from-chat", "x")).await;
        assert_eq!(status, 503);
    }

    #[tokio::test]
    async fn a_rejected_delivery_is_502() {
        let dir = TempDir::new().unwrap();
        let fake = Arc::new(FakeBackend::new("fake").failing_posts());
        let app = router(seeded_state(&dir, vec![fake]));
        let (status, _) = send(app, notify_req("from-chat", "x")).await;
        assert_eq!(status, 502);
    }
}
