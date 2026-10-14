//! In-memory test doubles: a [`FakeBackend`] that records every platform call,
//! and a [`FakeAsker`] with a canned answer. Compiled for this crate's tests and,
//! behind the `test-support` feature, for other crates' (the server's notify
//! route tests).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::backend::{ChatBackend, ChatError, ChatResult, Inbox, Status, ThreadMessage};
use crate::bridge::Asker;

/// One recorded platform call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Status(String, Status),
    Reply {
        channel: String,
        thread: String,
        text: String,
    },
    Direct {
        user: String,
        text: String,
    },
    FetchThread(String),
}

/// A chat backend that records calls instead of talking to a platform. Its
/// `render` wraps text as `rendered(…)` and its `mention` prefixes `@`, so a
/// test can see that the bridge went through both.
pub struct FakeBackend {
    id: &'static str,
    allowed: Vec<String>,
    thread: Vec<ThreadMessage>,
    fail_posts: bool,
    slow_status: Option<(String, std::time::Duration)>,
    calls: Mutex<Vec<Call>>,
    inbox: Mutex<Option<Inbox>>,
}

impl FakeBackend {
    /// A backend with id `id` that allowlists `U1`.
    pub fn new(id: &'static str) -> Self {
        Self {
            id,
            allowed: vec!["U1".to_string()],
            thread: Vec::new(),
            fail_posts: false,
            slow_status: None,
            calls: Mutex::new(Vec::new()),
            inbox: Mutex::new(None),
        }
    }

    pub fn with_allowed(mut self, allowed: Vec<String>) -> Self {
        self.allowed = allowed;
        self
    }

    pub fn with_thread(mut self, thread: Vec<ThreadMessage>) -> Self {
        self.thread = thread;
        self
    }

    /// Make every post (reply or direct) fail.
    pub fn failing_posts(mut self) -> Self {
        self.fail_posts = true;
        self
    }

    /// Delay every status update on `message_id` by `delay`, to make that
    /// message's handling slow before it reaches the asker.
    pub fn with_slow_status(mut self, message_id: &str, delay: std::time::Duration) -> Self {
        self.slow_status = Some((message_id.to_string(), delay));
        self
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    /// The inbox handed to [`ChatBackend::run`], once the hub has started it.
    pub fn inbox(&self) -> Option<Inbox> {
        self.inbox.lock().unwrap().clone()
    }

    fn record(&self, call: Call) {
        self.calls.lock().unwrap().push(call);
    }

    fn post_result(&self) -> ChatResult<()> {
        if self.fail_posts {
            Err(ChatError::new("post failed"))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl ChatBackend for FakeBackend {
    fn id(&self) -> &'static str {
        self.id
    }

    fn display_name(&self) -> &'static str {
        "Fake"
    }

    fn allowed_users(&self) -> &[String] {
        &self.allowed
    }

    /// Park the inbox for the test to deliver through, then stay "connected".
    async fn run(self: Arc<Self>, inbox: Inbox) -> ChatResult<()> {
        *self.inbox.lock().unwrap() = Some(inbox);
        std::future::pending::<()>().await;
        Ok(())
    }

    async fn post_reply(&self, channel: &str, thread: &str, text: &str) -> ChatResult<()> {
        self.record(Call::Reply {
            channel: channel.into(),
            thread: thread.into(),
            text: text.into(),
        });
        self.post_result()
    }

    async fn post_direct(&self, user: &str, text: &str) -> ChatResult<()> {
        self.record(Call::Direct {
            user: user.into(),
            text: text.into(),
        });
        self.post_result()
    }

    async fn fetch_thread(&self, _channel: &str, thread: &str) -> ChatResult<Vec<ThreadMessage>> {
        self.record(Call::FetchThread(thread.into()));
        Ok(self.thread.clone())
    }

    async fn permalink(&self, _channel: &str, _message_id: &str) -> ChatResult<Option<String>> {
        Ok(Some("https://fake/link".into()))
    }

    async fn set_status(&self, _channel: &str, message_id: &str, status: Status) -> ChatResult<()> {
        if let Some((slow, delay)) = &self.slow_status
            && slow == message_id
        {
            tokio::time::sleep(*delay).await;
        }
        self.record(Call::Status(message_id.into(), status));
        Ok(())
    }

    fn render(&self, markdown: &str) -> String {
        format!("rendered({markdown})")
    }

    fn mention(&self, user: &str) -> String {
        format!("@{user}")
    }
}

/// An [`Asker`] with a fixed answer that remembers what it was asked.
pub struct FakeAsker {
    answer: Result<String, String>,
    seen: Mutex<Vec<(String, String)>>,
}

impl FakeAsker {
    pub fn answering(answer: Result<String, String>) -> Self {
        Self {
            answer,
            seen: Mutex::new(Vec::new()),
        }
    }

    /// The last `(conversation key, prompt)` asked.
    pub fn seen(&self) -> Option<(String, String)> {
        self.seen.lock().unwrap().last().cloned()
    }

    /// Every `(conversation key, prompt)` asked, in order.
    pub fn all(&self) -> Vec<(String, String)> {
        self.seen.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

#[async_trait]
impl Asker for FakeAsker {
    async fn ask(&self, key: &str, prompt: &str) -> Result<String, String> {
        self.seen.lock().unwrap().push((key.into(), prompt.into()));
        self.answer.clone()
    }
}
