//! The accepted-event handling flow, generic over its two side-effecting seams
//! so it is testable without Slack or a `claude` subprocess.
//!
//! [`SlackApi`] is the tiny slice of the Slack Web API the bridge needs; the
//! real implementation ([`super::client::SlackWebClient`]) wraps
//! `slack-morphism`, and tests substitute an in-memory fake. [`Asker`] is the
//! headless commander (the real one streams [`CommanderService::commander_ask`]
//! through a [`TextAccumulator`](super::decision::TextAccumulator); the fake
//! returns canned outcomes). The flow itself — react, fetch context, ask, reply,
//! swap the reaction — lives here and is exercised end-to-end against fakes.

use async_trait::async_trait;
use tracing::debug;

use super::decision::{
    AcceptedMessage, REACTION_ACK, REACTION_DONE, REACTION_FAILED, ThreadMessage, assemble_prompt,
    conversation_key, error_reply,
};

/// Result of a Slack Web API call. The error is a plain string: it is only ever
/// logged (never surfaced to a user), and must not carry token material.
pub type SlackResult<T> = Result<T, SlackError>;

/// A Slack Web API failure. Kept opaque so nothing token-bearing is ever
/// formatted into it.
#[derive(Debug, thiserror::Error)]
#[error("slack api error: {0}")]
pub struct SlackError(pub String);

/// The minimal Slack Web API surface the bridge uses. Behind a trait so the
/// handling flow is unit-testable with an in-memory fake.
#[async_trait]
pub trait SlackApi: Send + Sync {
    /// Post a message into a thread.
    async fn post_message(&self, channel: &str, thread_ts: &str, text: &str) -> SlackResult<()>;
    /// Add a reaction emoji to a message.
    async fn add_reaction(&self, channel: &str, ts: &str, name: &str) -> SlackResult<()>;
    /// Remove a reaction emoji from a message.
    async fn remove_reaction(&self, channel: &str, ts: &str, name: &str) -> SlackResult<()>;
    /// Fetch a thread's replies (oldest first) for context.
    async fn fetch_replies(
        &self,
        channel: &str,
        thread_ts: &str,
    ) -> SlackResult<Vec<ThreadMessage>>;
    /// A permalink to a message, for provenance in the prompt.
    async fn get_permalink(&self, channel: &str, ts: &str) -> SlackResult<String>;
    /// Open (or fetch) the DM channel with a user, returning its channel id.
    /// Used by the notify path when a session has no originating thread.
    async fn open_dm(&self, user_id: &str) -> SlackResult<String>;
    /// Post a top-level message to a channel (no thread). Used for DM notifies.
    async fn post_channel_message(&self, channel: &str, text: &str) -> SlackResult<()>;
}

/// The headless-commander seam: ask a prompt in a conversation, get the final
/// reply text or an error message.
#[async_trait]
pub trait Asker: Send + Sync {
    async fn ask(&self, key: &str, prompt: &str) -> Result<String, String>;
}

