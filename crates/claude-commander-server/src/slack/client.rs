//! Concrete `slack-morphism`-backed [`SlackApi`] and the real [`Asker`].
//!
//! This is the thin network layer: each method opens a session with the bot
//! token and issues one Web API call, translating the request/response into the
//! bridge's own token-free types. [`CommanderAsker`] streams
//! [`CommanderService::commander_ask`] through a
//! [`TextAccumulator`](super::decision::TextAccumulator) to a single final
//! reply.

use std::sync::Arc;

use async_trait::async_trait;
use claude_commander_core::api::CommanderService;
use slack_morphism::prelude::*;
use tracing::warn;

use super::decision::{TextAccumulator, ThreadMessage};
use super::handler::{Asker, SlackApi, SlackError, SlackResult};

/// Concrete Slack Web API client over a shared hyper connector + bot token.
pub struct SlackWebClient {
    client: Arc<SlackHyperClient>,
    token: SlackApiToken,
}

impl SlackWebClient {
    pub fn new(client: Arc<SlackHyperClient>, bot_token: String) -> Self {
        Self {
            client,
            token: SlackApiToken::new(bot_token.into()),
        }
    }

    /// Best-effort `auth.test` to learn our own bot user id, so the bridge can
    /// strip our mention from an ask precisely. `None` on failure — the bridge
    /// falls back to dropping a single leading mention token.
    pub async fn auth_user_id(&self) -> Option<String> {
        let session = self.client.open_session(&self.token);
        match session.auth_test().await {
            Ok(resp) => Some(resp.user_id.to_string()),
            Err(e) => {
                warn!(target: "slack", "auth.test failed: {e}");
                None
            }
        }
    }
}

/// Map any Slack Web API error to the bridge's opaque error. `slack-morphism`
/// errors do not embed the token, so this cannot leak secrets.
fn to_err(e: impl std::fmt::Display) -> SlackError {
    SlackError(e.to_string())
}

#[async_trait]
impl SlackApi for SlackWebClient {
    async fn post_message(&self, channel: &str, thread_ts: &str, text: &str) -> SlackResult<()> {
        let session = self.client.open_session(&self.token);
        let req = SlackApiChatPostMessageRequest::new(
            SlackChannelId::new(channel.to_string()),
            SlackMessageContent::new().with_text(text.to_string()),
        )
        .with_thread_ts(SlackTs::new(thread_ts.to_string()));
        session.chat_post_message(&req).await.map_err(to_err)?;
        Ok(())
    }

    async fn add_reaction(&self, channel: &str, ts: &str, name: &str) -> SlackResult<()> {
        let session = self.client.open_session(&self.token);
        let req = SlackApiReactionsAddRequest::new(
            SlackChannelId::new(channel.to_string()),
            SlackReactionName::new(name.to_string()),
            SlackTs::new(ts.to_string()),
        );
        session.reactions_add(&req).await.map_err(to_err)?;
        Ok(())
    }

    async fn remove_reaction(&self, channel: &str, ts: &str, name: &str) -> SlackResult<()> {
        let session = self.client.open_session(&self.token);
        let req = SlackApiReactionsRemoveRequest::new(SlackReactionName::new(name.to_string()))
            .with_channel(SlackChannelId::new(channel.to_string()))
            .with_timestamp(SlackTs::new(ts.to_string()));
        session.reactions_remove(&req).await.map_err(to_err)?;
        Ok(())
    }

    async fn fetch_replies(
        &self,
        channel: &str,
        thread_ts: &str,
    ) -> SlackResult<Vec<ThreadMessage>> {
        let session = self.client.open_session(&self.token);
        let req = SlackApiConversationsRepliesRequest::new(
            SlackChannelId::new(channel.to_string()),
            SlackTs::new(thread_ts.to_string()),
        );
        let resp = session.conversations_replies(&req).await.map_err(to_err)?;
        Ok(resp
            .messages
            .into_iter()
            .map(|m| ThreadMessage {
                user: m.sender.user.map(|u| u.to_string()),
                text: m.content.text.unwrap_or_default(),
                ts: m.origin.ts.to_string(),
            })
            .collect())
    }

    async fn get_permalink(&self, channel: &str, ts: &str) -> SlackResult<String> {
        let session = self.client.open_session(&self.token);
        let req = SlackApiChatGetPermalinkRequest::new(
            SlackChannelId::new(channel.to_string()),
            SlackTs::new(ts.to_string()),
        );
        let resp = session.chat_get_permalink(&req).await.map_err(to_err)?;
        Ok(resp.permalink.to_string())
    }

    async fn open_dm(&self, user_id: &str) -> SlackResult<String> {
        let session = self.client.open_session(&self.token);
        let req = SlackApiConversationsOpenRequest::new()
            .with_users(vec![SlackUserId::new(user_id.to_string())]);
        let resp = session.conversations_open(&req).await.map_err(to_err)?;
        Ok(resp.channel.id.to_string())
    }

    async fn post_channel_message(&self, channel: &str, text: &str) -> SlackResult<()> {
        let session = self.client.open_session(&self.token);
        let req = SlackApiChatPostMessageRequest::new(
            SlackChannelId::new(channel.to_string()),
            SlackMessageContent::new().with_text(text.to_string()),
        );
        session.chat_post_message(&req).await.map_err(to_err)?;
        Ok(())
    }
}

/// The real headless-commander seam: stream one ask to completion and fold the
/// events into a single reply (or an error message).
pub struct CommanderAsker {
    service: CommanderService,
}

impl CommanderAsker {
    pub fn new(service: CommanderService) -> Self {
        Self { service }
    }
}

#[async_trait]
impl Asker for CommanderAsker {
    async fn ask(&self, key: &str, prompt: &str) -> Result<String, String> {
        let mut stream = self.service.commander_ask(key, prompt);
        let mut acc = TextAccumulator::new();
        while let Some(ev) = stream.recv().await {
            acc.push(&ev);
        }
        acc.finish()
    }
}
