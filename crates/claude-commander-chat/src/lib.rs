//! Platform-neutral chat bridge for Claude Commander.
//!
//! People talk to the headless commander from a chat platform — mention the
//! bot or message it directly — and worker sessions report back with
//! `claude-commander notify`. This crate holds everything about that which is
//! the same on every platform:
//!
//! - [`ChatBackend`] — the trait each platform implements (connecting,
//!   normalising its events, its Web API calls, rendering Markdown). Platform
//!   crates (`claude-commander-chat-slack`, …) implement it, each behind a
//!   server cargo feature so a build only carries the SDKs it uses.
//! - [`bridge`] — allowlist, redelivery dedup, prompt assembly, asking the
//!   headless commander, posting the reply.
//! - [`ChatHub`] — runs every enabled backend at once and routes `notify`
//!   back to the backend and thread a session came from.
//!
//! No platform SDK is a dependency here.

pub mod backend;
pub mod bridge;
pub mod hub;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use backend::{
    ChatBackend, ChatError, ChatResult, Inbox, IncomingMessage, Status, ThreadMessage,
};
pub use bridge::{Asker, CommanderAsker};
pub use hub::{ChatHub, Delivery, NotifyError};
