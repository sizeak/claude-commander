//! Embedded web UI server.
//!
//! When enabled in config, this spins up an [`axum`] HTTP + WebSocket server
//! that shares the running [`CommanderService`] with the TUI, so a browser can
//! list, watch, drive, and jump into the same sessions the terminal UI manages.
//!
//! ## Surface
//! - `GET  /`                       — embedded single-page frontend (xterm.js)
//! - `GET  /api/sessions`           — list all sessions (incl. stopped)
//! - `GET  /api/sessions/:id`       — session detail (status, agent state, diff)
//! - `POST /api/sessions`           — create a session
//! - `POST /api/sessions/:id/restart`
//! - `POST /api/sessions/:id/kill`
//! - `DELETE /api/sessions/:id`     — delete a session
//! - `GET  /ws/sessions/:id`        — live terminal: streams pane output, accepts input
//!
//! ## Security
//! The server binds `0.0.0.0` so it is reachable from other machines — this is
//! deliberate (the point is remote control) but it means *every route and the
//! WebSocket upgrade are gated behind HTTP Basic auth*. Credentials are sent
//! base64-encoded over plain HTTP, so on untrusted networks this should sit
//! behind a TLS reverse proxy or SSH tunnel. Auth is mandatory: if the web UI
//! is enabled without a configured password, [`serve`] refuses to start rather
//! than run unauthenticated or rewrite the user's config with a generated one.

mod assets;
mod auth;
mod routes;
mod tls;

use std::sync::Arc;

use axum::Router;
use axum::middleware;
use tracing::{info, warn};

use crate::api::CommanderService;
use crate::config::{Config, WebUiAuth};
use crate::error::WebError;

pub use auth::{Credentials, basic_auth_matches, resolve_credentials};

/// Shared state handed to every route handler.
#[derive(Clone)]
pub(crate) struct WebState {
    pub service: CommanderService,
    /// Basic-auth credentials. `None` under mutual TLS, where the client
    /// certificate is the identity and there is no password.
    pub credentials: Option<Arc<Credentials>>,
    /// Poll interval (ms) for the terminal WebSocket. Mirrors the content
    /// capture TTL so we don't poll faster than the cache refreshes.
    pub stream_interval_ms: u64,
}

/// Bind and serve the web UI until the process exits or the listener errors.
///
/// Dispatches on [`Config::web_ui_auth`]: Basic auth runs over plain HTTP with
/// a password-checking middleware; mutual TLS runs over HTTPS where the rustls
/// handshake itself rejects any client without a CA-signed certificate (so no
/// password layer is needed). Returns a [`WebError`] on any setup/serve failure;
/// the caller (the spawned task in `App::run`) logs it and lets the rest of the
/// app continue — a failed web server must never take down the TUI.
pub async fn serve(service: CommanderService, config: Config) -> Result<(), WebError> {
    // Match the stream poll to the capture TTL, with a sane floor so a
    // misconfigured 0 doesn't spin.
    let stream_interval_ms = config.capture_cache_ttl_ms.max(50);

    match config.web_ui_auth {
        WebUiAuth::Basic => serve_basic(service, &config, stream_interval_ms).await,
        WebUiAuth::MutualTls => serve_mtls(service, &config, stream_interval_ms).await,
    }
}

/// Basic-auth over plain HTTP (the original path).
async fn serve_basic(
    service: CommanderService,
    config: &Config,
    stream_interval_ms: u64,
) -> Result<(), WebError> {
    let credentials = resolve_credentials(config).map_err(WebError::MissingPassword)?;
    let state = WebState {
        service,
        credentials: Some(Arc::new(credentials)),
        stream_interval_ms,
    };
    let app = build_app(state, /* with_basic_auth */ true);

    let addr = format!("0.0.0.0:{}", config.web_ui_port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|source| WebError::BindFailed {
            addr: addr.clone(),
            source,
        })?;

    info!(
        "Web UI listening on http://{addr} (Basic auth, user `admin`). \
         Bound on all interfaces — put it behind TLS/SSH on untrusted networks."
    );
    warn!("Web UI grants full remote control of every session. Keep the password secret.");

    axum::serve(listener, app)
        .await
        .map_err(|e| WebError::Serve(e.to_string()))
}

/// Mutual TLS over HTTPS: every client must present a CA-signed certificate.
async fn serve_mtls(
    service: CommanderService,
    config: &Config,
    stream_interval_ms: u64,
) -> Result<(), WebError> {
    let server_config = tls::build_server_config(config)?;
    let state = WebState {
        service,
        // No password under mTLS — the client cert is the identity.
        credentials: None,
        stream_interval_ms,
    };
    let app = build_app(state, /* with_basic_auth */ false);

    let addr = format!("0.0.0.0:{}", config.web_ui_port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|source| WebError::BindFailed {
            addr: addr.clone(),
            source,
        })?;

    info!(
        "Web UI listening on https://{addr} (mutual TLS — clients must present a \
         certificate signed by the configured CA). No password is used in this mode."
    );
    warn!("Web UI grants full remote control of every session. Guard the client certificates.");

    tls::serve_tls(listener, server_config, app).await
}

