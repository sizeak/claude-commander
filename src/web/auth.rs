//! HTTP Basic auth for the web UI.
//!
//! The username is fixed (`admin`); the password comes from config. When the
//! web UI is enabled with no password set we generate one, persist it, and log
//! it — auth is never silently disabled, because the server binds all
//! interfaces.

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::config::Config;

use super::WebState;

/// Fixed username for the web UI's Basic auth.
pub const WEB_UI_USERNAME: &str = "admin";

/// Resolved credentials the server checks every request against.
#[derive(Debug, Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// Resolve the web UI credentials from config.
///
/// The web UI binds all interfaces, so it must be authenticated. We *require* a
/// configured password rather than generating one: generating-and-persisting
/// would rewrite the user's config file on launch (stripping comments, and
/// surprising them by mutating settings), so instead we refuse to start without
/// an explicit password. Returns `Err` with a user-facing message when none is
/// set; the caller declines to start the server and logs it.
pub fn resolve_credentials(config: &Config) -> Result<Credentials, String> {
    match config
        .web_ui_password
        .as_ref()
        .filter(|p| !p.trim().is_empty())
    {
        Some(pw) => Ok(Credentials {
            username: WEB_UI_USERNAME.to_string(),
            password: pw.clone(),
        }),
        None => Err(
            "web UI is enabled but no password is set — set `web_ui_password` in config \
             (or pass --password to `serve-web`). The web UI will not start without one."
                .to_string(),
        ),
    }
}

/// Constant-time-ish comparison of a decoded `user:pass` against the expected
/// credentials.
///
/// We OR per-byte differences across the full length of both fields so the
/// comparison time doesn't short-circuit on the first mismatching byte,
/// avoiding a trivial timing oracle on the password. Length differences are
/// folded in too.
pub fn basic_auth_matches(expected: &Credentials, user: &str, pass: &str) -> bool {
    constant_time_eq(user.as_bytes(), expected.username.as_bytes())
        & constant_time_eq(pass.as_bytes(), expected.password.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    // Fold length difference into the accumulator so equal-length-but-different
    // and different-length both fail, without an early return.
    let mut diff: u8 = (a.len() ^ b.len()) as u8;
    let max = a.len().max(b.len());
    for i in 0..max {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= x ^ y;
    }
    diff == 0
}

/// Parse a `Authorization: Basic <base64>` header value into `(user, pass)`.
/// Returns `None` for any malformed header.
pub fn parse_basic_auth(value: &HeaderValue) -> Option<(String, String)> {
    let value = value.to_str().ok()?;
    let encoded = value
        .strip_prefix("Basic ")
        .or_else(|| value.strip_prefix("basic "))?;
    let decoded = BASE64.decode(encoded.trim()).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (user, pass) = decoded.split_once(':')?;
    Some((user.to_string(), pass.to_string()))
}

/// Axum middleware: reject any request lacking valid Basic auth with `401` and
/// a `WWW-Authenticate` challenge so browsers prompt for credentials.
pub async fn require_basic_auth(
    State(state): State<WebState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    // This layer is only attached in Basic mode, where credentials are always
    // present. If they're somehow absent, fail closed (deny) rather than open.
    let ok = match state.credentials.as_ref() {
        Some(creds) => req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(parse_basic_auth)
            .map(|(u, p)| basic_auth_matches(creds, &u, &p))
            .unwrap_or(false),
        None => false,
    };

    if ok {
        return next.run(req).await;
    }

    let mut resp = Response::new(Body::from("Unauthorized"));
    *resp.status_mut() = StatusCode::UNAUTHORIZED;
    resp.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"claude-commander\""),
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds() -> Credentials {
        Credentials {
            username: "admin".to_string(),
            password: "s3cret".to_string(),
        }
    }

    #[test]
    fn accepts_matching_credentials() {
        assert!(basic_auth_matches(&creds(), "admin", "s3cret"));
    }

    #[test]
    fn rejects_wrong_password() {
        assert!(!basic_auth_matches(&creds(), "admin", "nope"));
    }

    #[test]
    fn rejects_wrong_user() {
        assert!(!basic_auth_matches(&creds(), "root", "s3cret"));
    }

    #[test]
    fn rejects_password_prefix() {
        // A correct prefix that is shorter must not pass (length folded in).
        assert!(!basic_auth_matches(&creds(), "admin", "s3cre"));
    }

    #[test]
    fn rejects_password_with_extra_bytes() {
        assert!(!basic_auth_matches(&creds(), "admin", "s3cret!"));
    }

    #[test]
    fn parses_well_formed_header() {
        let raw = BASE64.encode("admin:s3cret");
        let header = HeaderValue::from_str(&format!("Basic {raw}")).unwrap();
        assert_eq!(
            parse_basic_auth(&header),
            Some(("admin".to_string(), "s3cret".to_string()))
        );
    }

    #[test]
    fn parses_password_containing_colon() {
        let raw = BASE64.encode("admin:a:b:c");
        let header = HeaderValue::from_str(&format!("Basic {raw}")).unwrap();
        assert_eq!(
            parse_basic_auth(&header),
            Some(("admin".to_string(), "a:b:c".to_string()))
        );
    }

    #[test]
    fn rejects_non_basic_scheme() {
        let header = HeaderValue::from_static("Bearer token");
        assert_eq!(parse_basic_auth(&header), None);
    }

    #[test]
    fn rejects_garbage_base64() {
        let header = HeaderValue::from_static("Basic !!!notbase64!!!");
        assert_eq!(parse_basic_auth(&header), None);
    }

    #[test]
    fn rejects_missing_colon() {
        let raw = BASE64.encode("nocolon");
        let header = HeaderValue::from_str(&format!("Basic {raw}")).unwrap();
        assert_eq!(parse_basic_auth(&header), None);
    }

    #[test]
    fn resolve_credentials_requires_a_password() {
        // No password set → error (never auto-generate, never rewrite config).
        let config = Config {
            web_ui_password: None,
            ..Config::default()
        };
        assert!(resolve_credentials(&config).is_err());

        // Empty/whitespace password is treated as unset.
        let config = Config {
            web_ui_password: Some("   ".to_string()),
            ..Config::default()
        };
        assert!(resolve_credentials(&config).is_err());
    }

    #[test]
    fn resolve_credentials_uses_configured_password() {
        let config = Config {
            web_ui_password: Some("hunter2".to_string()),
            ..Config::default()
        };
        let creds = resolve_credentials(&config).unwrap();
        assert_eq!(creds.username, WEB_UI_USERNAME);
        assert_eq!(creds.password, "hunter2");
    }
}
