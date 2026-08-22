//! Pure client-side decisions over a workspace snapshot.
//!
//! Every Claude Commander frontend — the ratatui TUI, the Flutter app, the
//! server's own callers — has to answer the same questions about a snapshot:
//! does this session match what the operator typed, and how do these rows rank?
//! Those answers are not wire contract and not host work, so they had nowhere to
//! live: they sat in [`claude_commander_core`], which is host-bound (gix,
//! pty-process, tmux, filesystem) and therefore unreachable from the Flutter
//! client's cdylib. The client re-implemented them in Dart instead, and the two
//! implementations drifted — `session_filter.dart` documented its own scorer as
//! "not byte-identical" to the Rust one.
//!
//! # What belongs here
//!
//! A decision a client makes *about* data it already holds. It renders nothing
//! and touches no I/O. If it needs a terminal, a file, a subprocess or a socket,
//! it belongs in `claude-commander-core` or in a frontend.
//!
//! Functions here take `&str`/scalars rather than DTOs wherever it is natural:
//! a core `WorktreeSession` and a protocol `SessionInfo` can then share one
//! implementation, and it crosses the Flutter client's FFI boundary without
//! marshalling a struct.
//!
//! # Why not `claude-commander-protocol`
//!
//! Protocol is what the server and its clients must **agree** on — change it and
//! the wire breaks. A view-model is how *one* client interprets a snapshot;
//! nothing has to agree, and it can churn freely. Keeping them apart is what
//! keeps "is this a wire change?" answerable.

pub mod query;

pub use query::{fuzzy_score, session_score};
