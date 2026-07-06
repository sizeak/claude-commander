//! Run a **non-`Send`** future to completion off the caller's runtime.
//!
//! Several core futures build a `gix::Repository` — which holds an `Rc` — and
//! keep it live across an `.await`, so the resulting future is `!Send`. A
//! multi-thread runtime (axum's, or a `tokio::spawn`ed backend task) requires
//! the futures it drives to be `Send`, so such a future cannot be `.await`ed
//! directly there.
//!
//! [`run_local`] keeps the `!Send` value from ever crossing a thread boundary:
//! the `Send + 'static` *closure* is moved to a dedicated thread running a
//! current-thread Tokio runtime, which builds and drives the `!Send` future
//! entirely on that thread; only the `Send` output is sent back. This is a thin
//! bridge at the cost of one short-lived thread per call (the callers — session
//! mutations, project scans — are infrequent, so this is acceptable).
//!
//! This lived in the server crate originally; it moved here so both the server
//! handlers and the in-process [`LocalBackend`](super::local::LocalBackend) can
//! share one copy of the bridge. The bridge is generic over the future's error
//! type ([`RunLocalError`]) so neither the server's `ApiError` nor core's
//! [`BackendError`](super::BackendError) leaks into it.

use std::future::Future;

/// The outcome of a [`run_local`] call that did not produce the inner future's
/// `Ok` value: either the inner future returned an error, or the worker thread
/// was lost (panicked, or dropped its result) before sending one.
///
/// Callers map this onto their own error type — the server to a 500 `ApiError`,
/// the local backend to [`BackendError`](super::BackendError) — via a `From`
/// impl, so `run_local(...).await?` stays ergonomic at the call site.
#[derive(Debug)]
pub enum RunLocalError<E> {
    /// The inner future completed with this error.
    Inner(E),
    /// The worker thread panicked or was otherwise lost before sending a
    /// result. Treated as an internal failure by every caller.
    WorkerLost,
}

impl<E: std::fmt::Display> std::fmt::Display for RunLocalError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunLocalError::Inner(e) => write!(f, "{e}"),
            RunLocalError::WorkerLost => {
                write!(f, "internal worker failed to produce a response")
            }
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for RunLocalError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RunLocalError::Inner(e) => Some(e),
            RunLocalError::WorkerLost => None,
        }
    }
}

/// Drive a **non-`Send`** future `f()` to completion on a dedicated
/// current-thread runtime and return its output.
///
/// The closure yields a `Result<T, E>`; a returned error is wrapped in
/// [`RunLocalError::Inner`]. If the worker thread panics it drops the oneshot
/// sender, and the receive resolves to [`RunLocalError::WorkerLost`] rather than
/// panicking the caller — so a panicking mutation surfaces as a clean error
/// (the server's `CatchPanicLayer` is the second line of defence).
pub async fn run_local<F, Fut, T, E>(f: F) -> Result<T, RunLocalError<E>>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, E>>,
    T: Send + 'static,
    E: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread runtime for non-Send core call");
        let out = rt.block_on(f());
        // Sender is dropped without sending only if `block_on` panicked
        // (handled below) or the receiver was dropped; ignore the send result.
        let _ = tx.send(out);
    });
    match rx.await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(RunLocalError::Inner(e)),
        // The worker thread panicked (or was otherwise lost) before sending.
        Err(_) => Err(RunLocalError::WorkerLost),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The happy path passes the closure's `Ok` through unchanged.
    #[tokio::test]
    async fn run_local_ok_passes_through() {
        let result: Result<u32, RunLocalError<std::io::Error>> =
            run_local(|| async { Ok::<u32, std::io::Error>(7) }).await;
        assert_eq!(result.unwrap(), 7);
    }

    /// An inner error is wrapped in `Inner`, preserving the original error.
    #[tokio::test]
    async fn run_local_inner_error_is_wrapped() {
        let result: Result<(), RunLocalError<String>> =
            run_local(|| async { Err::<(), String>("boom".to_string()) }).await;
        match result {
            Err(RunLocalError::Inner(e)) => assert_eq!(e, "boom"),
            other => panic!("expected Inner, got {other:?}"),
        }
    }

    /// A panic inside the worker must surface as `WorkerLost`, not a dropped
    /// receiver / panicked caller.
    #[tokio::test]
    async fn run_local_worker_panic_yields_worker_lost() {
        let result: Result<(), RunLocalError<String>> = run_local(|| async {
            panic!("boom inside worker");
            #[allow(unreachable_code)]
            Ok::<(), String>(())
        })
        .await;
        assert!(matches!(result, Err(RunLocalError::WorkerLost)));
    }

    /// A `!Send` value held across an `.await` is fine — it never leaves the
    /// worker thread. (`Rc` is the canonical `!Send` stand-in for `gix`.)
    #[tokio::test]
    async fn run_local_drives_non_send_future() {
        let out: Result<i32, RunLocalError<std::io::Error>> = run_local(|| async {
            let rc = std::rc::Rc::new(41);
            tokio::task::yield_now().await;
            Ok::<i32, std::io::Error>(*rc + 1)
        })
        .await;
        assert_eq!(out.unwrap(), 42);
    }
}