/// Assemble the full application: the route table, optionally wrapped with the
/// Basic-auth layer. The layer is applied for Basic mode and skipped for mutual
/// TLS (where the handshake already authenticated the client). Shared by
/// [`serve_basic`]/[`serve_mtls`] and the router tests so they exercise the same
/// stack.
fn build_app(state: WebState, with_basic_auth: bool) -> Router {
    let router = routes::router(state.clone());
    if with_basic_auth {
        router.layer(middleware::from_fn_with_state(
            state,
            auth::require_basic_auth,
        ))
    } else {
        router
    }
}

#[cfg(test)]
mod send_check {
    fn _assert_send<T: Send>() {}

    /// The web server hands a cloned [`CommanderService`] to axum handlers, which
    /// axum may move across threads — so the service must stay `Send`. It holds
    /// no `Rc`/gix state itself today (gix backends are built transiently inside
    /// method bodies), but if that ever regresses this guard fails at build time
    /// rather than surfacing as an inscrutable `Handler` trait error.
    #[test]
    fn commander_service_is_send() {
        _assert_send::<crate::api::CommanderService>();
    }
}

#[cfg(test)]
mod router_tests {
    use super::*;
    use crate::api::CommanderService;
    use crate::config::{AppState, ConfigStore, StateStore};
    use crate::telemetry::FrontendInfo;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use base64::Engine as _;
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _; // for `oneshot`

    fn test_app() -> Router {
        // Telemetry-off config so construction needs no network/runtime side effects.
        let mut config = crate::config::Config::default();
        config.telemetry.enabled = false;
        let config_store = std::sync::Arc::new(ConfigStore::new(config).unwrap());
        let store = std::sync::Arc::new(StateStore::new(AppState::new()).unwrap());
        let service =
            CommanderService::new(config_store, store, FrontendInfo::new("web-test", "0.0.0"));

        let state = WebState {
            service,
            credentials: Some(Arc::new(Credentials {
                username: "admin".to_string(),
                password: "pw".to_string(),
            })),
            stream_interval_ms: 50,
        };
        build_app(state, /* with_basic_auth */ true)
    }

    fn basic(user: &str, pass: &str) -> String {
        let raw = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
        format!("Basic {raw}")
    }

    #[tokio::test]
    async fn api_requires_auth() {
        let resp = test_app()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(resp.headers().contains_key(header::WWW_AUTHENTICATE));
    }

    #[tokio::test]
    async fn api_rejects_bad_password() {
        let resp = test_app()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .header(header::AUTHORIZATION, basic("admin", "wrong"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_lists_sessions_with_auth() {
        let resp = test_app()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .header(header::AUTHORIZATION, basic("admin", "pw"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        // No sessions in a fresh state → empty JSON array.
        assert_eq!(&body[..], b"[]");
    }

    #[tokio::test]
    async fn index_html_served_with_auth() {
        let resp = test_app()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::AUTHORIZATION, basic("admin", "pw"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
        assert!(ct.to_str().unwrap().starts_with("text/html"));
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert!(
            String::from_utf8_lossy(&body).contains("Claude Commander"),
            "index.html should contain the app title"
        );
    }

    #[tokio::test]
    async fn unknown_route_falls_back_to_index_html() {
        // The SPA fallback must report text/html even for an extension-less path
        // (regression: mime was inferred from the requested path, not the file
        // actually served).
        let resp = test_app()
            .oneshot(
                Request::builder()
                    .uri("/some/unknown/route")
                    .header(header::AUTHORIZATION, basic("admin", "pw"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
        assert!(
            ct.to_str().unwrap().starts_with("text/html"),
            "SPA fallback should be served as text/html, got {ct:?}"
        );
    }

    #[tokio::test]
    async fn invalid_session_id_is_bad_request() {
        let resp = test_app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions/not-a-uuid/restart")
                    .header(header::AUTHORIZATION, basic("admin", "pw"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    async fn get_json(app: Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::AUTHORIZATION, basic("admin", "pw"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn projects_requires_auth() {
        let resp = test_app()
            .oneshot(
                Request::builder()
                    .uri("/api/projects")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn lists_projects_empty_with_auth() {
        let (status, json) = get_json(test_app(), "/api/projects").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json, serde_json::json!([]));
    }

    #[tokio::test]
    async fn invalid_project_id_delete_is_bad_request() {
        let resp = test_app()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/projects/not-a-uuid")
                    .header(header::AUTHORIZATION, basic("admin", "pw"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn config_get_redacts_secret_and_reports_restart() {
        let (status, json) = get_json(test_app(), "/api/config").await;
        assert_eq!(status, StatusCode::OK);
        assert!(json.get("config").is_some());
        assert!(json.get("restart_required").is_some());
        // Default config has no password set → null (not the marker).
        assert!(json["config"]["web_ui_password"].is_null());
        // A non-secret scalar comes through as-is.
        assert_eq!(json["config"]["default_program"], "claude");
    }

    #[tokio::test]
    async fn meta_lists_form_options() {
        let (status, json) = get_json(test_app(), "/api/meta").await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            json["effort_levels"]
                .as_array()
                .unwrap()
                .contains(&"high".into())
        );
        assert!(
            json["permission_modes"]
                .as_array()
                .unwrap()
                .contains(&"plan".into())
        );
        assert!(json["sections"].is_array());
    }

    #[tokio::test]
    async fn config_put_updates_scalar_and_reports_restart() {
        let resp = test_app()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/config")
                    .header(header::AUTHORIZATION, basic("admin", "pw"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"branch_prefix":"web/"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("restart_required").is_some());
    }
}
