//! Router construction: the `/api` surface (behind bearer auth + a CORS layer),
//! the `/ws` upgrade, and a lightweight `/health` liveness probe.

use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderValue, Method, header::AUTHORIZATION},
    middleware::from_fn_with_state,
    routing::{delete, get, post, put},
};
use tower_http::{
    catch_panic::CatchPanicLayer,
    cors::{AllowOrigin, CorsLayer},
};
use tracing::warn;

use crate::auth::require_bearer;
use crate::handlers::{
    blobs, cascade, config, github, health, paste, projects, review, sessions, workspace,
};
use crate::state::AppState;
use crate::ws;

/// Build the CORS layer for the `/api` surface from the configured allowlist.
///
/// An empty allowlist denies all cross-origin requests (same-origin only): no
/// `Access-Control-Allow-Origin` header is ever emitted, so browsers block
/// cross-origin reads. A non-empty allowlist permits exactly those origins
/// (each parsed to a `HeaderValue`; unparseable entries are dropped with a
/// warning), plus the methods/headers our API actually uses.
fn cors_layer(allowed_origins: &[String]) -> CorsLayer {
    let origins: Vec<HeaderValue> = allowed_origins
        .iter()
        .filter_map(|o| match HeaderValue::from_str(o) {
            Ok(v) => Some(v),
            Err(e) => {
                warn!("ignoring invalid CORS origin {o:?}: {e}");
                None
            }
        })
        .collect();

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers([AUTHORIZATION, axum::http::header::CONTENT_TYPE])
}

