//! HTTP Basic auth for BFF mode.
//!
//! Applied as a layer over the whole app when a commander token was supplied at
//! launch ([`AuthMode::Bff`]). Browsers replay cached Basic credentials on
//! same-origin requests — including the WebSocket upgrade — so this one layer
//! guards the SPA, the `/api` proxy, and `/ws/attach` alike. In
//! [`AuthMode::PassThrough`] the layer isn't installed at all (the commander
//! token the browser carries is the credential).

use axum::{
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::Engine as _;

use crate::config::{AppState, AuthMode};

/// Reject requests that don't present valid Basic credentials (BFF mode only).
pub async fn require_basic_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if let AuthMode::Bff {
        username, password, ..
    } = state.auth.as_ref()
    {
        let ok = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_basic)
            .map(|(u, p)| {
                // Bitwise-AND (not `&&`) so both comparisons always run — no
                // early-out timing oracle across the two fields.
                constant_time_eq(u.as_bytes(), username.as_bytes())
                    & constant_time_eq(p.as_bytes(), password.as_bytes())
            })
            .unwrap_or(false);
        if !ok {
            return unauthorized();
        }
    }
    next.run(req).await
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(
            header::WWW_AUTHENTICATE,
            "Basic realm=\"Claude Commander\", charset=\"UTF-8\"",
        )],
        "unauthorized",
    )
        .into_response()
}

/// Decode a `Basic <base64(user:pass)>` header into `(user, pass)`.
fn parse_basic(header: &str) -> Option<(String, String)> {
    let b64 = header
        .strip_prefix("Basic ")
        .or_else(|| header.strip_prefix("basic "))?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .ok()?;
    let s = String::from_utf8(decoded).ok()?;
    let (u, p) = s.split_once(':')?;
    Some((u.to_string(), p.to_string()))
}

/// Constant-time byte comparison. Length is leaked (unavoidable and not
/// sensitive here); content differences don't short-circuit.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_basic_header() {
        // base64("admin:pw") = YWRtaW46cHc=
        let got = parse_basic("Basic YWRtaW46cHc=");
        assert_eq!(got, Some(("admin".to_string(), "pw".to_string())));
    }

    #[test]
    fn rejects_malformed_basic_header() {
        assert_eq!(parse_basic("Bearer abc"), None);
        assert_eq!(parse_basic("Basic !!!notbase64"), None);
    }

    #[test]
    fn constant_time_eq_matches_and_differs() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreto"));
        assert!(!constant_time_eq(b"secret", b"Secret"));
    }
}
