//! HTTP handlers, one module per resource group. Each handler is a thin wrapper
//! over a `CommanderService` method, mapping the result onto an HTTP response
//! (`Json`/status code) and errors onto [`crate::error::ApiError`].

pub mod blobs;
pub mod cascade;
pub mod config;
pub mod health;
pub mod projects;
pub mod review;
pub mod sessions;
pub mod workspace;

#[cfg(test)]
pub(crate) mod test_support;

use std::future::Future;

use claude_commander_core::error::SessionError;
use claude_commander_core::session::SessionId;
use uuid::Uuid;

use crate::error::ApiError;

/// Run a **non-`Send`** core future to completion off the axum worker.
///
/// The session-mutation and project methods (`create_session`, `kill_session`,
/// `restart_session`, `delete_session`, `add_project`, `ensure_project`,
/// `scan_directory`) build a `gix::Repository` — which holds an `Rc` — and keep
/// it live across an `.await`, so the resulting future is `!Send`. (The
/// review/blob/config read paths use only the git CLI, so they stay `Send` and
/// are `.await`ed directly.) axum requires handler futures
/// to be `Send` (it runs on the multi-thread runtime), so such a future cannot
/// be `.await`ed directly inside a handler.
///
/// The fix keeps the `!Send` value from ever crossing a thread boundary: the
/// `Send + 'static` *closure* is moved to a dedicated thread running a
/// current-thread Tokio runtime, which builds and drives the `!Send` future
/// entirely on that thread; only the `Send` output is returned. This is a
/// thin, correct bridge — it does not touch core (the dependency direction is
/// server → core), at the cost of one short-lived thread per mutating request
/// (mutations are infrequent, so this is acceptable for v1).
///
/// NOTE for integration: a process-wide single-threaded executor pool (e.g.
/// `tokio_util::task::LocalPoolHandle`, held in `AppState`) would avoid the
/// per-call thread spawn. Left as a follow-up to avoid adding a dependency and
/// touching the shared `AppState` mid-fan-out.
///
/// The closure yields a `Result<T, E>` (any `E: Into<ApiError>`, e.g. the core
/// `Error`); the inner error is converted to an [`ApiError`]. If the worker
/// thread panics it drops the oneshot sender and the receive resolves to `Err`;
/// rather than `expect`-ing (which would drop the connection with no response),
/// that case is mapped to a 500 [`ApiError`] so the client gets a proper error.
/// The router's `CatchPanicLayer` is the second line of defence.
pub async fn run_local<F, Fut, T, E>(f: F) -> Result<T, ApiError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, E>>,
    T: Send + 'static,
    E: Into<ApiError> + Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread runtime for non-Send core call");
        let out = rt.block_on(f());
        // Sender is dropped without sending only if `block_on` panicked
        // (handled below) or the request was cancelled; ignore the send result.
        let _ = tx.send(out);
    });
    match rx.await {
        Ok(result) => result.map_err(Into::into),
        // The worker thread panicked (or was otherwise lost) before sending.
        Err(_) => Err(ApiError::internal(
            "internal worker failed to produce a response",
        )),
    }
}

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

    /// A panic inside the `run_local` worker must surface as a 500 `ApiError`,
    /// not a dropped connection (the old `expect` would panic the handler).
    #[tokio::test]
    async fn run_local_panic_yields_500() {
        let result = run_local(|| async {
            panic!("boom inside worker");
            #[allow(unreachable_code)]
            Ok::<(), ApiError>(())
        })
        .await;
        let err = result.expect_err("a panicking worker should yield Err, not Ok");
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// The happy path passes the closure's `Ok` through unchanged.
    #[tokio::test]
    async fn run_local_ok_passes_through() {
        let result = run_local(|| async { Ok::<u32, ApiError>(7) }).await;
        assert_eq!(result.unwrap(), 7);
    }
}
