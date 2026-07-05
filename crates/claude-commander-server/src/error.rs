//! Server-local error type.
//!
//! Wraps the core library's [`claude_commander_core::Error`] and maps its
//! variants onto HTTP status codes, rendering a uniform JSON body
//! `{"error": {"kind", "message"}}`. The core `Error` is **not** modified —
//! this mapping lives entirely in the server crate (the dependency direction is
//! server → core, never the reverse), mirroring the existing `TtsError::Status`
//! pattern in core.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use claude_commander_core::Error as CoreError;
use claude_commander_core::backend::RunLocalError;
use claude_commander_core::error::{SessionError, TmuxError};
use serde_json::json;

/// An error returned from an API handler. Wraps a [`CoreError`] and maps it to
/// an HTTP status + JSON body via [`IntoResponse`].
#[derive(Debug)]
pub struct ApiError(pub CoreError);

impl From<CoreError> for ApiError {
    fn from(err: CoreError) -> Self {
        ApiError(err)
    }
}

impl From<RunLocalError<CoreError>> for ApiError {
    /// A `!Send` core call routed through [`run_local`](claude_commander_core::backend::run_local):
    /// an inner core error keeps its usual status mapping; a lost worker thread
    /// (panic) becomes a 500, so a handler's `run_local(...).await?` behaves
    /// exactly as it did when `run_local` lived in this crate.
    fn from(err: RunLocalError<CoreError>) -> Self {
        match err {
            RunLocalError::Inner(e) => ApiError(e),
            RunLocalError::WorkerLost => {
                ApiError::internal("internal worker failed to produce a response")
            }
        }
    }
}

impl ApiError {
    /// An internal (500) error with a free-form message. Used for failures that
    /// aren't a specific core error — e.g. a `run_local` worker thread that
    /// panicked, dropping its result before sending. Mapped to 500 via the
    /// catch-all in [`Self::status`].
    pub fn internal(message: impl Into<String>) -> Self {
        ApiError(CoreError::Io(std::io::Error::other(message.into())))
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ApiError {}

impl ApiError {
    /// HTTP status this error maps to.
    pub fn status(&self) -> StatusCode {
        match &self.0 {
            // Missing things → 404.
            CoreError::Session(SessionError::NotFound(_))
            | CoreError::Session(SessionError::ProjectNotFound(_))
            | CoreError::Session(SessionError::TmuxSessionNotFound(_))
            | CoreError::Session(SessionError::FileNotInDiff(_)) => StatusCode::NOT_FOUND,

            // Conflicting existing state → 409.
            CoreError::Session(SessionError::AlreadyExists(_))
            | CoreError::Session(SessionError::InvalidState(_))
            | CoreError::Session(SessionError::MaxSessionsReached(_)) => StatusCode::CONFLICT,

            // Bad client input → 400.
            CoreError::Session(SessionError::InvalidName { .. })
            | CoreError::Session(SessionError::InvalidProgram(_)) => StatusCode::BAD_REQUEST,

            // tmux missing entirely → 503 (the backing service is unavailable).
            CoreError::Tmux(TmuxError::NotInstalled)
            | CoreError::Tmux(TmuxError::ServerNotRunning) => StatusCode::SERVICE_UNAVAILABLE,

            // Everything else (git failures, IO, persistence, cascade, other
            // tmux/TUI/TTS/config errors) is an internal server error.
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Short machine-readable error category for the JSON body.
    fn kind(&self) -> &'static str {
        match &self.0 {
            CoreError::Session(_) => "session",
            CoreError::Tmux(_) => "tmux",
            CoreError::Git(_) => "git",
            CoreError::Config(_) => "config",
            CoreError::Io(_) => "io",
            CoreError::Tui(_) => "tui",
            CoreError::Tts(_) => "tts",
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        error_response(self.status(), self.kind(), self.0.to_string())
    }
}

/// Render the uniform `{"error": {"kind", "message"}}` body at `status`. Shared
/// by [`ApiError`] and the bearer-auth middleware so *every* error response —
/// including a 401 from the auth layer, which isn't backed by a [`CoreError`] —
/// carries the same envelope a client can parse.
pub fn error_response(status: StatusCode, kind: &str, message: impl Into<String>) -> Response {
    let body = Json(json!({
        "error": {
            "kind": kind,
            "message": message.into(),
        }
    }));
    (status, body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_commander_core::error::GitError;
    use claude_commander_core::session::SessionId;

    #[test]
    fn not_found_maps_to_404() {
        let err = ApiError(CoreError::Session(SessionError::NotFound(SessionId::new())));
        assert_eq!(err.status(), StatusCode::NOT_FOUND);
        assert_eq!(err.kind(), "session");
    }

    #[test]
    fn already_exists_maps_to_409() {
        let err = ApiError(CoreError::Session(SessionError::AlreadyExists("x".into())));
        assert_eq!(err.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn invalid_state_maps_to_409() {
        let err = ApiError(CoreError::Session(SessionError::InvalidState(
            SessionId::new(),
        )));
        assert_eq!(err.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn invalid_name_maps_to_400() {
        let err = ApiError(CoreError::Session(SessionError::InvalidName {
            name: "n".into(),
            reason: "r".into(),
        }));
        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn invalid_program_maps_to_400() {
        let err = ApiError(CoreError::Session(SessionError::InvalidProgram("p".into())));
        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn tmux_not_installed_maps_to_503() {
        let err = ApiError(CoreError::Tmux(TmuxError::NotInstalled));
        assert_eq!(err.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(err.kind(), "tmux");
    }

    #[test]
    fn git_error_maps_to_500() {
        let err = ApiError(CoreError::Git(GitError::OperationFailed("boom".into())));
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(err.kind(), "git");
    }
}
