//! Backend connection health.
//!
//! A plain data enum shared across the network boundary: the transport client
//! ([`claude-commander-client`]) drives it from its poll loop, `claude-commander-core`
//! re-exports it as `backend::ConnectionState` (so every TUI call site is
//! unchanged), and a remote client renders it in the server header.

use serde::{Deserialize, Serialize};

/// A backend's connection health, rendered in its server header. The local
/// backend is always [`Connected`](ConnectionState::Connected).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    /// The initial handshake / first snapshot hasn't landed yet.
    Connecting,
    /// Healthy: snapshots are current.
    Connected,
    /// Reachable but stale or erroring; `reason` is a short user-facing note.
    Degraded { reason: String },
}
