//! The seam every chat platform implements.
//!
//! A [`ChatBackend`] owns everything platform-specific: connecting, turning the
//! platform's events into [`IncomingMessage`]s (dropping bot posts, edits and
//! other noise, stripping the bot's own mention), the platform's Web API calls,
//! status markers, and rendering Markdown into the platform's own format.
//! Everything else — allowlist, dedup, prompt assembly, asking the headless
//! commander, notify routing — is shared and lives in [`crate::bridge`] and
//! [`crate::hub`].

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

/// Result of a chat platform call.
pub type ChatResult<T> = Result<T, ChatError>;

/// A chat platform failure. Opaque and string-only on purpose: it is logged,
/// and backends must never format token material into it.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ChatError(pub String);

impl ChatError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// A message addressed to the bot, normalised by its backend. Only messages a
/// human sent to the bot (a mention or a direct message) are delivered; the
/// backend has already dropped everything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMessage {
    /// The message's platform identity, for redelivery dedup. One message must
    /// map to one id even when the platform delivers it as several events
    /// (Slack sends a DM that mentions the bot as two).
    pub dedup_id: String,
    /// A direct message rather than a mention in a shared channel.
    pub is_direct: bool,
    /// The conversation it was posted in (a Slack channel id, …).
    pub channel: String,
    /// The message itself, the target of status markers.
    pub message_id: String,
    /// The thread replies go into: the message's thread, or the message itself
    /// when it starts one.
    pub thread: String,
    /// Whether the message is a reply inside an existing thread (so earlier
    /// messages are worth fetching for context).
    pub is_thread_reply: bool,
    /// The sender's platform user id.
    pub user: String,
    /// The request text, with the bot's own mention removed.
    pub text: String,
}

/// One earlier message in a thread, for context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadMessage {
    pub user: Option<String>,
    pub text: String,
    pub message_id: String,
}

/// Progress shown on the triggering message while a request is handled. Slack
/// renders these as reactions; a backend with nothing equivalent ignores them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Working,
    Done,
    Failed,
}

/// Where a running backend delivers the messages it receives.
#[derive(Clone)]
pub struct Inbox(mpsc::UnboundedSender<IncomingMessage>);

impl Inbox {
    pub(crate) fn channel() -> (Self, mpsc::UnboundedReceiver<IncomingMessage>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self(tx), rx)
    }

    /// Hand a message to the bridge. Returns `false` once the bridge has
    /// stopped, so the backend can stop too.
    pub fn deliver(&self, message: IncomingMessage) -> bool {
        self.0.send(message).is_ok()
    }
}

/// A chat platform.
#[async_trait]
pub trait ChatBackend: Send + Sync {
    /// Stable id (`slack`): the `backend` of a [`ChatOrigin`], the prefix of
    /// conversation keys, and the value of `[chat] default_notify`.
    ///
    /// [`ChatOrigin`]: claude_commander_protocol::session::ChatOrigin
    fn id(&self) -> &'static str;

    /// Human name (`Slack`), used in prompts and logs.
    fn display_name(&self) -> &'static str;

    /// Platform user ids allowed to use the bridge. The first is also who an
    /// originless `notify` is sent to when this backend is the default.
    fn allowed_users(&self) -> &[String];

    /// Connect and deliver incoming messages to `inbox` until the connection
    /// ends for good. Reconnecting after transient failures is the backend's
    /// job; returning ends this backend's bridge (sending keeps working).
    async fn run(self: Arc<Self>, inbox: Inbox) -> ChatResult<()>;

    /// Post `text` (already [rendered](Self::render)) into a thread.
    async fn post_reply(&self, channel: &str, thread: &str, text: &str) -> ChatResult<()>;

    /// Send `text` (already rendered) to a user directly.
    async fn post_direct(&self, user: &str, text: &str) -> ChatResult<()>;

    /// Earlier messages in a thread, oldest first. Default: none.
    async fn fetch_thread(&self, _channel: &str, _thread: &str) -> ChatResult<Vec<ThreadMessage>> {
        Ok(Vec::new())
    }

    /// A link to a message. Default: the platform has none.
    async fn permalink(&self, _channel: &str, _message_id: &str) -> ChatResult<Option<String>> {
        Ok(None)
    }

    /// Show `status` on a message. Default: no-op.
    async fn set_status(
        &self,
        _channel: &str,
        _message_id: &str,
        _status: Status,
    ) -> ChatResult<()> {
        Ok(())
    }

    /// Convert standard Markdown (what the agents write) into the platform's
    /// own format. Default: the platform renders Markdown as-is.
    fn render(&self, markdown: &str) -> String {
        markdown.to_string()
    }

    /// How to refer to a user in a prompt so the agent can tell people apart.
    fn mention(&self, user: &str) -> String {
        user.to_string()
    }
}
