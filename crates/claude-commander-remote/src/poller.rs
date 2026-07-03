//! Background poller: the [`RemoteBackend`](crate::RemoteBackend)'s change-feed
//! and connection state machine.
//!
//! A remote server has no push channel for state changes (that would be a second
//! WebSocket; Phase F is busy enough with attach), so the client polls. Every
//! [`interval`](PollConfig::interval) the poller fetches the workspace snapshot
//! and the agent-state snapshot, content-hashes both, and — when the hash moved
//! (or the connection just recovered) — bumps a [`watch`] generation counter.
//! The TUI's per-backend change-feed task waits on that counter and re-fetches,
//! exactly as it does for the local backend's store generation.
//!
//! Alongside the generation counter the poller drives a
//! [`ConnectionState`](claude_commander_core::backend::ConnectionState) watch:
//! `Connecting` until the first successful poll, `Connected` while polls
//! succeed, and `Degraded { reason }` on failure, with exponential backoff
//! ([`backoff_delay`]) between retries so a downed server isn't hammered.
//!
//! The poll task holds no strong reference back to the `RemoteBackend`, so it
//! forms no cycle; [`Poller`]'s `Drop` aborts it when the backend goes away.

use std::sync::Arc;
use std::time::Duration;

use claude_commander_core::backend::ConnectionState;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::backend::RemoteInner;
use crate::backoff::{BackoffConfig, backoff_delay};

/// Cadence + backoff for the background poller. Fields are public so Phase G can
/// wire them from config; the defaults match the local backend's agent-state
/// poll cadence.
#[derive(Clone, Copy, Debug)]
pub struct PollConfig {
    /// Healthy poll cadence. Each tick fetches both the workspace and
    /// agent-state snapshots.
    pub interval: Duration,
    /// Reconnect backoff applied while polls are failing.
    pub backoff: BackoffConfig,
}

impl Default for PollConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(2),
            backoff: BackoffConfig::default(),
        }
    }
}

/// A watch on a backend's [`ConnectionState`], mirroring the shape of core's
/// `BackendChangeFeed` so the later TUI wiring can render health reactively.
#[derive(Clone)]
pub struct ConnectionFeed {
    rx: watch::Receiver<ConnectionState>,
}

impl ConnectionFeed {
    pub(crate) fn new(rx: watch::Receiver<ConnectionState>) -> Self {
        Self { rx }
    }

    /// The current connection state (cheap; no `.await`).
    pub fn current(&self) -> ConnectionState {
        self.rx.borrow().clone()
    }

    /// Wait until the connection state changes. Returns `false` once the poller
    /// is gone (the backend was dropped), so a render task can exit cleanly.
    pub async fn changed(&mut self) -> bool {
        self.rx.changed().await.is_ok()
    }
}

/// Owns the spawned poll task and the receiver ends of its two watches. Aborting
/// the task on `Drop` prevents an orphaned poll loop from outliving the backend.
pub(crate) struct Poller {
    pub(crate) generation: watch::Receiver<u64>,
    pub(crate) connection: watch::Receiver<ConnectionState>,
    handle: JoinHandle<()>,
}

impl Drop for Poller {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Spawn the poll loop for `inner` and return the handle owning it.
pub(crate) fn spawn(inner: Arc<RemoteInner>, config: PollConfig) -> Poller {
    let (gen_tx, gen_rx) = watch::channel(0u64);
    let (conn_tx, conn_rx) = watch::channel(ConnectionState::Connecting);
    let handle = tokio::spawn(run(inner, config, gen_tx, conn_tx));
    Poller {
        generation: gen_rx,
        connection: conn_rx,
        handle,
    }
}

/// The poll loop. Fetches + hashes on each tick, bumps `gen_tx` on a real change
/// or a recovery, and moves `conn_tx` through the connection state machine with
/// exponential backoff on failure.
async fn run(
    inner: Arc<RemoteInner>,
    config: PollConfig,
    gen_tx: watch::Sender<u64>,
    conn_tx: watch::Sender<ConnectionState>,
) {
    let mut last_hash: Option<u64> = None;
    let mut consecutive_failures: u32 = 0;

    loop {
        match inner.poll_hashes().await {
            Ok(hash) => {
                let content_changed = last_hash != Some(hash);
                let recovered = !matches!(&*conn_tx.borrow(), ConnectionState::Connected);
                last_hash = Some(hash);
                consecutive_failures = 0;

                if recovered {
                    // Only send on a real transition so idle polls don't spam
                    // the connection watch.
                    let _ = conn_tx.send(ConnectionState::Connected);
                }
                // Bump the change feed on the first successful poll (recovered
                // from `Connecting`), on any later recovery, or whenever the
                // observable state actually moved — so the TUI re-fetches.
                if content_changed || recovered {
                    gen_tx.send_modify(|g| *g = g.wrapping_add(1));
                }

                tokio::time::sleep(config.interval).await;
            }
            Err(err) => {
                consecutive_failures += 1;
                let reason = err.to_string();
                tracing::debug!(server = %inner.name(), %reason, "remote poll failed");
                mark_degraded(&conn_tx, reason);
                tokio::time::sleep(backoff_delay(&config.backoff, consecutive_failures)).await;
            }
        }
    }
}

/// Move the connection watch into `Degraded` on a failed poll, notifying
/// watchers only on the *transition* into degraded — mirror the Connected
/// side's guard so a run of failed polls doesn't spam the connection watch.
/// An already-degraded state stays put (reason and all). Returns whether a
/// notification was sent.
fn mark_degraded(conn_tx: &watch::Sender<ConnectionState>, reason: String) -> bool {
    conn_tx.send_if_modified(|state| {
        if matches!(state, ConnectionState::Degraded { .. }) {
            false
        } else {
            *state = ConnectionState::Degraded { reason };
            true
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consecutive_failures_notify_connection_watch_once() {
        let (tx, mut rx) = watch::channel(ConnectionState::Connecting);

        // First failure transitions Connecting → Degraded and notifies.
        assert!(mark_degraded(&tx, "boom".to_string()));
        assert!(rx.has_changed().unwrap());
        rx.borrow_and_update();

        // Second failure is already degraded — no further notification.
        assert!(!mark_degraded(&tx, "boom again".to_string()));
        assert!(!rx.has_changed().unwrap());
    }

    #[test]
    fn recovery_after_degraded_then_new_failure_notifies_again() {
        let (tx, mut rx) = watch::channel(ConnectionState::Connecting);
        assert!(mark_degraded(&tx, "down".to_string()));
        rx.borrow_and_update();
        // Recover, then fail again — a fresh transition notifies.
        let _ = tx.send(ConnectionState::Connected);
        rx.borrow_and_update();
        assert!(mark_degraded(&tx, "down again".to_string()));
        assert!(rx.has_changed().unwrap());
    }
}