/// Handle one accepted Slack message end to end. Reaction/permalink/history
/// calls are best-effort: a failure is logged and the flow continues, because a
/// missing 👀 or missing context must not block answering. The single reply and
/// the terminal reaction swap are the load-bearing steps.
pub async fn handle_accepted(api: &dyn SlackApi, asker: &dyn Asker, accepted: &AcceptedMessage) {
    let channel = accepted.channel.as_str();
    let trigger = accepted.trigger_ts.as_str();
    let thread = accepted.thread_root_ts.as_str();

    if let Err(e) = api.add_reaction(channel, trigger, REACTION_ACK).await {
        debug!(target: "slack", "add ack reaction failed: {e}");
    }

    // Only a reply within an existing thread has prior context worth fetching;
    // drop the triggering message itself so it isn't duplicated with the ask.
    let history = if accepted.is_thread_reply {
        match api.fetch_replies(channel, thread).await {
            Ok(mut msgs) => {
                msgs.retain(|m| m.ts != accepted.trigger_ts);
                msgs
            }
            Err(e) => {
                debug!(target: "slack", "fetch replies failed: {e}");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    let permalink = match api.get_permalink(channel, trigger).await {
        Ok(link) => Some(link),
        Err(e) => {
            debug!(target: "slack", "get permalink failed: {e}");
            None
        }
    };

    let key = conversation_key(channel, thread);
    let prompt = assemble_prompt(accepted, &history, permalink.as_deref());

    let (reply, ok) = match asker.ask(&key, &prompt).await {
        Ok(text) => (text, true),
        Err(err) => (error_reply(&err), false),
    };

    // The reply post is load-bearing: if it fails, the user sees no answer, so
    // the final reaction must be ❌ regardless of whether the ask itself succeeded.
    let posted = match api.post_message(channel, thread, &reply).await {
        Ok(()) => true,
        Err(e) => {
            debug!(target: "slack", "post reply failed: {e}");
            false
        }
    };

    // Swap 👀 → ✅/❌. Removing the ack first keeps the final state unambiguous.
    if let Err(e) = api.remove_reaction(channel, trigger, REACTION_ACK).await {
        debug!(target: "slack", "remove ack reaction failed: {e}");
    }
    let final_reaction = if ok && posted {
        REACTION_DONE
    } else {
        REACTION_FAILED
    };
    if let Err(e) = api.add_reaction(channel, trigger, final_reaction).await {
        debug!(target: "slack", "add final reaction failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Post {
            channel: String,
            thread: String,
            text: String,
        },
        Add {
            ts: String,
            name: String,
        },
        Remove {
            ts: String,
            name: String,
        },
        Replies {
            thread: String,
        },
        Permalink {
            ts: String,
        },
    }

    #[derive(Default)]
    struct FakeSlack {
        calls: Mutex<Vec<Call>>,
        replies: Vec<ThreadMessage>,
        fail_permalink: bool,
        fail_post: bool,
    }

    #[async_trait]
    impl SlackApi for FakeSlack {
        async fn post_message(
            &self,
            channel: &str,
            thread_ts: &str,
            text: &str,
        ) -> SlackResult<()> {
            self.calls.lock().unwrap().push(Call::Post {
                channel: channel.into(),
                thread: thread_ts.into(),
                text: text.into(),
            });
            if self.fail_post {
                Err(SlackError("post failed".into()))
            } else {
                Ok(())
            }
        }
        async fn add_reaction(&self, _channel: &str, ts: &str, name: &str) -> SlackResult<()> {
            self.calls.lock().unwrap().push(Call::Add {
                ts: ts.into(),
                name: name.into(),
            });
            Ok(())
        }
        async fn remove_reaction(&self, _channel: &str, ts: &str, name: &str) -> SlackResult<()> {
            self.calls.lock().unwrap().push(Call::Remove {
                ts: ts.into(),
                name: name.into(),
            });
            Ok(())
        }
        async fn fetch_replies(
            &self,
            _channel: &str,
            thread_ts: &str,
        ) -> SlackResult<Vec<ThreadMessage>> {
            self.calls.lock().unwrap().push(Call::Replies {
                thread: thread_ts.into(),
            });
            Ok(self.replies.clone())
        }
        async fn get_permalink(&self, _channel: &str, ts: &str) -> SlackResult<String> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Permalink { ts: ts.into() });
            if self.fail_permalink {
                Err(SlackError("nope".into()))
            } else {
                Ok("https://slack/permalink".into())
            }
        }
        async fn open_dm(&self, user_id: &str) -> SlackResult<String> {
            Ok(format!("D-{user_id}"))
        }
        async fn post_channel_message(&self, _channel: &str, _text: &str) -> SlackResult<()> {
            Ok(())
        }
    }

    struct FakeAsker {
        outcome: Result<String, String>,
        seen: Mutex<Option<(String, String)>>,
    }

    #[async_trait]
    impl Asker for FakeAsker {
        async fn ask(&self, key: &str, prompt: &str) -> Result<String, String> {
            *self.seen.lock().unwrap() = Some((key.into(), prompt.into()));
            self.outcome.clone()
        }
    }

    fn top_level_mention() -> AcceptedMessage {
        AcceptedMessage {
            kind_dm: false,
            channel: "C1".into(),
            trigger_ts: "100.1".into(),
            thread_root_ts: "100.1".into(),
            user: "U1".into(),
            ask_text: "status?".into(),
            is_thread_reply: false,
        }
    }

    #[tokio::test]
    async fn success_flow_posts_reply_and_swaps_ack_to_done() {
        let api = FakeSlack::default();
        let asker = FakeAsker {
            outcome: Ok("all good".into()),
            seen: Mutex::new(None),
        };
        handle_accepted(&api, &asker, &top_level_mention()).await;

        let calls = api.calls.lock().unwrap().clone();
        // 👀 added first, ✅ added last, ❌ never.
        assert_eq!(
            calls.first(),
            Some(&Call::Add {
                ts: "100.1".into(),
                name: REACTION_ACK.into()
            })
        );
        assert!(calls.contains(&Call::Remove {
            ts: "100.1".into(),
            name: REACTION_ACK.into()
        }));
        assert!(calls.contains(&Call::Add {
            ts: "100.1".into(),
            name: REACTION_DONE.into()
        }));
        assert!(
            !calls
                .iter()
                .any(|c| matches!(c, Call::Add { name, .. } if name == REACTION_FAILED))
        );
        // Exactly one reply, in the thread root, carrying the commander text.
        assert!(calls.contains(&Call::Post {
            channel: "C1".into(),
            thread: "100.1".into(),
            text: "all good".into()
        }));
        // A top-level mention fetches no history.
        assert!(!calls.iter().any(|c| matches!(c, Call::Replies { .. })));
        // The ask used the derived conversation key.
        let (key, _) = asker.seen.lock().unwrap().clone().unwrap();
        assert_eq!(key, "slack:C1:100.1");
    }

    #[tokio::test]
    async fn failure_flow_posts_error_and_swaps_ack_to_x() {
        let api = FakeSlack::default();
        let asker = FakeAsker {
            outcome: Err("timed out".into()),
            seen: Mutex::new(None),
        };
        handle_accepted(&api, &asker, &top_level_mention()).await;

        let calls = api.calls.lock().unwrap().clone();
        assert!(calls.contains(&Call::Add {
            ts: "100.1".into(),
            name: REACTION_FAILED.into()
        }));
        assert!(
            !calls
                .iter()
                .any(|c| matches!(c, Call::Add { name, .. } if name == REACTION_DONE))
        );
        // The single reply is the formatted error text.
        assert!(calls.iter().any(|c| matches!(
            c,
            Call::Post { text, .. } if text.contains("timed out")
        )));
    }

    #[tokio::test]
    async fn failed_reply_post_yields_x_even_when_ask_succeeded() {
        // The commander answered, but posting the reply to Slack failed — the
        // user sees nothing, so the honest final reaction is ❌, never ✅.
        let api = FakeSlack {
            fail_post: true,
            ..Default::default()
        };
        let asker = FakeAsker {
            outcome: Ok("all good".into()),
            seen: Mutex::new(None),
        };
        handle_accepted(&api, &asker, &top_level_mention()).await;

        let calls = api.calls.lock().unwrap().clone();
        assert!(calls.contains(&Call::Add {
            ts: "100.1".into(),
            name: REACTION_FAILED.into()
        }));
        assert!(
            !calls
                .iter()
                .any(|c| matches!(c, Call::Add { name, .. } if name == REACTION_DONE)),
            "a failed reply post must not leave a ✅"
        );
    }

    #[tokio::test]
    async fn thread_reply_fetches_history_excluding_the_trigger() {
        let api = FakeSlack {
            replies: vec![
                ThreadMessage {
                    user: Some("U1".into()),
                    text: "root".into(),
                    ts: "50.5".into(),
                },
                ThreadMessage {
                    user: Some("U1".into()),
                    text: "the trigger".into(),
                    ts: "100.1".into(),
                },
            ],
            ..Default::default()
        };
        let asker = FakeAsker {
            outcome: Ok("done".into()),
            seen: Mutex::new(None),
        };
        let mut accepted = top_level_mention();
        accepted.thread_root_ts = "50.5".into();
        accepted.is_thread_reply = true;
        handle_accepted(&api, &asker, &accepted).await;

        assert!(api.calls.lock().unwrap().contains(&Call::Replies {
            thread: "50.5".into()
        }));
        let (_, prompt) = asker.seen.lock().unwrap().clone().unwrap();
        assert!(prompt.contains("root"));
        // The trigger message is filtered out of the fetched context.
        assert!(!prompt.contains("the trigger"));
    }
}
