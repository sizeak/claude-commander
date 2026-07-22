//! Thin Socket Mode wiring: connect, normalize each raw `slack-morphism` event
//! into an [`IncomingMessage`], dedup, classify, and dispatch accepted messages
//! to [`handle_accepted`] on their own task.
//!
//! Everything with real behaviour is delegated to [`super::decision`] and
//! [`super::handler`]; this module only bridges `slack-morphism`'s callback
//! machinery to those pure/fake-testable pieces. Reconnect and backoff on a
//! dropped socket are handled by the `slack-morphism` listener itself.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use claude_commander_core::Config;
use claude_commander_core::api::CommanderService;
use slack_morphism::prelude::*;
use tracing::{debug, error, info};

use super::client::{CommanderAsker, SlackWebClient};
use super::decision::{Decision, DedupSet, IncomingMessage, MessageKind, classify};
use super::handler::{Asker, SlackApi, handle_accepted};

/// How long a seen event id is remembered for dedup, and how many at most.
const DEDUP_TTL: Duration = Duration::from_secs(600);
const DEDUP_CAPACITY: usize = 4096;

/// Shared state handed to the push-event callback via `slack-morphism`'s typed
/// user-state store (callbacks are plain fns, not closures).
struct BridgeState {
    api: Arc<dyn SlackApi>,
    asker: Arc<dyn Asker>,
    dedup: Mutex<DedupSet>,
    allowed_user_ids: Vec<String>,
    /// Our own bot user id (from `auth.test`), for precise mention stripping.
    bot_user_id: Option<String>,
}

/// Start the Slack bridge as a background task iff `[slack]` is configured.
/// A no-op otherwise. The task runs for the process lifetime, reconnecting
/// internally; if it exits (fatal connect error) the failure is logged.
pub fn spawn_bridge(service: CommanderService, config: &Config) {
    if !config.slack.is_enabled() {
        return;
    }
    // `is_enabled()` guarantees both tokens are present and non-empty.
    let app_token = config.slack.app_token.clone().unwrap_or_default();
    let bot_token = config.slack.bot_token.clone().unwrap_or_default();
    let allowed = config.slack.allowed_user_ids.clone();

    tokio::spawn(async move {
        if let Err(e) = run_bridge(service, app_token, bot_token, allowed).await {
            error!(target: "slack", "Slack bridge stopped: {e}");
        }
    });
}

async fn run_bridge(
    service: CommanderService,
    app_token: String,
    bot_token: String,
    allowed_user_ids: Vec<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = Arc::new(SlackClient::new(SlackClientHyperConnector::new()?));

    let web = Arc::new(SlackWebClient::new(client.clone(), bot_token));
    let bot_user_id = web.auth_user_id().await;
    let asker = Arc::new(CommanderAsker::new(service));

    let state = Arc::new(BridgeState {
        api: web,
        asker,
        dedup: Mutex::new(DedupSet::new(DEDUP_TTL, DEDUP_CAPACITY)),
        allowed_user_ids,
        bot_user_id,
    });

    let callbacks = SlackSocketModeListenerCallbacks::new().with_push_events(on_push_event);
    let environment = Arc::new(
        SlackClientEventsListenerEnvironment::new(client.clone())
            .with_user_state(state)
            .with_error_handler(on_error),
    );
    let listener = SlackClientSocketModeListener::new(
        &SlackClientSocketModeConfig::new(),
        environment,
        callbacks,
    );

    let app = SlackApiToken::new(app_token.into());
    info!(target: "slack", "Slack bridge connecting via Socket Mode");
    listener.listen_for(&app).await?;
    listener.serve().await;
    Ok(())
}

/// Push-event entry point. Must return promptly (Slack redelivers slow acks), so
/// accepted work is spawned onto its own task and this returns immediately.
async fn on_push_event(
    event: SlackPushEventCallback,
    _client: Arc<SlackHyperClient>,
    states: SlackClientEventsUserState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = states
        .read()
        .await
        .get_user_state::<Arc<BridgeState>>()
        .cloned();
    let Some(state) = state else {
        return Ok(());
    };
    let Some(msg) = normalize_event(&event) else {
        return Ok(());
    };

    // Redelivery guard: Slack resends events it thinks weren't acked.
    let is_new = state.dedup.lock().unwrap().insert_if_new(&msg.event_id);
    if !is_new {
        debug!(target: "slack", "dropping redelivered event {}", msg.event_id);
        return Ok(());
    }

    match classify(&msg, &state.allowed_user_ids, state.bot_user_id.as_deref()) {
        Decision::Act(accepted) => {
            let state = state.clone();
            tokio::spawn(async move {
                handle_accepted(state.api.as_ref(), state.asker.as_ref(), &accepted).await;
            });
        }
        Decision::Ignore(reason) => {
            debug!(target: "slack", "ignoring slack event {}: {reason:?}", msg.event_id);
        }
    }
    Ok(())
}

/// Observability-only error handler. Reconnection is the listener's job; we just
/// log and ack.
fn on_error(
    err: Box<dyn std::error::Error + Send + Sync>,
    _client: Arc<SlackHyperClient>,
    _states: SlackClientEventsUserState,
) -> HttpStatusCode {
    error!(target: "slack", "socket mode error: {err}");
    HttpStatusCode::OK
}

/// Normalize a raw push event into the bridge's own [`IncomingMessage`], or
/// `None` for events we don't handle. Channel mentions arrive as `app_mention`;
/// `message` events are only handled for DMs (`im`) so channel messages aren't
/// double-processed.
fn normalize_event(event: &SlackPushEventCallback) -> Option<IncomingMessage> {
    let event_id = event.event_id.to_string();
    match &event.event {
        SlackEventCallbackBody::AppMention(ev) => Some(IncomingMessage {
            event_id,
            kind: MessageKind::Mention,
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
            Some(IncomingMessage {
                event_id,
                kind: MessageKind::DirectMessage,
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
