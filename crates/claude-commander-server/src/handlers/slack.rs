//! Slack notify route: `POST /api/slack/notify`.
//!
//! Relays a worker session's message to Slack. The server owns the only Slack
//! client (workers never hold Slack credentials); a worker running
//! `claude-commander slack notify` POSTs here and the server delivers it.
//!
//! Resolution mirrors the design: a session created from Slack carries a
//! `slack_origin`, so its message posts back into that channel/thread; a session
//! with no origin has its message DM'd to the first allowlisted user, prefixed
//! with a label so it's identifiable out of context.
//!
//! Status codes: 503 when Slack isn't enabled/running (no client on the state),
//! 404 for an unknown session, 502 when Slack itself rejects the delivery, 204
//! on success. The `slack_notify` telemetry feature is recorded in the
//! `CommanderService` method (the domain layer), not here.

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use claude_commander_protocol::api::SlackNotifyRequest;

use crate::error::{ApiError, error_response};
use crate::state::AppState;

/// `POST /api/slack/notify` → deliver `message` to Slack for `session`.
pub async fn notify(
    State(state): State<AppState>,
    Json(req): Json<SlackNotifyRequest>,
) -> Result<Response, ApiError> {
    // No Slack client means the bridge isn't running (Slack disabled) — 503.
    let Some(slack) = state.slack.clone() else {
        return Ok(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "slack",
            "Slack is not enabled on this server",
        ));
    };

    let Some(target) = state.service.slack_notify_target(&req.session).await? else {
        return Ok(error_response(
            StatusCode::NOT_FOUND,
            "session",
            format!("no session matches {:?}", req.session),
        ));
    };

    let delivery = match &target.origin {
        // Created from Slack: reply into the originating thread.
        Some(origin) => {
            slack
                .post_message(&origin.channel, &origin.thread_ts, &req.message)
                .await
        }
        // No originating thread: DM the first allowlisted user, naming the
        // session so the message is identifiable out of context.
        None => {
            let allowed = state.service.read_config().slack.allowed_user_ids;
            let Some(user) = allowed.first() else {
                return Ok(error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "slack",
                    "Slack has no allowlisted user to notify",
                ));
            };
            let text = format!("*{}*\n{}", target.label, req.message);
            match slack.open_dm(user).await {
                Ok(channel) => slack.post_channel_message(&channel, &text).await,
                Err(e) => Err(e),
            }
        }
    };

    match delivery {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => Ok(error_response(
            StatusCode::BAD_GATEWAY,
            "slack",
            format!("Slack delivery failed: {e}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use axum::{Router, routing::post};
    use claude_commander_core::api::CommanderService;
    use claude_commander_core::config::storage::AppState as CoreState;
    use claude_commander_core::config::{Config, ConfigStore, StateStore};
    use claude_commander_core::session::{Project, SlackOrigin, WorktreeSession};
    use claude_commander_core::telemetry::FrontendInfo;
    use tempfile::TempDir;

    use crate::auth::AuthConfig;
    use crate::handlers::test_support::send;
    use crate::slack::decision::ThreadMessage;
    use crate::slack::handler::{SlackApi, SlackError, SlackResult};
    use crate::state::AppState;

    /// A fake Slack client recording each call as a compact string so a test can
    /// assert the routing decision (thread post vs DM open + channel post).
    #[derive(Default)]
    struct FakeSlack {
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl SlackApi for FakeSlack {
        async fn post_message(
            &self,
            channel: &str,
            thread_ts: &str,
            text: &str,
        ) -> SlackResult<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("post_message:{channel}:{thread_ts}:{text}"));
            Ok(())
        }
        async fn add_reaction(&self, _c: &str, _ts: &str, _n: &str) -> SlackResult<()> {
            Ok(())
        }
        async fn remove_reaction(&self, _c: &str, _ts: &str, _n: &str) -> SlackResult<()> {
            Ok(())
        }
        async fn fetch_replies(&self, _c: &str, _t: &str) -> SlackResult<Vec<ThreadMessage>> {
            Ok(Vec::new())
        }
        async fn get_permalink(&self, _c: &str, _ts: &str) -> SlackResult<String> {
            Ok("https://slack/x".into())
        }
        async fn open_dm(&self, user_id: &str) -> SlackResult<String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("open_dm:{user_id}"));
            Ok(format!("D-{user_id}"))
        }
        async fn post_channel_message(&self, channel: &str, text: &str) -> SlackResult<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("post_channel_message:{channel}:{text}"));
            Ok(())
        }
    }

    /// A fake that always fails delivery, for the 502 path.
    struct FailingSlack;

    #[async_trait]
    impl SlackApi for FailingSlack {
        async fn post_message(&self, _c: &str, _t: &str, _x: &str) -> SlackResult<()> {
            Err(SlackError("boom".into()))
        }
        async fn add_reaction(&self, _c: &str, _ts: &str, _n: &str) -> SlackResult<()> {
            Ok(())
        }
        async fn remove_reaction(&self, _c: &str, _ts: &str, _n: &str) -> SlackResult<()> {
            Ok(())
        }
        async fn fetch_replies(&self, _c: &str, _t: &str) -> SlackResult<Vec<ThreadMessage>> {
            Ok(Vec::new())
        }
        async fn get_permalink(&self, _c: &str, _ts: &str) -> SlackResult<String> {
            Ok("https://slack/x".into())
        }
        async fn open_dm(&self, _u: &str) -> SlackResult<String> {
            Err(SlackError("boom".into()))
        }
        async fn post_channel_message(&self, _c: &str, _t: &str) -> SlackResult<()> {
            Err(SlackError("boom".into()))
        }
    }

    /// Build a hermetic [`AppState`] seeded with one project and two sessions:
    /// `"from-slack"` (carries a `slack_origin`) and `"local"` (no origin). The
    /// config allowlists user `U1`. `slack` is the injected client (or `None`).
    fn seeded_state(dir: &TempDir, slack: Option<Arc<dyn SlackApi>>) -> AppState {
        let mut config = Config::default();
        config.telemetry.enabled = false;
        config.slack.allowed_user_ids = vec!["U1".to_string()];

        let mut core = CoreState::default();
        let mut project = Project::new("p", std::path::PathBuf::from("/tmp/p"), "main");
        let pid = project.id;

        let mut with_origin =
            WorktreeSession::new(pid, "from-slack", "sb", std::path::PathBuf::new(), "claude");
        with_origin.slack_origin = Some(SlackOrigin {
            channel: "C9".to_string(),
            thread_ts: "555.5".to_string(),
            permalink: "https://slack/p".to_string(),
        });
        let mut without_origin =
            WorktreeSession::new(pid, "local", "lb", std::path::PathBuf::new(), "claude");
        without_origin.status = claude_commander_core::SessionStatus::Running;
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
        AppState::new(service, AuthConfig::Disabled).with_slack(slack)
    }

    fn router(state: AppState) -> Router {
        Router::new()
            .route("/slack/notify", post(super::notify))
            .with_state(state)
    }

    fn notify_req(session: &str, message: &str) -> Request<Body> {
        Request::post("/slack/notify")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "session": session, "message": message }).to_string(),
            ))
            .unwrap()
    }

    #[tokio::test]
    async fn origin_session_posts_into_its_thread() {
        let dir = TempDir::new().unwrap();
        let fake = Arc::new(FakeSlack::default());
        let state = seeded_state(&dir, Some(fake.clone()));
        let (status, _) = send(router(state), notify_req("from-slack", "done")).await;
        assert_eq!(status, 204);

        let calls = fake.calls.lock().unwrap().clone();
        assert_eq!(calls, vec!["post_message:C9:555.5:done".to_string()]);
    }

    #[tokio::test]
    async fn originless_session_dms_first_allowlisted_user() {
        let dir = TempDir::new().unwrap();
        let fake = Arc::new(FakeSlack::default());
        let state = seeded_state(&dir, Some(fake.clone()));
        let (status, _) = send(router(state), notify_req("local", "ping")).await;
        assert_eq!(status, 204);

        let calls = fake.calls.lock().unwrap().clone();
        // Opens a DM with the first allowlisted user, then posts a labelled
        // message to that DM channel (no thread post).
        assert_eq!(calls[0], "open_dm:U1");
        assert!(
            calls[1].starts_with("post_channel_message:D-U1:*local"),
            "DM message must name the session; got {:?}",
            calls[1]
        );
        assert!(calls[1].contains("ping"));
        assert!(!calls.iter().any(|c| c.starts_with("post_message:")));
    }

    #[tokio::test]
    async fn unknown_session_is_404() {
        let dir = TempDir::new().unwrap();
        let state = seeded_state(&dir, Some(Arc::new(FakeSlack::default())));
        let (status, _) = send(router(state), notify_req("nope", "x")).await;
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn slack_disabled_is_503() {
        let dir = TempDir::new().unwrap();
        // No Slack client on the state → Slack unavailable.
        let state = seeded_state(&dir, None);
        let (status, _) = send(router(state), notify_req("from-slack", "x")).await;
        assert_eq!(status, 503);
    }

    #[tokio::test]
    async fn slack_delivery_failure_is_502() {
        let dir = TempDir::new().unwrap();
        let state = seeded_state(&dir, Some(Arc::new(FailingSlack)));
        let (status, _) = send(router(state), notify_req("from-slack", "x")).await;
        assert_eq!(status, 502);
    }
}
