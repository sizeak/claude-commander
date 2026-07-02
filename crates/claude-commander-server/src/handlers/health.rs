//! Liveness handler.
//!
//! `/health` is a cheap, unauthenticated liveness probe: it only proves the
//! process is up and serving, doing no I/O. Readiness of the tmux backend is a
//! separate probe at `/api/health/tmux` (see [`super::config::health_tmux`]).

use axum::http::StatusCode;

/// `GET /health` → 200 `"ok"`. Lightweight liveness only.
pub async fn live() -> (StatusCode, &'static str) {
    (StatusCode::OK, "ok")
}

#[cfg(test)]
mod tests {
    use axum::{Router, routing::get};

    use crate::handlers::test_support::get as do_get;

    #[tokio::test]
    async fn health_is_200_ok() {
        let router = Router::new().route("/health", get(super::live));
        let (status, body) = do_get(router, "/health").await;
        assert_eq!(status, 200);
        assert_eq!(&body, b"ok");
    }
}
