//! HTTP handlers, one module per resource group. Each handler is a thin wrapper
//! over a `CommanderService` method, mapping the result onto an HTTP response
//! (`Json`/status code) and errors onto [`crate::error::ApiError`].

pub mod blobs;
pub mod cascade;
pub mod commander;
pub mod config;
pub mod health;
pub mod paste;
pub mod projects;
pub mod review;
pub mod sessions;
pub mod slack;
pub mod workspace;

#[cfg(test)]
pub(crate) mod test_support;

use claude_commander_core::error::SessionError;
use claude_commander_core::session::SessionId;
use uuid::Uuid;

use crate::error::ApiError;

/// The non-`Send` core-future bridge now lives in core
/// ([`claude_commander_core::backend::run_local`]) so both the server handlers
/// and the in-process `LocalBackend` share one copy. Re-exported here so the
/// existing `super::run_local` / `crate::handlers::run_local` import paths keep
/// working. Its error type is generic; [`ApiError`] converts from
/// `RunLocalError<CoreError>` (see `crate::error`), so handler call sites are
/// unchanged: `run_local(...).await?` maps an inner error via the usual
/// [`ApiError`] conversion and a lost worker thread to a 500.
pub use claude_commander_core::backend::run_local;

/// Parse a `{id}` path param into a [`SessionId`], mapping a malformed UUID to a
/// 400 (`InvalidName`) rather than a 404 — the client sent a syntactically bad
/// id, not a well-formed id that happens not to exist.
pub fn parse_session_id(raw: &str) -> Result<SessionId, ApiError> {
    Uuid::parse_str(raw).map(SessionId::from_uuid).map_err(|e| {
        ApiError(
            SessionError::InvalidName {
                name: raw.to_string(),
                reason: format!("not a valid session id: {e}"),
            }
            .into(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use claude_commander_core::Error as CoreError;
    use claude_commander_core::error::SessionError;

    /// `run_local`'s behaviour (ok / inner error / worker panic) is unit-tested
    /// in core; here we verify the server's glue: a closure returning a core
    /// `Error` maps to the right HTTP status through `run_local(...).await?`, and
    /// a panicking worker still surfaces as a 500 rather than a dropped
    /// connection.
    #[tokio::test]
    async fn run_local_panic_yields_500() {
        async fn handler() -> Result<(), ApiError> {
            run_local(|| async {
                panic!("boom inside worker");
                #[allow(unreachable_code)]
                Ok::<(), CoreError>(())
            })
            .await?;
            Ok(())
        }
        let err = handler()
            .await
            .expect_err("a panicking worker should yield Err, not Ok");
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// The happy path passes the closure's `Ok` through unchanged.
    #[tokio::test]
    async fn run_local_ok_passes_through() {
        async fn handler() -> Result<u32, ApiError> {
            Ok(run_local(|| async { Ok::<u32, CoreError>(7) }).await?)
        }
        assert_eq!(handler().await.unwrap(), 7);
    }

    /// A core error returned by the worker keeps its HTTP-status mapping when it
    /// flows through `run_local` (here: `NotFound` → 404).
    #[tokio::test]
    async fn run_local_inner_error_preserves_status() {
        async fn handler() -> Result<(), ApiError> {
            run_local(|| async {
                Err::<(), CoreError>(CoreError::Session(SessionError::NotFound(
                    claude_commander_core::session::SessionId::new(),
                )))
            })
            .await?;
            Ok(())
        }
        let err = handler().await.expect_err("expected NotFound error");
        assert_eq!(err.status(), StatusCode::NOT_FOUND);
    }
}
