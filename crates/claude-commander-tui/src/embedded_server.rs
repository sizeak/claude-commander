//! What the TUI knows about the HTTP server running inside its own process.
//!
//! Deliberately just facts, not machinery: the binary starts the server (it owns
//! the dependency on `claude-commander-server`; this crate must not, or the
//! terminal frontend would compile axum) and hands the outcome down with
//! [`App::set_embedded_server`](crate::App::set_embedded_server). The TUI's job
//! is to show it and to offer the token for pairing a client.

/// The outcome of trying to start the embedded server.
///
/// A failure is kept rather than discarded because the common cause — the port
/// is already taken — is worth telling the operator about: it usually means a
/// standalone server is already running, in which case nothing is wrong, but it
/// can equally mean the port is held by something else entirely.
#[derive(Clone, PartialEq, Eq)]
pub enum EmbeddedServerStatus {
    /// Bound and serving.
    Listening {
        /// Base URL a client can be pointed at, e.g. `http://127.0.0.1:7878`.
        url: String,
        /// The bearer token clients must present, or `None` when authentication
        /// is disabled.
        token: Option<String>,
    },
    /// The listener could not be bound. The TUI runs on regardless.
    Failed {
        /// Operator-facing reason, already formatted.
        reason: String,
    },
}

impl EmbeddedServerStatus {
    /// The base URL, if the server came up.
    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Listening { url, .. } => Some(url),
            Self::Failed { .. } => None,
        }
    }

    /// The bearer token, if there is one to hand out.
    pub fn token(&self) -> Option<&str> {
        match self {
            Self::Listening { token, .. } => token.as_deref(),
            Self::Failed { .. } => None,
        }
    }
}

impl std::fmt::Debug for EmbeddedServerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redacted at the value, not at each log site. `AppUiState` derives no
        // `Debug` today, so nothing prints this yet — which is exactly when to
        // do it: a token that survives one `Debug` survives every future one,
        // and whoever adds that `Debug` will not be thinking about this field.
        match self {
            Self::Listening { url, token } => f
                .debug_struct("Listening")
                .field("url", url)
                .field(
                    "token",
                    &token.as_ref().map(|_| "<redacted>").unwrap_or("None"),
                )
                .finish(),
            Self::Failed { reason } => f.debug_struct("Failed").field("reason", reason).finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_the_token() {
        let status = EmbeddedServerStatus::Listening {
            url: "http://127.0.0.1:7878".into(),
            token: Some("sekret".into()),
        };
        let rendered = format!("{status:?}");
        assert!(!rendered.contains("sekret"), "token leaked: {rendered}");
        assert!(rendered.contains("http://127.0.0.1:7878"), "{rendered}");
    }

    #[test]
    fn accessors_report_nothing_for_a_failed_start() {
        let status = EmbeddedServerStatus::Failed {
            reason: "could not bind 127.0.0.1:7878: Address already in use".into(),
        };
        assert!(status.url().is_none());
        assert!(status.token().is_none());
    }
}
