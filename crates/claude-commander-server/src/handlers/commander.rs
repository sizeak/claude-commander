//! Headless-commander route: `POST /api/commander/ask`.
//!
//! A thin wrapper over [`CommanderService::commander_ask`]. The response is a
//! streamed NDJSON body — one JSON [`CommanderEvent`] per line — so a client
//! (the Slack bridge runs in-process; a CLI or the mobile app over HTTP) reads
//! `started` → `delta`* → `turn_complete`, or a terminal `error`, as they
//! happen rather than waiting for the whole reply.
//!
//! Not gated on `commander_enabled`: the service serves whenever a commander
//! program resolves (it always falls back to `claude`). A spawn/timeout failure
//! surfaces as a 200 response whose stream ends in an `error` event, matching
//! the NDJSON contract — the status line is committed before any turn work runs.

use axum::{
    Json,
    body::{Body, Bytes},
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use claude_commander_core::commander::headless::stream_event_to_wire;
use claude_commander_protocol::api::CommanderAskRequest;

use crate::state::AppState;

/// `POST /api/commander/ask` → chunked `application/x-ndjson` stream of events.
pub async fn ask(State(state): State<AppState>, Json(req): Json<CommanderAskRequest>) -> Response {
    let rx = state
        .service
        .commander_ask(&req.conversation_key, &req.prompt)
        .into_receiver();

    // One JSON object per line. Serialization of a fixed, small enum can't
    // realistically fail; an empty line on the theoretical error is harmless.
    let body = Body::from_stream(futures::stream::unfold(rx, |mut rx| async move {
        let ev = rx.recv().await?;
        let mut line = serde_json::to_vec(&stream_event_to_wire(ev)).unwrap_or_default();
        line.push(b'\n');
        Some((Ok::<Bytes, std::convert::Infallible>(Bytes::from(line)), rx))
    }));

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::Request;
    use axum::{Router, routing::post};
    use claude_commander_core::commander::headless::{CommanderChild, CommanderSpawn, SpawnSpec};
    use claude_commander_core::error::Result as CoreResult;
    use claude_commander_core::stream_json::StreamEvent;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::auth::AuthConfig;
    use crate::handlers::test_support::{send, test_state};
    use crate::state::AppState;

    /// A scripted fake process emitting a canned turn, so the route can be
    /// exercised without a real `claude` subprocess.
    struct FakeChild {
        queue: std::collections::VecDeque<StreamEvent>,
    }

    #[async_trait::async_trait]
    impl CommanderChild for FakeChild {
        async fn send(&mut self, _text: &str) -> CoreResult<()> {
            Ok(())
        }
        async fn recv(&mut self) -> Option<StreamEvent> {
            self.queue.pop_front()
        }
        async fn kill(&mut self) {}
    }

    struct FakeSpawn;

    #[async_trait::async_trait]
    impl CommanderSpawn for FakeSpawn {
        async fn spawn(&self, _spec: SpawnSpec) -> CoreResult<Box<dyn CommanderChild>> {
            Ok(Box::new(FakeChild {
                queue: [
                    StreamEvent::Started {
                        session_id: "sess-1".into(),
                    },
                    StreamEvent::Delta("Hello".into()),
                    StreamEvent::Delta(" there".into()),
                    StreamEvent::TurnComplete,
                ]
                .into_iter()
                .collect(),
            }))
        }
    }

    fn router(state: AppState) -> Router {
        Router::new()
            .route("/commander/ask", post(super::ask))
            .with_state(state)
    }

    fn stubbed_state(dir: &TempDir) -> AppState {
        let base = test_state(dir);
        let service = base.service.with_commander_spawn(Arc::new(FakeSpawn));
        AppState::new(service, AuthConfig::Disabled)
    }

    #[tokio::test]
    async fn ask_streams_ndjson_events() {
        let dir = TempDir::new().unwrap();
        let req = Request::post("/commander/ask")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "prompt": "hi", "conversation_key": "slack:C:T" }).to_string(),
            ))
            .unwrap();
        let (status, body) = send(router(stubbed_state(&dir)), req).await;
        assert_eq!(status, 200);

        let text = String::from_utf8(body).unwrap();
        let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
        // Each line is a self-describing JSON event; last is turn_complete.
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["type"], "started");
        assert_eq!(first["session_id"], "sess-1");
        let last: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
        assert_eq!(last["type"], "turn_complete");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("\"delta\"") && l.contains("Hello")),
            "expected a delta event carrying the text"
        );
    }

    #[tokio::test]
    async fn ask_requires_bearer_auth() {
        let dir = TempDir::new().unwrap();
        // A token-guarded service: the request carries no Authorization header.
        let base = test_state(&dir);
        let service = base.service.with_commander_spawn(Arc::new(FakeSpawn));
        let state = AppState::new(service, AuthConfig::Token("secret".into()));
        let app = Router::new()
            .route("/commander/ask", post(super::ask))
            .layer(axum::middleware::from_fn_with_state(
                state.auth.clone(),
                crate::auth::require_bearer,
            ))
            .with_state(state);

        let req = Request::post("/commander/ask")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "prompt": "hi", "conversation_key": "k" }).to_string(),
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 401);
    }
}
