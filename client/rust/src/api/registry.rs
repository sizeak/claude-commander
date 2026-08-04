//! Shared runtime + the opaque per-server handle registry.
//!
//! Every route the cdylib exposes to Dart runs against a
//! [`claude_commander_client::RemoteClient`]. Rather than thread a base URL +
//! token through every call (and re-validate/re-build a client each time), Dart
//! calls [`connect_server`] once to get back an opaque **handle** string, then
//! passes that handle to every subsequent route/terminal/feed call. The handle
//! is the seam a future multi-server client grows into: one registry entry per
//! connected server, each with its own [`RemoteClient`] + background [`Poller`].
//!
//! The `RemoteClient`'s methods are all `async`, but the frb functions stay
//! **synchronous** (frb already runs each on a worker thread off the Dart
//! isolate). So every route resolves its client from the registry and drives the
//! async call to completion on a process-wide multi-thread [`runtime`]. Because
//! the frb worker thread is not itself a runtime worker, `block_on` is safe
//! here — and the `server_flows` integration tests call these fns from a plain
//! test thread (never from inside their own `block_on`) for the same reason.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use anyhow::{Context, Result};
use claude_commander_client::{
    ClientError, PollConfig, Poller, RemoteClient, RemoteServerSpec, SecretString, spawn_poller,
};
use claude_commander_protocol::connection::ConnectionState;
use claude_commander_protocol::session::{ProjectId, SessionId};
use tokio::runtime::Runtime;
use tokio::sync::watch;
use uuid::Uuid;

/// One connected server: its transport client plus the background poller whose
/// change-feed / connection-health watches drive the live Dart feeds. Dropping
/// the entry drops the `Poller`, which aborts its poll task.
struct ServerEntry {
    client: Arc<RemoteClient>,
    poller: Poller,
}

/// Process-wide multi-thread runtime that owns every server's poll task and
/// attach pump, and drives each synchronous frb route call to completion. Two
/// worker threads are plenty: the work is IO-bound (HTTP + a WebSocket pump).
pub(crate) fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to build tokio runtime")
    })
}

/// Drive a future to completion on the shared runtime. Safe from an frb worker
/// thread (not a runtime worker); never call from inside another `block_on`.
pub(crate) fn block_on<F: Future>(fut: F) -> F::Output {
    runtime().block_on(fut)
}

type Servers = HashMap<String, ServerEntry>;

/// Connected servers keyed by the opaque handle returned from [`connect_server`].
fn servers() -> &'static Mutex<Servers> {
    static SERVERS: OnceLock<Mutex<Servers>> = OnceLock::new();
    SERVERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Lock the registry, recovering from a poisoned mutex rather than panicking
/// across the FFI boundary — the critical sections are tiny and panic-free, so a
/// poison would only ever follow an unrelated panic.
fn lock_servers() -> MutexGuard<'static, Servers> {
    servers().lock().unwrap_or_else(|e| e.into_inner())
}

/// Resolve a connected server's transport client by handle, or a friendly error
/// if the handle is unknown (never connected, or already disconnected).
pub(crate) fn with_client(handle: &str) -> Result<Arc<RemoteClient>> {
    lock_servers()
        .get(handle)
        .map(|entry| entry.client.clone())
        .context("not connected (call connectServer first)")
}

/// A fresh clone of a server's change-feed generation watch (bumped by the
/// poller whenever observable state moves), or an error if the handle is unknown.
pub(crate) fn generation_watch(handle: &str) -> Result<watch::Receiver<u64>> {
    lock_servers()
        .get(handle)
        .map(|entry| entry.poller.generation_watch())
        .context("not connected (call connectServer first)")
}

/// A fresh clone of a server's connection-health watch, or an error if the
/// handle is unknown.
pub(crate) fn connection_watch(handle: &str) -> Result<watch::Receiver<ConnectionState>> {
    lock_servers()
        .get(handle)
        .map(|entry| entry.poller.connection_watch())
        .context("not connected (call connectServer first)")
}

/// Map a [`ClientError`] to an `anyhow::Error` with a user-facing message,
/// preserving the historical auth wording the connect screen expects.
pub(crate) fn map_client_err(err: ClientError) -> anyhow::Error {
    match err {
        ClientError::Auth => anyhow::anyhow!("authentication failed (check your token)"),
        other => anyhow::anyhow!(other.to_string()),
    }
}

