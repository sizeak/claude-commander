//! Turning a Slack event into the bridge's [`IncomingMessage`] — pure, so the
//! Slack-specific filtering is unit-tested without a Socket Mode connection.

use claude_commander_chat::IncomingMessage;

/// The fields of a Slack `app_mention` or `message.im` event the bridge needs,
/// lifted off the `slack-morphism` types by the listener.
#[derive(Debug, Clone)]
pub struct SlackEvent {
    /// An `im` message rather than a channel `app_mention`.
    pub is_direct: bool,
    /// Channel id (`C…` for a mention, the `im` id `D…` for a DM).
    pub channel: String,
    /// Timestamp of the message: its id, and the status-reaction target.
    pub ts: String,
    /// Thread timestamp when the message is a reply within a thread.
    pub thread_ts: Option<String>,
    /// Sending user id, when present.
    pub user: Option<String>,
    /// Bot id, present iff a bot sent the message.
    pub bot_id: Option<String>,
    /// Message subtype (`message_changed`, `message_deleted`, joins, …). Any
    /// subtype means this is not a fresh human message.
    pub subtype: Option<String>,
    /// Slack `hidden` flag (set on tombstones/edits).
    pub hidden: bool,
    /// Raw text, still carrying any `<@BOT>` mention token.
    pub text: String,
}

/// Normalise `event`, or `None` for anything that isn't a fresh message from a
/// person: bot posts (our own included, by bot id or by `bot_user_id`), edits
/// and other subtypes, hidden tombstones, and messages with no user. The bot's
/// own mention is stripped from the text.
///
/// Dedup is on `(channel, ts)`, not the event envelope id: Slack delivers a DM
/// that mentions the bot as both an `app_mention` and a `message.im` with
/// distinct envelope ids, and both describe one message.
pub fn normalize(event: &SlackEvent, bot_user_id: Option<&str>) -> Option<IncomingMessage> {
    if event.bot_id.is_some() || event.subtype.is_some() || event.hidden {
        return None;
    }
    let user = event.user.as_deref().filter(|u| !u.is_empty())?;
    if bot_user_id == Some(user) {
        return None;
    }
    let thread_ts = event.thread_ts.as_deref().filter(|t| !t.is_empty());
    Some(IncomingMessage {
        dedup_id: format!("{}:{}", event.channel, event.ts),
        is_direct: event.is_direct,
        channel: event.channel.clone(),
        message_id: event.ts.clone(),
        // A top-level message roots its own thread.
        thread: thread_ts.unwrap_or(&event.ts).to_string(),
        is_thread_reply: thread_ts.is_some_and(|t| t != event.ts),
        user: user.to_string(),
        text: strip_mention(&event.text, bot_user_id),
    })
}

/// Strip the bot's `<@ID>` mention token(s) from `text`. With the bot's id
/// known, only its tokens (plain `<@ID>` and labelled `<@ID|name>`) go, so
/// mentions of *other* users survive as context; without it (`auth.test`
/// failed), a single leading mention token is dropped.
pub fn strip_mention(text: &str, bot_user_id: Option<&str>) -> String {
    let stripped = match bot_user_id {
        Some(id) => remove_bot_mentions(text, id),
        None => strip_leading_mention(text),
    };
    stripped.trim().to_string()
}

fn remove_bot_mentions(text: &str, bot_id: &str) -> String {
    let mut out = text.replace(&format!("<@{bot_id}>"), "");
    let prefix = format!("<@{bot_id}|");
    while let Some(start) = out.find(&prefix) {
        let Some(rel) = out[start..].find('>') else {
            break;
        };
        out.replace_range(start..start + rel + 1, "");
    }
    out
}

fn strip_leading_mention(text: &str) -> String {
    let trimmed = text.trim_start();
    if let Some(rest) = trimmed.strip_prefix("<@")
        && let Some(rel) = rest.find('>')
    {
        return rest[rel + 1..].to_string();
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(text: &str) -> SlackEvent {
        SlackEvent {
            is_direct: false,
            channel: "C1".into(),
            ts: "100.1".into(),
            thread_ts: None,
            user: Some("U1".into()),
            bot_id: None,
            subtype: None,
            hidden: false,
            text: text.into(),
        }
    }

    #[test]
    fn a_top_level_mention_roots_its_own_thread_with_the_mention_stripped() {
        let m = normalize(&event("<@BOT> what is the status?"), Some("BOT")).unwrap();
        assert_eq!(m.text, "what is the status?");
        assert_eq!((m.channel.as_str(), m.message_id.as_str()), ("C1", "100.1"));
        assert_eq!(m.thread, "100.1");
        assert!(!m.is_thread_reply);
        assert_eq!(m.user, "U1");
        assert_eq!(m.dedup_id, "C1:100.1");
    }

    #[test]
    fn a_thread_reply_uses_the_thread_root() {
        let mut e = event("<@BOT> follow up");
        e.thread_ts = Some("50.5".into());
        let m = normalize(&e, Some("BOT")).unwrap();
        assert_eq!(m.thread, "50.5");
        assert!(m.is_thread_reply);
    }

    #[test]
    fn noise_is_dropped() {
        let mut bot = event("hi");
        bot.bot_id = Some("B1".into());
        let mut edited = event("hi");
        edited.subtype = Some("message_changed".into());
        let mut hidden = event("hi");
        hidden.hidden = true;
        let mut nobody = event("hi");
        nobody.user = None;
        let mut ourselves = event("hi");
        ourselves.user = Some("BOT".into());
        for e in [bot, edited, hidden, nobody, ourselves] {
            assert!(normalize(&e, Some("BOT")).is_none(), "{e:?}");
        }
    }

    #[test]
    fn a_dm_and_its_mention_event_share_a_dedup_id() {
        let mention = event("<@BOT> hi");
        let mut dm = mention.clone();
        dm.is_direct = true;
        let a = normalize(&mention, Some("BOT")).unwrap();
        let b = normalize(&dm, Some("BOT")).unwrap();
        assert_eq!(a.dedup_id, b.dedup_id);
        let mut later = mention.clone();
        later.ts = "200.2".into();
        assert_ne!(normalize(&later, Some("BOT")).unwrap().dedup_id, a.dedup_id);
    }

    #[test]
    fn strip_mention_keeps_other_users_and_falls_back_without_our_id() {
        assert_eq!(
            strip_mention("<@BOT> ping <@U2> please", Some("BOT")),
            "ping <@U2> please"
        );
        assert_eq!(strip_mention("<@BOT|commander> go", Some("BOT")), "go");
        assert_eq!(strip_mention("<@ANY> do it", None), "do it");
        assert_eq!(strip_mention("just text", None), "just text");
    }
}
