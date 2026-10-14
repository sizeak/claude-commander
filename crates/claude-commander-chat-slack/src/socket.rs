//! Thin Socket Mode wiring: connect, lift each raw `slack-morphism` event into
//! a [`SlackEvent`], [`normalize`] it, and hand it to the bridge's inbox.
//! Reconnect and backoff on a dropped socket are the `slack-morphism`
//! listener's own job.

use std::sync::Arc;

use claude_commander_chat::{ChatError, ChatResult, Inbox};
use slack_morphism::prelude::*;
use tracing::{error, info};

use crate::normalize::{SlackEvent, normalize};

/// Handed to the push-event callback through `slack-morphism`'s typed
/// user-state store (callbacks are plain fns, not closures).
struct SocketState {
    inbox: Inbox,
    bot_user_id: Option<String>,
}

/// Listen until the connection ends for good.
pub(crate) async fn listen(
    client: Arc<SlackHyperClient>,
    app_token: &SlackApiToken,
    inbox: Inbox,
    bot_user_id: Option<String>,
) -> ChatResult<()> {
    let state = Arc::new(SocketState { inbox, bot_user_id });
    let callbacks = SlackSocketModeListenerCallbacks::new().with_push_events(on_push_event);
    let environment = Arc::new(
        SlackClientEventsListenerEnvironment::new(client)
            .with_user_state(state)
            .with_error_handler(on_error),
    );
    let listener = SlackClientSocketModeListener::new(
        &SlackClientSocketModeConfig::new(),
        environment,
        callbacks,
    );
    info!(target: "slack", "connecting via Socket Mode");
    listener
        .listen_for(app_token)
        .await
        .map_err(|e| ChatError::new(format!("socket mode: {e}")))?;
    listener.serve().await;
    Ok(())
}

/// Push-event entry point. Must return promptly (Slack redelivers slow acks),
/// so it only enqueues; the bridge does the work on its own tasks.
async fn on_push_event(
    event: SlackPushEventCallback,
    _client: Arc<SlackHyperClient>,
    states: SlackClientEventsUserState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = states
        .read()
        .await
        .get_user_state::<Arc<SocketState>>()
        .cloned();
    let Some(state) = state else {
        return Ok(());
    };
    if let Some(message) = lift(&event).and_then(|e| normalize(&e, state.bot_user_id.as_deref())) {
        state.inbox.deliver(message);
    }
    Ok(())
}

/// Observability-only error handler. Reconnection is the listener's job.
fn on_error(
    err: Box<dyn std::error::Error + Send + Sync>,
    _client: Arc<SlackHyperClient>,
    _states: SlackClientEventsUserState,
) -> HttpStatusCode {
    error!(target: "slack", "socket mode error: {err}");
    HttpStatusCode::OK
}

/// Lift a raw push event into a [`SlackEvent`], or `None` for events the bridge
/// doesn't handle. Channel mentions arrive as `app_mention`; `message` events
/// are only taken from DMs (`im`) so channel messages aren't handled twice.
fn lift(event: &SlackPushEventCallback) -> Option<SlackEvent> {
    match &event.event {
        SlackEventCallbackBody::AppMention(ev) => Some(SlackEvent {
            is_direct: false,
            channel: ev.channel.to_string(),
            ts: ev.origin.ts.to_string(),
            thread_ts: ev.origin.thread_ts.as_ref().map(|t| t.to_string()),
            user: Some(ev.user.to_string()),
            bot_id: None,
            subtype: None,
            hidden: false,
            text: ev.content.text.clone().unwrap_or_default(),
        }),
        SlackEventCallbackBody::Message(ev) => {
            let is_im = ev.origin.channel_type.as_ref().is_some_and(|t| t.0 == "im");
            if !is_im {
                return None;
            }
            Some(SlackEvent {
                is_direct: true,
                channel: ev.origin.channel.as_ref()?.to_string(),
                ts: ev.origin.ts.to_string(),
                thread_ts: ev.origin.thread_ts.as_ref().map(|t| t.to_string()),
                user: ev.sender.user.as_ref().map(|u| u.to_string()),
                bot_id: ev.sender.bot_id.as_ref().map(|b| b.to_string()),
                subtype: ev.subtype.as_ref().map(|s| format!("{s:?}")),
                hidden: ev.hidden.unwrap_or(false),
                text: ev
                    .content
                    .as_ref()
                    .and_then(|c| c.text.clone())
                    .unwrap_or_default(),
            })
        }
        _ => None,
    }
}
