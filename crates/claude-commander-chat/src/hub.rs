//! [`ChatHub`]: every chat backend this server runs, started together and
//! addressed by id for `notify`.

use std::sync::Arc;

use claude_commander_core::api::NotifyTarget;
use tracing::{error, info};

use crate::backend::{ChatBackend, ChatError, Inbox};
use crate::bridge::{self, Asker};

/// The chat backends running on this server. Any number may run at once; each
/// gets its own bridge, and a session's `notify` goes back to whichever one it
/// came from.
pub struct ChatHub {
    backends: Vec<Arc<dyn ChatBackend>>,
    default_notify: Option<String>,
}

/// Where a `notify` was delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// Into the session's originating thread.
    Thread { backend: &'static str },
    /// As a direct message, because the session has no origin.
    Direct { backend: &'static str, user: String },
}

/// Why a `notify` could not be delivered.
#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error("no chat backend is running on this server")]
    NoBackends,
    #[error("the session came from {0:?}, which is not running on this server")]
    OriginNotRunning(String),
    #[error("[chat] default_notify is {0:?}, which is not running on this server")]
    DefaultNotRunning(String),
    #[error("{0} has no allowlisted user to send the message to")]
    NoRecipient(&'static str),
    #[error("{backend} delivery failed: {source}")]
    Delivery {
        backend: &'static str,
        source: ChatError,
    },
}

impl NotifyError {
    /// Whether the failure is the platform rejecting a delivery (as opposed to
    /// this server not being set up to deliver at all).
    pub fn is_delivery_failure(&self) -> bool {
        matches!(self, Self::Delivery { .. })
    }
}

impl ChatHub {
    /// `default_notify` is `[chat] default_notify`: the backend that receives
    /// notifies from sessions with no origin (the first backend when unset).
    pub fn new(backends: Vec<Arc<dyn ChatBackend>>, default_notify: Option<String>) -> Self {
        Self {
            backends,
            default_notify,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.backends.is_empty()
    }

    pub fn backend_ids(&self) -> Vec<&'static str> {
        self.backends.iter().map(|b| b.id()).collect()
    }

    fn backend(&self, id: &str) -> Option<&Arc<dyn ChatBackend>> {
        self.backends.iter().find(|b| b.id() == id)
    }

    /// Connect every backend and start its bridge. The tasks run for the
    /// process lifetime; a backend whose connection ends for good is logged,
    /// and its `notify` sending keeps working.
    pub fn start(&self, asker: Arc<dyn Asker>) {
        for backend in &self.backends {
            let (inbox, rx) = Inbox::channel();
            tokio::spawn(bridge::run(backend.clone(), asker.clone(), rx));
            let backend = backend.clone();
            tokio::spawn(async move {
                let id = backend.id();
                info!(target: "chat", "{id}: connecting");
                if let Err(e) = backend.run(inbox).await {
                    error!(target: "chat", "{id}: bridge stopped: {e}");
                }
            });
        }
    }

    /// Deliver `message` (Markdown) about a session: into its originating
    /// thread, or — with no origin — as a direct message to the default
    /// backend's first allowlisted user, labelled with the session so it reads
    /// on its own.
    pub async fn notify(
        &self,
        target: &NotifyTarget,
        message: &str,
    ) -> Result<Delivery, NotifyError> {
        if self.backends.is_empty() {
            return Err(NotifyError::NoBackends);
        }
        let delivery_failed =
            |backend: &'static str| move |source| NotifyError::Delivery { backend, source };
        if let Some(origin) = &target.origin {
            let backend = self
                .backend(&origin.backend)
                .ok_or_else(|| NotifyError::OriginNotRunning(origin.backend.clone()))?;
            let id = backend.id();
            backend
                .post_reply(&origin.channel, &origin.thread, &backend.render(message))
                .await
                .map_err(delivery_failed(id))?;
            return Ok(Delivery::Thread { backend: id });
        }
        let backend = match &self.default_notify {
            Some(id) => self
                .backend(id)
                .ok_or_else(|| NotifyError::DefaultNotRunning(id.clone()))?,
            None => &self.backends[0],
        };
        let id = backend.id();
        let user = backend
            .allowed_users()
            .first()
            .ok_or(NotifyError::NoRecipient(id))?
            .clone();
        let text = backend.render(&format!("**{}**\n{message}", target.label));
        backend
            .post_direct(&user, &text)
            .await
            .map_err(delivery_failed(id))?;
        Ok(Delivery::Direct { backend: id, user })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use claude_commander_protocol::session::ChatOrigin;

    use super::*;
    use crate::backend::IncomingMessage;
    use crate::testing::{Call, FakeAsker, FakeBackend};

    fn target(origin: Option<ChatOrigin>) -> NotifyTarget {
        NotifyTarget {
            origin,
            label: "fix-auth (fix-auth-br)".into(),
        }
    }

    fn origin(backend: &str) -> ChatOrigin {
        ChatOrigin {
            backend: backend.into(),
            channel: "C1".into(),
            thread: "100.1".into(),
            permalink: None,
        }
    }

    fn hub(backends: Vec<Arc<FakeBackend>>, default_notify: Option<&str>) -> ChatHub {
        ChatHub::new(
            backends
                .into_iter()
                .map(|b| b as Arc<dyn ChatBackend>)
                .collect(),
            default_notify.map(String::from),
        )
    }

    #[tokio::test]
    async fn an_origin_session_replies_into_its_own_backends_thread() {
        let slack = Arc::new(FakeBackend::new("slack"));
        let other = Arc::new(FakeBackend::new("other"));
        let hub = hub(vec![other.clone(), slack.clone()], None);

        let delivery = hub
            .notify(&target(Some(origin("slack"))), "PR is up")
            .await
            .unwrap();

        assert_eq!(delivery, Delivery::Thread { backend: "slack" });
        assert_eq!(
            slack.calls(),
            vec![Call::Reply {
                channel: "C1".into(),
                thread: "100.1".into(),
                text: "rendered(PR is up)".into(),
            }]
        );
        assert!(other.calls().is_empty());
    }

    #[tokio::test]
    async fn an_originless_session_dms_the_first_backends_first_user_with_a_label() {
        let first =
            Arc::new(FakeBackend::new("first").with_allowed(vec!["U7".into(), "U8".into()]));
        let second = Arc::new(FakeBackend::new("second"));
        let hub = hub(vec![first.clone(), second.clone()], None);

        let delivery = hub.notify(&target(None), "done").await.unwrap();

        assert_eq!(
            delivery,
            Delivery::Direct {
                backend: "first",
                user: "U7".into()
            }
        );
        assert_eq!(
            first.calls(),
            vec![Call::Direct {
                user: "U7".into(),
                text: "rendered(**fix-auth (fix-auth-br)**\ndone)".into(),
            }]
        );
    }

    #[tokio::test]
    async fn default_notify_picks_the_backend_for_originless_sessions() {
        let first = Arc::new(FakeBackend::new("first"));
        let second = Arc::new(FakeBackend::new("second"));
        let hub = hub(vec![first.clone(), second.clone()], Some("second"));
        hub.notify(&target(None), "done").await.unwrap();
        assert!(first.calls().is_empty());
        assert_eq!(second.calls().len(), 1);
    }

    #[tokio::test]
    async fn undeliverable_notifies_say_why() {
        let empty = ChatHub::new(Vec::new(), None);
        assert!(matches!(
            empty.notify(&target(None), "x").await,
            Err(NotifyError::NoBackends)
        ));

        let one = hub(vec![Arc::new(FakeBackend::new("slack"))], Some("discord"));
        assert!(matches!(
            one.notify(&target(Some(origin("teams"))), "x").await,
            Err(NotifyError::OriginNotRunning(b)) if b == "teams"
        ));
        assert!(matches!(
            one.notify(&target(None), "x").await,
            Err(NotifyError::DefaultNotRunning(b)) if b == "discord"
        ));

        let nobody = hub(
            vec![Arc::new(FakeBackend::new("slack").with_allowed(vec![]))],
            None,
        );
        assert!(matches!(
            nobody.notify(&target(None), "x").await,
            Err(NotifyError::NoRecipient("slack"))
        ));

        let failing = hub(
            vec![Arc::new(FakeBackend::new("slack").failing_posts())],
            None,
        );
        let err = failing
            .notify(&target(Some(origin("slack"))), "x")
            .await
            .unwrap_err();
        assert!(err.is_delivery_failure());
    }

    #[tokio::test]
    async fn backends_run_side_by_side_each_answering_its_own_messages() {
        let first = Arc::new(FakeBackend::new("first"));
        let second = Arc::new(FakeBackend::new("second"));
        let hub = hub(vec![first.clone(), second.clone()], None);
        let asker = Arc::new(FakeAsker::answering(Ok("hi".into())));
        hub.start(asker.clone());

        for backend in [&first, &second] {
            let inbox = wait_for(|| backend.inbox()).await;
            assert!(inbox.deliver(IncomingMessage {
                dedup_id: "C1:1".into(),
                is_direct: false,
                channel: "C1".into(),
                message_id: "1".into(),
                thread: "1".into(),
                is_thread_reply: false,
                user: "U1".into(),
                text: "status?".into(),
            }));
        }
        for backend in [&first, &second] {
            wait_for(|| {
                backend
                    .calls()
                    .iter()
                    .any(|c| matches!(c, Call::Reply { .. }))
                    .then_some(())
            })
            .await;
        }
        // Same channel/thread ids on both platforms are still two conversations.
        assert_eq!(asker.count(), 2);
    }

    async fn wait_for<T>(mut probe: impl FnMut() -> Option<T>) -> T {
        for _ in 0..200 {
            if let Some(value) = probe() {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("condition never held");
    }
}
