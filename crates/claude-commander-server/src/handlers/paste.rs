//! Pasted-image upload handler.
//!
//! `POST /sessions/{id}/paste-image` accepts the raw image bytes as the request
//! body (the desktop TUI captures the operator's local clipboard image during a
//! remote attach and uploads it here). The bytes are validated + written to a
//! pruned temp file server-side and the file path is injected into the
//! session's Claude pane — a form the Claude CLI accepts. See
//! [`claude_commander_core::api::CommanderService::paste_image`] for the
//! security-relevant details (magic-byte sniffing, size limit, server-generated
//! filename, literal `send-keys`).

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
};
use serde_json::{Value, json};

use crate::error::ApiError;
use crate::state::AppState;

/// `POST /sessions/{id}/paste-image` — body is the raw image bytes. Returns
/// `{ "path": "<absolute path written on the server>" }`.
///
/// The route carries its own body-size limit (see the router) matching
/// [`claude_commander_core::paste_image::MAX_IMAGE_BYTES`]; the service also
/// re-checks the length so the limit holds regardless of how the handler is
/// mounted.
pub async fn paste_image(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let path = state.service.paste_image(&id, &body).await?;
    Ok(Json(json!({ "path": path.display().to_string() })))
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        routing::post,
    };
    use tempfile::TempDir;

    use crate::handlers::test_support::{send, test_state};

    fn router(dir: &TempDir) -> Router {
        Router::new()
            .route("/sessions/{id}/paste-image", post(super::paste_image))
            .with_state(test_state(dir))
    }

    async fn post_bytes(router: Router, id: &str, body: &[u8]) -> StatusCode {
        let req = Request::post(format!("/sessions/{id}/paste-image"))
            .header("content-type", "image/png")
            .body(Body::from(body.to_vec()))
            .unwrap();
        send(router, req).await.0
    }

    // A valid 1×1 PNG (matches the core paste_image tests).
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    /// A non-image body is rejected as a 400 before any session lookup, so this
    /// holds even though the fixture has no sessions.
    #[tokio::test]
    async fn non_image_body_is_400() {
        let dir = TempDir::new().unwrap();
        let status = post_bytes(router(&dir), "abc123", b"not an image").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// A valid image aimed at a session that doesn't exist is a 404 — validation
    /// passes, then the session resolve fails.
    #[tokio::test]
    async fn valid_image_unknown_session_is_404() {
        let dir = TempDir::new().unwrap();
        let status = post_bytes(router(&dir), "no-such-session", TINY_PNG).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
