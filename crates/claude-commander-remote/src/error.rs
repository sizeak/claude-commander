//! Mapping HTTP/transport failures onto the shared [`BackendError`] categories.
//!
//! The inverse of the server's status mapping (`server/src/error.rs`): a `404`
//! becomes [`BackendError::NotFound`], `401`/`403` → [`BackendError::Auth`],
//! `400`/`422`/`409` → [`BackendError::InvalidRequest`], `503` →
//! [`BackendError::Unavailable`], other `5xx` → [`BackendError::Server`], and a
//! JSON decode failure → [`BackendError::Protocol`]. A transport failure (no
//! response — connection refused, timeout, DNS) is [`BackendError::Unavailable`].
//!
//! **Token safety:** no path here reads the bearer token (it lives only in a
//! request header), and every reason string is derived from the HTTP status or
//! the server's own `{"error":{"message"}}` body — never from request headers —
//! so a token can't reach an error's `Display`/`Debug`. The
//! `token_never_appears_in_errors` test in `backend.rs` guards this.

use claude_commander_core::backend::BackendError;
use reqwest::{Response, StatusCode};

/// Map a transport-level reqwest error (the request never produced a response)
/// onto a backend category. Connection refused / timeout / DNS failures mean the
/// server is unreachable → [`BackendError::Unavailable`]; a stray decode error
/// is a protocol violation.
pub(crate) fn transport_error(err: reqwest::Error) -> BackendError {
    if err.is_decode() {
        BackendError::Protocol(short_transport_reason(&err))
    } else {
        BackendError::Unavailable {
            reason: short_transport_reason(&err),
        }
    }
}

/// Map an error reading/decoding a response body onto a backend category.
pub(crate) fn body_error(err: reqwest::Error) -> BackendError {
    if err.is_decode() {
        BackendError::Protocol(short_transport_reason(&err))
    } else {
        BackendError::Unavailable {
            reason: short_transport_reason(&err),
        }
    }
}

/// A short, user-facing reason for a transport failure. Deliberately does NOT
/// embed `err.to_string()` (which can be verbose and URL-bearing); the fixed
/// phrasing keeps toasts tidy and the reason deterministic for tests. The full
/// error is logged at `debug` by the caller.
fn short_transport_reason(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        "connection timed out".to_string()
    } else if err.is_connect() {
        "could not connect to server".to_string()
    } else if err.is_decode() {
        "malformed response from server".to_string()
    } else {
        "network error".to_string()
    }
}

/// Map a non-success HTTP status + an extracted message onto a backend category.
pub(crate) fn status_error(status: StatusCode, message: String) -> BackendError {
    match status {
        // Deliberately message-free so nothing sensitive can ride along.
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => BackendError::Auth,
        StatusCode::NOT_FOUND => BackendError::NotFound,
        // The backing service (tmux) is down, not the request's fault.
        StatusCode::SERVICE_UNAVAILABLE => BackendError::Unavailable {
            reason: if message.is_empty() {
                "remote service unavailable".to_string()
            } else {
                message
            },
        },
        _ if status.is_server_error() => BackendError::Server(message),
        // Any other 4xx (bad input, conflict, unsupported method, …) is a
        // request the server rejected.
        _ if status.is_client_error() => BackendError::InvalidRequest(message),
        // Anything else non-2xx (e.g. an unexpected 3xx we didn't follow).
        _ => BackendError::Server(format!("unexpected status {}", status.as_u16())),
    }
}

/// Consume an error response and produce a reason string, preferring the
/// server's `{"error":{"message":…}}` body and falling back to the status'
/// canonical phrase. The body is the server's own error text (a core error
/// string) — never anything token-bearing.
pub(crate) async fn error_message(resp: Response) -> String {
    let status = resp.status();
    let fallback = || {
        status
            .canonical_reason()
            .unwrap_or("request failed")
            .to_string()
    };
    match resp.text().await {
        Ok(text) => parse_error_message(&text).unwrap_or_else(fallback),
        Err(_) => fallback(),
    }
}

/// Pull `error.message` out of the server's uniform error body, if present.
fn parse_error_message(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value
        .get("error")?
        .get("message")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_maps_to_categories() {
        assert!(matches!(
            status_error(StatusCode::UNAUTHORIZED, "x".into()),
            BackendError::Auth
        ));
        assert!(matches!(
            status_error(StatusCode::FORBIDDEN, "x".into()),
            BackendError::Auth
        ));
        assert!(matches!(
            status_error(StatusCode::NOT_FOUND, "x".into()),
            BackendError::NotFound
        ));
        assert!(matches!(
            status_error(StatusCode::BAD_REQUEST, "bad".into()),
            BackendError::InvalidRequest(_)
        ));
        assert!(matches!(
            status_error(StatusCode::UNPROCESSABLE_ENTITY, "bad".into()),
            BackendError::InvalidRequest(_)
        ));
        assert!(matches!(
            status_error(StatusCode::CONFLICT, "conflict".into()),
            BackendError::InvalidRequest(_)
        ));
        assert!(matches!(
            status_error(StatusCode::SERVICE_UNAVAILABLE, "tmux".into()),
            BackendError::Unavailable { .. }
        ));
        assert!(matches!(
            status_error(StatusCode::INTERNAL_SERVER_ERROR, "boom".into()),
            BackendError::Server(_)
        ));
        assert!(matches!(
            status_error(StatusCode::BAD_GATEWAY, "boom".into()),
            BackendError::Server(_)
        ));
    }

    #[test]
    fn parses_server_error_body() {
        let body = r#"{"error":{"kind":"session","message":"no such session"}}"#;
        assert_eq!(
            parse_error_message(body).as_deref(),
            Some("no such session")
        );
    }

    #[test]
    fn parse_ignores_non_error_shapes() {
        assert!(parse_error_message("not json").is_none());
        assert!(parse_error_message(r#"{"other":1}"#).is_none());
        assert!(parse_error_message(r#"{"error":{}}"#).is_none());
    }
}