/// Drive a `RemoteClient` call to completion and map its [`ClientError`] to a
/// friendly `anyhow::Error`. The route fn owns the `Arc<RemoteClient>` the
/// future borrows, so it stays alive for the whole call. The single funnel every
/// route fn goes through.
pub(crate) fn call<T, Fut>(fut: Fut) -> Result<T>
where
    Fut: Future<Output = std::result::Result<T, ClientError>>,
{
    block_on(fut).map_err(map_client_err)
}

/// Parse a full-UUID session id string (what `SessionInfo.id` carries) into a
/// [`SessionId`]. The loose detail *query* (branch/title/prefix) is NOT parsed —
/// it stays a raw string.
pub(crate) fn parse_session_id(id: &str) -> Result<SessionId> {
    Uuid::parse_str(id)
        .map(SessionId::from_uuid)
        .with_context(|| format!("invalid session id {id:?}"))
}

/// Parse a full-UUID project id string into a [`ProjectId`].
pub(crate) fn parse_project_id(id: &str) -> Result<ProjectId> {
    Uuid::parse_str(id)
        .map(ProjectId::from_uuid)
        .with_context(|| format!("invalid project id {id:?}"))
}

/// Connect to a server: build + validate a [`RemoteClient`], spawn its poller,
/// and register both under a freshly generated opaque handle (returned). An
/// empty `token` means "auth disabled" (loopback dev) — no bearer is sent.
///
/// The URL is validated here (bad scheme/host fails now); *reachability*
/// surfaces later through the connection feed, not from this call.
pub fn connect_server(base_url: String, token: Option<String>) -> Result<String> {
    let name = if base_url.is_empty() {
        "server".to_string()
    } else {
        base_url.clone()
    };
    let token = token
        .filter(|t| !t.is_empty())
        .map(SecretString::from);
    let spec = RemoteServerSpec {
        name,
        base_url,
        token,
    };
    let client = Arc::new(RemoteClient::new(spec).map_err(map_client_err)?);
    // The poller task spawns onto the shared runtime.
    let poller = {
        let _guard = runtime().enter();
        spawn_poller(client.clone(), PollConfig::default())
    };
    let handle = Uuid::new_v4().to_string();
    lock_servers().insert(handle.clone(), ServerEntry { client, poller });
    Ok(handle)
}

/// Disconnect a server: tear down any in-flight terminal attaches it owns (an
/// attach holds its own client clone, so it outlives the registry entry
/// otherwise), then drop its registry entry, which drops the [`Poller`] and
/// aborts its poll task. A no-op for an unknown handle.
pub fn disconnect_server(handle: String) {
    crate::api::terminal::detach_all_for_handle(&handle);
    lock_servers().remove(&handle);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_rejects_bad_url_and_disconnect_is_idempotent() {
        // A malformed base URL fails at construction (RemoteClient::new validates).
        assert!(connect_server("not a url".to_string(), None).is_err());

        // A valid URL connects (no reachability check here) and yields a handle
        // that resolves; disconnecting removes it and is safe to repeat.
        let handle = connect_server("http://127.0.0.1:9/".to_string(), None)
            .expect("a well-formed url should connect");
        assert!(with_client(&handle).is_ok());
        disconnect_server(handle.clone());
        assert!(with_client(&handle).is_err(), "handle must be gone");
        disconnect_server(handle); // idempotent
    }

    #[test]
    fn with_client_unknown_handle_errors() {
        assert!(with_client("no-such-handle").is_err());
    }

    #[test]
    fn parse_ids_round_trip_and_reject_garbage() {
        let sid = SessionId::new();
        assert_eq!(parse_session_id(&sid.as_uuid().to_string()).unwrap(), sid);
        assert!(parse_session_id("not-a-uuid").is_err());

        let pid = ProjectId::new();
        assert_eq!(parse_project_id(&pid.as_uuid().to_string()).unwrap(), pid);
        assert!(parse_project_id("nope").is_err());
    }

    #[test]
    fn map_client_err_preserves_auth_wording() {
        let msg = map_client_err(ClientError::Auth).to_string();
        assert!(msg.contains("authentication failed"), "got {msg}");
        assert!(msg.contains("check your token"), "got {msg}");
    }
}
