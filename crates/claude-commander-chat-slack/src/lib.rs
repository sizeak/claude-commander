//! The Slack backend for Claude Commander's chat bridge.
//!
//! [`SlackBackend`] implements [`ChatBackend`] over `slack-morphism`: it listens
//! over Socket Mode (`app_mention` in channels, `message.im` for DMs), turns
//! events into the bridge's messages ([`normalize`]), shows progress as
//! reactions (👀 → ✅/❌), and renders the agents' Markdown as Slack mrkdwn
//! ([`mrkdwn`]). Everything platform-neutral lives in `claude-commander-chat`.

pub mod mrkdwn;
pub mod normalize;
mod socket;

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use claude_commander_chat::{ChatBackend, ChatError, ChatResult, Inbox, Status, ThreadMessage};
use claude_commander_core::config::SlackConfig;
use slack_morphism::prelude::*;
use tracing::{debug, error, warn};

/// Reaction names (Slack `reactions.add`/`remove` `name` values) for each
/// [`Status`].
const REACTION_WORKING: &str = "eyes";
const REACTION_DONE: &str = "white_check_mark";
const REACTION_FAILED: &str = "x";

/// A connected-on-demand Slack workspace: the bot's Web API client plus the
/// Socket Mode app token the listener connects with.
pub struct SlackBackend {
    client: Arc<SlackHyperClient>,
    bot_token: SlackApiToken,
    app_token: SlackApiToken,
    allowed_users: Vec<String>,
    /// Our own bot user id, learnt from `auth.test` when [`run`](ChatBackend::run)
    /// starts, for precise mention stripping and ignoring our own posts.
    bot_user_id: OnceLock<String>,
}

impl SlackBackend {
    /// Build the backend from `[slack]`, or `None` when Slack isn't enabled
    /// there (tokens and an allowlist are all required) or the HTTPS connector
    /// can't be built.
    pub fn from_config(config: &SlackConfig) -> Option<Arc<Self>> {
        if !config.is_enabled() {
            return None;
        }
        install_crypto_provider();
        let connector = match SlackClientHyperConnector::new() {
            Ok(c) => c,
            Err(e) => {
                error!(target: "slack", "could not build Slack HTTP connector: {e}");
                return None;
            }
        };
        // `is_enabled()` guarantees both tokens are present and non-empty.
        let token = |t: &Option<String>| SlackApiToken::new(t.clone().unwrap_or_default().into());
        Some(Arc::new(Self {
            client: Arc::new(SlackClient::new(connector)),
            bot_token: token(&config.bot_token),
            app_token: token(&config.app_token),
            allowed_users: config.allowed_user_ids.clone(),
            bot_user_id: OnceLock::new(),
        }))
    }

    fn session(&self) -> SlackClientSession<'_, SlackClientHyperHttpsConnector> {
        self.client.open_session(&self.bot_token)
    }

    async fn react(&self, channel: &str, ts: &str, name: &str) -> ChatResult<()> {
        let req = SlackApiReactionsAddRequest::new(channel.into(), name.into(), ts.into());
        self.session().reactions_add(&req).await.map_err(to_err)?;
        Ok(())
    }

    async fn unreact(&self, channel: &str, ts: &str, name: &str) -> ChatResult<()> {
        let req = SlackApiReactionsRemoveRequest::new(name.into())
            .with_channel(channel.into())
            .with_timestamp(ts.into());
        self.session()
            .reactions_remove(&req)
            .await
            .map_err(to_err)?;
        Ok(())
    }

    async fn post(&self, channel: &str, thread: Option<&str>, text: &str) -> ChatResult<()> {
        let mut req = SlackApiChatPostMessageRequest::new(
            channel.into(),
            SlackMessageContent::new().with_text(text.to_string()),
        );
        if let Some(thread) = thread {
            req = req.with_thread_ts(thread.into());
        }
        self.session()
            .chat_post_message(&req)
            .await
            .map_err(to_err)?;
        Ok(())
    }
}