/// Build the full application router.
pub fn build_router(state: AppState) -> Router {
    let auth = state.auth.clone();
    let cors = cors_layer(&state.cors_allowed_origins);

    let api = Router::new()
        // -- workspace surface --
        .route("/workspace", get(workspace::snapshot))
        .route("/agent-states", get(workspace::agent_states))
        .route("/pr-refresh", post(workspace::pr_refresh))
        .route("/create-options", get(workspace::create_options))
        .route("/comments/pending", get(review::pending))
        // -- cascade / push-stack --
        .route("/cascade/resume", post(cascade::resume))
        .route("/cascade/abandon", post(cascade::abandon))
        // -- sessions --
        .route("/sessions", get(sessions::list).post(sessions::create))
        .route("/sessions/find", get(sessions::find))
        .route("/sessions/unread", post(sessions::unread))
        .route("/sessions/{q}/detail", get(sessions::detail))
        .route("/sessions/{q}/pane", get(sessions::pane))
        .route("/sessions/{id}/kill", post(sessions::kill))
        .route("/sessions/{id}/restart", post(sessions::restart))
        .route(
            "/sessions/{id}",
            delete(sessions::delete).patch(sessions::patch),
        )
        .route("/sessions/{id}/preview", get(sessions::preview))
        .route("/sessions/{id}/branch-diff", get(sessions::branch_diff))
        .route("/sessions/{id}/read", post(sessions::read))
        .route("/sessions/{id}/keep-alive", post(sessions::keep_alive))
        .route("/sessions/{id}/cascade", post(cascade::cascade))
        .route("/sessions/{id}/push-stack", post(cascade::push_stack))
        // -- review + comments --
        .route("/sessions/{id}/review", get(review::open))
        .route("/sessions/{id}/review/refresh", get(review::refresh))
        .route(
            "/sessions/{id}/comments",
            get(review::list_comments).post(review::create_comment),
        )
        .route(
            "/sessions/{id}/comments/{cid}",
            delete(review::delete_comment),
        )
        .route("/sessions/{id}/comments/apply", post(review::apply))
        .route(
            "/sessions/{id}/files/reviewed",
            post(review::toggle_reviewed),
        )
        // -- blobs --
        .route("/sessions/{id}/blob", get(blobs::fetch))
        // -- pasted-image upload (raw image body) --
        // The route carries its own body-size limit sized to the paste-image cap
        // (the global axum default is 2 MiB, too small for a screenshot); the
        // service re-checks the length as defence in depth.
        .route(
            "/sessions/{id}/paste-image",
            post(paste::paste_image).route_layer(DefaultBodyLimit::max(
                claude_commander_protocol::paste::MAX_IMAGE_BYTES,
            )),
        )
        // -- projects --
        .route("/projects", get(projects::list).post(projects::add))
        .route("/projects/scan", post(projects::scan))
        .route("/projects/ensure", post(projects::ensure))
        .route("/projects/{id}", delete(projects::delete))
        .route("/projects/{id}/branches", get(projects::branches))
        .route("/projects/{id}/preview", get(projects::preview))
        // -- repo picker + clone --
        // `/projects/clone` sits alongside `/projects/{id}`, and it **does** rely
        // on the router preferring a static segment over a capture: without that,
        // "clone" binds to `{id}` and a POST here reaches a path that only
        // registers DELETE, i.e. a 405 rather than this handler. The differing
        // methods do not save it — they are what the failure would look like.
        // `routes_reach_the_clone_handlers` is the receipt: it drives the real
        // router and identifies the handler by a message only it produces, so a
        // capture match shows up as a wrong status/body rather than passing
        // quietly.
        .route("/github/repos", get(github::repos))
        .route("/projects/clone", post(github::clone))
        .route("/projects/clone/{job}", get(github::clone_status))
        // -- config + health --
        .route("/config", get(config::read).patch(config::update))
        .route("/config/programs", put(config::put_programs))
        .route("/config/reload", post(config::reload))
        .route("/health/tmux", get(config::health_tmux))
        // Bearer auth guards the whole `/api` surface; the CORS layer sits
        // outside auth so browser preflight (OPTIONS, unauthenticated) is
        // answered correctly.
        .layer(from_fn_with_state(auth, require_bearer))
        .layer(cors);

    // The WS handshake authenticates in-band (browsers can't set headers on the
    // upgrade), so `/ws` sits outside the `/api` bearer layer.
    let ws = Router::new().route("/attach", get(ws::attach));

    Router::new()
        .nest("/api", api)
        .nest("/ws", ws)
        // Lightweight liveness probe, outside the auth layer.
        .route("/health", get(health::live))
        // Defense-in-depth: a panicking handler returns 500 instead of dropping
        // the connection (complements `run_local`'s explicit 500 mapping).
        .layer(CatchPanicLayer::new())
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::handlers::test_support::test_state;

    const ALLOWED: &str = "https://app.example.com";
    const OTHER: &str = "https://evil.example.com";

    /// An allowed origin gets an `Access-Control-Allow-Origin` header echoing it.
    #[tokio::test]
    async fn allowed_origin_gets_cors_header() {
        let dir = TempDir::new().unwrap();
        let state = test_state(&dir).with_cors(vec![ALLOWED.to_string()]);
        let app = super::build_router(state);

        let req = Request::get("/api/config")
            .header(header::ORIGIN, ALLOWED)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let allow = resp
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|v| v.to_str().ok());
        assert_eq!(allow, Some(ALLOWED));
    }

    /// An origin not on the allowlist gets no `Access-Control-Allow-Origin`
    /// header, so a browser blocks the cross-origin read.
    #[tokio::test]
    async fn unlisted_origin_gets_no_cors_header() {
        let dir = TempDir::new().unwrap();
        let state = test_state(&dir).with_cors(vec![ALLOWED.to_string()]);
        let app = super::build_router(state);

        let req = Request::get("/api/config")
            .header(header::ORIGIN, OTHER)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none(),
            "unlisted origin must not receive a CORS allow header"
        );
    }

    /// The clone routes resolve to *their own* handlers through the real router.
    ///
    /// `/projects/clone` sits next to `/projects/{id}`, so the literal "clone"
    /// could be captured as a project id and `/projects/clone/{job}` could be read
    /// as some `/projects/{id}/…` sub-route. Both assertions below identify the
    /// handler by a message only it produces, so a mis-resolution shows up as a
    /// wrong body rather than passing on a coincidentally equal status.
    #[tokio::test]
    async fn routes_reach_the_clone_handlers() {
        let dir = TempDir::new().unwrap();
        let app = super::build_router(test_state(&dir));

        // A malformed job id → 400 from `parse_clone_job_id`. Routed to
        // `/projects/{id}/…` instead this would be a 404 or a project-id message.
        let req = Request::get("/api/projects/clone/not-a-uuid")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 400);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("not a valid clone job id"),
            "not the clone-status handler: {body}"
        );

        // A rejected source → 400 from the clone handler. A routing miss would be
        // a 404 (no such path) or 405 (wrong method on `/projects/{id}`).
        let req = Request::post("/api/projects/clone")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({ "source": { "kind": "github", "full_name": "nope" } })
                    .to_string(),
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 400);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("owner/name repository slug"),
            "not the clone handler: {body}"
        );
    }

    /// With an empty allowlist (default), even a syntactically valid origin is
    /// denied — same-origin only.
    #[tokio::test]
    async fn empty_allowlist_denies_cross_origin() {
        let dir = TempDir::new().unwrap();
        let state = test_state(&dir); // no CORS configured
        let app = super::build_router(state);

        let req = Request::get("/api/config")
            .header(header::ORIGIN, ALLOWED)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none(),
            "empty allowlist must not emit a CORS allow header"
        );
    }
}
