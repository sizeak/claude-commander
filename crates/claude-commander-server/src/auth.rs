//! Bearer-token authentication for the `/api` surface.
//!
//! A constant-time comparison guards against timing oracles on the token. When
//! auth is disabled (`--allow-no-auth`, loopback dev only) the middleware lets
//! every request through. The token is **never** logged.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{
        StatusCode,
        header::{AUTHORIZATION, HeaderValue, WWW_AUTHENTICATE},
    },
    middleware::Next,
    response::Response,
};

/// Resolved authentication policy shared across handlers via `AppState`.
#[derive(Debug, Clone)]
pub enum AuthConfig {
    /// A bearer token is required; requests must present `Authorization: Bearer <token>`.
    Token(String),
    /// Auth disabled (loopback dev). Every request is allowed.
    Disabled,
}

impl AuthConfig {
    /// Returns true if the supplied `Authorization` header value authorises the
    /// request. `Disabled` always authorises.
    pub fn authorizes(&self, header: Option<&str>) -> bool {
        match self {
            AuthConfig::Disabled => true,
            AuthConfig::Token(expected) => match header.and_then(parse_bearer) {
                Some(presented) => constant_time_eq(presented.as_bytes(), expected.as_bytes()),
                None => false,
            },
        }
    }

    /// Returns true if a bare token (not a `Bearer …` header) authorises the
    /// request. Used by the WebSocket handshake, where browsers can't set
    /// headers so the token arrives in an in-band `auth` frame. `Disabled`
    /// always authorises (and ignores any token). The comparison is
    /// constant-time, like [`Self::authorizes`].
    pub fn authorizes_token(&self, token: &str) -> bool {
        match self {
            AuthConfig::Disabled => true,
            AuthConfig::Token(expected) => constant_time_eq(token.as_bytes(), expected.as_bytes()),
        }
    }
}

/// Extract the token from a `Bearer <token>` header value.
fn parse_bearer(header: &str) -> Option<&str> {
    let rest = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))?;
    let token = rest.trim();
    if token.is_empty() { None } else { Some(token) }
}

/// Constant-time byte comparison. Length is leaked (unavoidable, and not
/// secret-revealing here), but content comparison takes time independent of
/// where the first mismatch is.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Tower middleware enforcing the bearer token on `/api` routes. A rejection
/// uses the same `{"error": {"kind", "message"}}` envelope as every handler
/// error (via [`crate::error::error_response`]) plus a `WWW-Authenticate: Bearer`
/// challenge, so a client parses an auth failure identically to any other.
pub async fn require_bearer(
    State(auth): State<Arc<AuthConfig>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let header = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if auth.authorizes(header) {
        next.run(request).await
    } else {
        unauthorized_response()
    }
}

/// The standard 401 response: the shared error envelope (`kind: "auth"`) plus a
/// `WWW-Authenticate: Bearer` challenge header.
fn unauthorized_response() -> Response {
    let mut response = crate::error::error_response(
        StatusCode::UNAUTHORIZED,
        "auth",
        "missing or invalid bearer token",
    );
    response
        .headers_mut()
        .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, http::Request, middleware::from_fn_with_state, routing::get};
    use tower::ServiceExt;

    fn protected_router(auth: AuthConfig) -> Router {
        let auth = Arc::new(auth);
        Router::new()
            .route("/ping", get(|| async { "pong" }))
            .layer(from_fn_with_state(auth, require_bearer))
    }

    async fn response_of(router: Router, header: Option<&str>) -> axum::response::Response {
        let mut req = Request::builder().uri("/ping");
        if let Some(h) = header {
            req = req.header(AUTHORIZATION, h);
        }
        router
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn status_of(router: Router, header: Option<&str>) -> StatusCode {
        response_of(router, header).await.status()
    }

    #[test]
    fn parse_bearer_extracts_token() {
        assert_eq!(parse_bearer("Bearer abc"), Some("abc"));
        assert_eq!(parse_bearer("bearer abc"), Some("abc"));
        assert_eq!(parse_bearer("Basic abc"), None);
        assert_eq!(parse_bearer("Bearer "), None);
    }

    #[test]
    fn constant_time_eq_basics() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"sekret"));
        assert!(!constant_time_eq(b"secret", b"secrets"));
    }

    #[tokio::test]
    async fn valid_bearer_is_200() {
        let router = protected_router(AuthConfig::Token("s3cret".into()));
        assert_eq!(
            status_of(router, Some("Bearer s3cret")).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn missing_bearer_is_401() {
        let router = protected_router(AuthConfig::Token("s3cret".into()));
        assert_eq!(status_of(router, None).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn wrong_bearer_is_401() {
        let router = protected_router(AuthConfig::Token("s3cret".into()));
        assert_eq!(
            status_of(router, Some("Bearer nope")).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn rejection_carries_error_envelope_and_challenge() {
        // A 401 from the auth layer must match the handler error envelope
        // (`{"error":{"kind","message"}}`) and advertise the scheme. Red against
        // HEAD, which returned a bare `StatusCode::UNAUTHORIZED` with an empty
        // body and no `WWW-Authenticate` header.
        use axum::body::to_bytes;

        let router = protected_router(AuthConfig::Token("s3cret".into()));
        let resp = response_of(router, Some("Bearer nope")).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers()
                .get(WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer"),
            "a 401 must advertise the bearer challenge"
        );
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["kind"], "auth");
        assert!(
            json["error"]["message"].is_string(),
            "envelope must carry a message string, got {json}"
        );
    }

    #[tokio::test]
    async fn disabled_auth_allows_anything() {
        let router = protected_router(AuthConfig::Disabled);
        assert_eq!(status_of(router, None).await, StatusCode::OK);
    }
}