/// Make sure rustls has a process-wide crypto provider before the Slack HTTPS
/// connector is built. `slack-morphism`'s connector enables rustls's `ring`
/// provider, and other crates in the same binary (reqwest) enable `aws-lc-rs`;
/// with both compiled in, rustls refuses to choose one implicitly and the
/// connector panics. Installing `ring` is idempotent across calls, and leaves
/// alone a provider something else installed first.
fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Map a Slack Web API error to the bridge's opaque error. `slack-morphism`
/// errors do not embed the token, so this cannot leak secrets.
fn to_err(e: impl std::fmt::Display) -> ChatError {
    ChatError::new(format!("slack api error: {e}"))
}

#[async_trait]
impl ChatBackend for SlackBackend {
    fn id(&self) -> &'static str {
        "slack"
    }

    fn display_name(&self) -> &'static str {
        "Slack"
    }

    fn allowed_users(&self) -> &[String] {
        &self.allowed_users
    }

    async fn run(self: Arc<Self>, inbox: Inbox) -> ChatResult<()> {
        match self.session().auth_test().await {
            Ok(resp) => {
                let _ = self.bot_user_id.set(resp.user_id.to_string());
            }
            // Without our id the mention fallback still works; log and go on.
            Err(e) => warn!(target: "slack", "auth.test failed: {e}"),
        }
        socket::listen(
            self.client.clone(),
            &self.app_token,
            inbox,
            self.bot_user_id.get().cloned(),
        )
        .await
    }

    async fn post_reply(&self, channel: &str, thread: &str, text: &str) -> ChatResult<()> {
        self.post(channel, Some(thread), text).await
    }

    async fn post_direct(&self, user: &str, text: &str) -> ChatResult<()> {
        let req = SlackApiConversationsOpenRequest::new().with_users(vec![user.into()]);
        let resp = self
            .session()
            .conversations_open(&req)
            .await
            .map_err(to_err)?;
        self.post(resp.channel.id.as_ref(), None, text).await
    }

    async fn fetch_thread(&self, channel: &str, thread: &str) -> ChatResult<Vec<ThreadMessage>> {
        let req = SlackApiConversationsRepliesRequest::new(channel.into(), thread.into());
        let resp = self
            .session()
            .conversations_replies(&req)
            .await
            .map_err(to_err)?;
        Ok(resp
            .messages
            .into_iter()
            .map(|m| ThreadMessage {
                user: m.sender.user.map(|u| u.to_string()),
                text: m.content.text.unwrap_or_default(),
                message_id: m.origin.ts.to_string(),
            })
            .collect())
    }

    async fn permalink(&self, channel: &str, message_id: &str) -> ChatResult<Option<String>> {
        let req = SlackApiChatGetPermalinkRequest::new(channel.into(), message_id.into());
        let resp = self
            .session()
            .chat_get_permalink(&req)
            .await
            .map_err(to_err)?;
        Ok(Some(resp.permalink.to_string()))
    }

    /// 👀 while working, then swapped for ✅ or ❌ — removing the 👀 first so
    /// the final state is unambiguous.
    async fn set_status(&self, channel: &str, message_id: &str, status: Status) -> ChatResult<()> {
        let done = match status {
            Status::Working => return self.react(channel, message_id, REACTION_WORKING).await,
            Status::Done => REACTION_DONE,
            Status::Failed => REACTION_FAILED,
        };
        if let Err(e) = self.unreact(channel, message_id, REACTION_WORKING).await {
            debug!(target: "slack", "removing the working reaction failed: {e}");
        }
        self.react(channel, message_id, done).await
    }

    fn render(&self, markdown: &str) -> String {
        mrkdwn::render(markdown)
    }

    fn mention(&self, user: &str) -> String {
        format!("<@{user}>")
    }
}
