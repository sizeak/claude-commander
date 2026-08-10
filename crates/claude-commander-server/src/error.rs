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
use claude_commander_core::error::{GitError, SessionError, TmuxError};
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
            | CoreError::Session(SessionError::InvalidProgram(_))
            | CoreError::Session(SessionError::InvalidImage(_))
            // A refused clone source/destination name is the client's mistake, not
            // a git failure — which is exactly why core gives it its own variant.
            // Its message is redacted at construction (`clone_source_rejected`),
            // so rendering it here cannot echo a credential back.
            | CoreError::Git(GitError::CloneSourceRejected(_)) => StatusCode::BAD_REQUEST,

            // A missing backing tool → 503 (the service is unavailable, and the
            // user can fix it by installing the thing). `gh` joins tmux here for
            // the same reason core carved `GhUnavailable` out of `OperationFailed`:
            // "install the GitHub CLI" is a distinct, actionable state, and
            // flattening it into a 500 would deny every remote frontend the
            // distinction core went to the trouble of making.
            CoreError::Tmux(TmuxError::NotInstalled)
            | CoreError::Tmux(TmuxError::ServerNotRunning)
            | CoreError::Git(GitError::GhUnavailable) => StatusCode::SERVICE_UNAVAILABLE,

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

    /// A refused clone source is the *client's* mistake, so it must not share the
    /// 500 that every other git failure gets. Asserted alongside `OperationFailed`
    /// because the pair is the whole point: the two are the same shape (a `GitError`
    /// carrying a string) and only the variant separates a bad request from a
    /// broken server.
    #[test]
    fn rejected_clone_source_maps_to_400_unlike_other_git_errors() {
        let rejected = ApiError(CoreError::Git(GitError::CloneSourceRejected(
            "'o' is not an owner/name repository slug".into(),
        )));
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        assert_eq!(rejected.kind(), "git");
        // The message is rendered as-is — no prefix wrapping the user's reason.
        assert_eq!(
            rejected.to_string(),
            "Git error: 'o' is not an owner/name repository slug"
        );
        assert_eq!(
            ApiError(CoreError::Git(GitError::OperationFailed("boom".into()))).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    /// A missing `gh` is a 503, not a 500: core carved `GhUnavailable` out of
    /// `OperationFailed` so a frontend could render "install the GitHub CLI" as its
    /// own state, and flattening it here would throw that distinction away for
    /// every remote client. Same treatment as a missing tmux.
    #[test]
    fn missing_gh_maps_to_503_like_missing_tmux() {
        let err = ApiError(CoreError::Git(GitError::GhUnavailable));
        assert_eq!(err.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(err.kind(), "git");
        // A clone that timed out is still a genuine failure → 500.
        assert_eq!(
            ApiError(CoreError::Git(GitError::CloneTimedOut { secs: 600 })).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
