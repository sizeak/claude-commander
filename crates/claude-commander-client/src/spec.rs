//! Connection details for a remote server, plus a redacting wrapper for the
//! bearer token so it can never leak through `Debug`/`Display`.

use std::fmt;

/// A secret string (the bearer token) whose `Debug` — and, deliberately, its
/// absence of `Display` — never reveal the wrapped value. The token only ever
/// reaches the wire as an `Authorization: Bearer …` header; wrapping it here
/// means an accidental `{:?}` on a [`RemoteServerSpec`] (or a struct embedding
/// one) can't spill it into a log line.
#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the raw secret. Crate-internal: only the request builder needs it.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(\"<redacted>\")")
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// How to reach one remote `claude-commander-server`.
///
/// `base_url` is the scheme + host + port (e.g. `https://box.example:8443`); an
/// optional path prefix is honoured. `token` is the bearer secret, or `None`
/// when the server runs with auth disabled (loopback dev). Derives `Debug`
/// safely because [`SecretString`] redacts itself.
#[derive(Clone, Debug)]
pub struct RemoteServerSpec {
    /// Human-readable label shown in the backend's server header.
    pub name: String,
    /// Base URL of the server (scheme + host + port, optional path prefix).
    pub base_url: String,
    /// Bearer token, or `None` for an auth-disabled server.
    pub token: Option<SecretString>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_is_redacted() {
        let s = SecretString::new("hunter2");
        assert_eq!(format!("{s:?}"), "SecretString(\"<redacted>\")");
        assert!(!format!("{s:?}").contains("hunter2"));
    }

    #[test]
    fn spec_debug_hides_token() {
        let spec = RemoteServerSpec {
            name: "box".to_string(),
            base_url: "https://box.example".to_string(),
            token: Some(SecretString::new("SUPERSECRET")),
        };
        let dbg = format!("{spec:?}");
        assert!(!dbg.contains("SUPERSECRET"), "token leaked in Debug: {dbg}");
        assert!(dbg.contains("box"));
    }

    #[test]
    fn expose_returns_raw_value() {
        assert_eq!(SecretString::new("abc").expose(), "abc");
    }
}
