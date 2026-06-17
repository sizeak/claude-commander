//! Streaming TTS: turn the assistant's token deltas into spoken sentences.
//!
//! Deltas arrive split mid-word, so [`SentenceAccumulator`] buffers them and
//! emits whole sentences as they complete. The [`spawn_speaker`] task owns the
//! audio [`Player`] and TTS client, synthesizing + queuing each sentence as it
//! lands so speech starts within a sentence of the assistant beginning to type.

use tokio::sync::mpsc;
use tracing::warn;

use crate::config::ConversationConfig;
use crate::conversation::audio::Player;
use crate::conversation::extract::{SpeakScope, first_sentence_boundary, spoken_text};
use crate::conversation::tts::{SpeechRequest, TtsClient};
use crate::error::TtsError;

/// Buffers streamed text and yields complete sentences. Pure + unit-tested.
#[derive(Debug, Default)]
pub struct SentenceAccumulator {
    buf: String,
}

impl SentenceAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a streamed chunk; return any sentences that are now complete. A
    /// sentence is emitted as soon as its boundary is confirmed (a terminator
    /// followed by whitespace) — we don't wait for the next sentence to begin.
    /// The trailing, not-yet-terminated text stays buffered for
    /// [`flush`](Self::flush).
    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.buf.push_str(text);
        let mut out = Vec::new();
        while let Some(end) = first_sentence_boundary(&self.buf) {
            let sentence = self.buf[..end].trim().to_string();
            self.buf = self.buf[end..].trim_start().to_string();
            if !sentence.is_empty() {
                out.push(sentence);
            }
        }
        out
    }

    /// Return and clear whatever remains (end of turn).
    pub fn flush(&mut self) -> Option<String> {
        let remaining = std::mem::take(&mut self.buf);
        let remaining = remaining.trim().to_string();
        (!remaining.is_empty()).then_some(remaining)
    }

    /// Drop buffered text without speaking it (interrupt).
    pub fn clear(&mut self) {
        self.buf.clear();
    }
}

/// Commands to the speaker task.
#[derive(Debug, Clone)]
pub enum SpeakerCommand {
    /// A streamed chunk of assistant text.
    Chunk(String),
    /// The turn finished — speak any buffered remainder.
    Flush,
    /// Stop playback and discard buffered text (e.g. a new user turn).
    Interrupt,
}

/// Start the speaker task. Returns a sender for [`SpeakerCommand`]s, or an error
/// if no audio output device is available. Dropping the sender ends the task and
/// stops audio.
pub fn spawn_speaker(
    cfg: ConversationConfig,
) -> Result<mpsc::UnboundedSender<SpeakerCommand>, TtsError> {
    let player = Player::new(cfg.volume)?;
    let client = TtsClient::new(cfg.base_url.clone());
    let (tx, mut rx) = mpsc::unbounded_channel::<SpeakerCommand>();

    tokio::spawn(async move {
        let mut acc = SentenceAccumulator::new();
        while let Some(cmd) = rx.recv().await {
            match cmd {
                SpeakerCommand::Chunk(text) => {
                    for sentence in acc.push(&text) {
                        speak(&client, &player, &cfg, &sentence).await;
                    }
                }
                SpeakerCommand::Flush => {
                    if let Some(remainder) = acc.flush() {
                        speak(&client, &player, &cfg, &remainder).await;
                    }
                }
                SpeakerCommand::Interrupt => {
                    acc.clear();
                    player.stop();
                }
            }
        }
    });

    Ok(tx)
}

async fn speak(client: &TtsClient, player: &Player, cfg: &ConversationConfig, sentence: &str) {
    // Verbatim speaks markup as-is; every other scope strips to prose. (A
    // "final summary" can't be known mid-stream, so it degrades to prose here.)
    let text = match cfg.speak_scope {
        SpeakScope::Verbatim => sentence.to_string(),
        _ => match spoken_text(&[sentence.to_string()], SpeakScope::ProseOnly) {
            Some(t) => t,
            None => return, // nothing speakable (e.g. a code-only fragment)
        },
    };
    if text.trim().is_empty() {
        return;
    }
    let voice = cfg.voice.clone().unwrap_or_default();
    let req = SpeechRequest {
        model: &cfg.model,
        input: &text,
        voice: &voice,
        response_format: &cfg.response_format,
        speed: cfg.speed,
    };
    match client.synthesize(&req).await {
        Ok(bytes) => player.enqueue(bytes),
        Err(e) => warn!("TTS synthesis failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulator_emits_completed_sentences_only() {
        let mut acc = SentenceAccumulator::new();
        // A single in-progress sentence isn't emitted yet.
        assert!(acc.push("Hello ther").is_empty());
        // Completing it and starting another emits the first.
        assert_eq!(acc.push("e. How are"), vec!["Hello there."]);
        // Finishing the second + starting a third emits the second.
        assert_eq!(acc.push(" you? And then"), vec!["How are you?"]);
        // Flush yields the remainder.
        assert_eq!(acc.flush(), Some("And then".to_string()));
    }

    #[test]
    fn accumulator_emits_as_soon_as_boundary_confirmed() {
        let mut acc = SentenceAccumulator::new();
        // No boundary yet.
        assert!(acc.push("First sentence").is_empty());
        // A terminator + trailing whitespace confirms it immediately — no need
        // to wait for the next sentence's text to arrive.
        assert_eq!(acc.push(". "), vec!["First sentence."]);
    }

    #[test]
    fn accumulator_handles_multiple_sentences_in_one_chunk() {
        let mut acc = SentenceAccumulator::new();
        let out = acc.push("One. Two. Three");
        assert_eq!(out, vec!["One.", "Two."]);
        assert_eq!(acc.flush(), Some("Three".to_string()));
    }

    #[test]
    fn accumulator_flush_empty_is_none() {
        let mut acc = SentenceAccumulator::new();
        assert_eq!(acc.flush(), None);
        acc.push("partial");
        acc.clear();
        assert_eq!(acc.flush(), None);
    }

    #[test]
    fn accumulator_does_not_split_abbreviations() {
        let mut acc = SentenceAccumulator::new();
        // "e.g." must not be treated as a sentence end.
        let out = acc.push("Use it, e.g. like this. Next");
        assert_eq!(out, vec!["Use it, e.g. like this."]);
    }
}
