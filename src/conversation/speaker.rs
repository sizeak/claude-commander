//! Streaming TTS: turn the assistant's token deltas into spoken sentences.
//!
//! Deltas arrive split mid-word, so [`SentenceAccumulator`] buffers them and
//! emits whole sentences as they complete. The [`spawn_speaker`] task owns the
//! audio [`Player`] and TTS client, synthesizing + queuing each sentence as it
//! lands so speech starts within a sentence of the assistant beginning to type.

use futures::stream::{FuturesOrdered, StreamExt};
use tokio::sync::mpsc;
use tracing::warn;

use crate::config::ConversationConfig;
use crate::conversation::audio::{PlaybackEdge, Player};
use crate::conversation::extract::{SpeakScope, first_sentence_boundary, spoken_text};
use crate::conversation::media::{MediaSignal, signal as media_signal};
use crate::conversation::tts::{SpeechRequest, TtsClient};
use crate::error::TtsError;

/// Maximum concurrent synth requests. Enough to keep the audio queue fed
/// (synth runs ahead of playback) without flooding the TTS server, which on a
/// CPU box would cause request timeouts and dropped/skipped sentences.
const MAX_INFLIGHT: usize = 3;

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeakerCommand {
    /// A streamed chunk of assistant text.
    Chunk(String),
    /// The turn finished — speak any buffered remainder.
    Flush,
    /// Stop playback, discard buffered text, and mute until [`Resume`](Self::Resume)
    /// (the user started a new message — don't resume the old reply).
    Interrupt,
    /// Unmute: the new query has been submitted, so speak its reply.
    Resume,
}

/// What [`SpeakerCommand`] should make the speaker task do, given the current
/// mute state. Split out as a pure state machine (like [`crate::conversation::media::MediaGate`])
/// so the mute logic is unit-testable without an audio device.
#[derive(Debug, PartialEq, Eq)]
pub enum SpeakerEffect {
    /// Append the chunk's text to the accumulator and queue any complete sentences.
    Accumulate,
    /// Speak the buffered remainder.
    FlushBuffer,
    /// Stop playback, clear the buffer/queue/in-flight synth, and mute.
    InterruptAndMute,
    /// Clear the mute.
    Unmute,
    /// Drop the command (muted, or a no-op).
    Ignore,
}

/// Mute state for the speaker. Once interrupted (the user began a new message),
/// streamed chunks are dropped until [`SpeakerCommand::Resume`] arrives — so the
/// interrupted reply never resumes speaking.
#[derive(Debug, Default)]
pub struct SpeakerState {
    muted: bool,
}

impl SpeakerState {
    /// Decide what a command should do, updating the mute flag.
    pub fn on(&mut self, cmd: &SpeakerCommand) -> SpeakerEffect {
        match cmd {
            SpeakerCommand::Chunk(_) | SpeakerCommand::Flush if self.muted => SpeakerEffect::Ignore,
            SpeakerCommand::Chunk(_) => SpeakerEffect::Accumulate,
            SpeakerCommand::Flush => SpeakerEffect::FlushBuffer,
            SpeakerCommand::Interrupt => {
                self.muted = true;
                SpeakerEffect::InterruptAndMute
            }
            SpeakerCommand::Resume => {
                self.muted = false;
                SpeakerEffect::Unmute
            }
        }
    }
}

/// Respawn-stable handle to the current session's TTS speaker. The speaker is
/// recreated on each session (re)spawn, but the listener (which interrupts on
/// record-start) and the submit path (which resumes for the new reply) are
/// long-lived — they hold this handle and it's repointed at the live speaker.
/// Sends are best-effort: a no-op when no speaker is up.
#[derive(Clone, Default)]
pub struct SpeakerHandle(
    std::sync::Arc<std::sync::Mutex<Option<mpsc::UnboundedSender<SpeakerCommand>>>>,
);

impl SpeakerHandle {
    /// Point at the current speaker (or `None` when TTS is off / no session).
    pub fn set(&self, tx: Option<mpsc::UnboundedSender<SpeakerCommand>>) {
        *self.0.lock().unwrap() = tx;
    }

    /// Deliver a command to the current speaker, if any (best-effort no-op).
    pub fn send(&self, cmd: SpeakerCommand) {
        if let Some(tx) = self.0.lock().unwrap().as_ref() {
            let _ = tx.send(cmd);
        }
    }
}

/// Map a session event to the speaker command it drives (if any). Pure, so the
/// bridge's audio routing is unit-testable. Lets speech be driven straight off
/// the session stream, independent of the UI loop.
pub fn speaker_command_for(ev: &crate::conversation::ConversationEvent) -> Option<SpeakerCommand> {
    use crate::conversation::ConversationEvent as E;
    match ev {
        E::Delta(text) => Some(SpeakerCommand::Chunk(text.clone())),
        // A block boundary, turn end, or error all flush the buffered sentence.
        E::Break | E::TurnComplete | E::Error(_) => Some(SpeakerCommand::Flush),
        E::Started { .. } | E::Exited => None,
    }
}

/// Start the speaker task. Returns a sender for [`SpeakerCommand`]s, or an error
/// if no audio output device is available. Dropping the sender ends the task and
/// stops audio.
///
/// Sentences are synthesized **concurrently** (via `FuturesOrdered`) but enqueued
/// **in order**, so a chunk's audio is ready by the time the previous one
/// finishes playing — no pause-to-synthesize gap between short chunks. Playback
/// itself runs on the audio thread, so `enqueue` never blocks synthesis.
pub fn spawn_speaker(
    cfg: ConversationConfig,
    gate: Option<mpsc::UnboundedSender<MediaSignal>>,
) -> Result<mpsc::UnboundedSender<SpeakerCommand>, TtsError> {
    // Playback span boundaries from the audio thread, so we can tell the media
    // gate when the reply actually starts and finishes being spoken.
    let (edge_tx, mut edge_rx) = mpsc::unbounded_channel::<PlaybackEdge>();
    let player = Player::with_edges(cfg.volume, gate.as_ref().map(|_| edge_tx))?;
    let client = TtsClient::new(cfg.base_url.clone());
    let (tx, mut rx) = mpsc::unbounded_channel::<SpeakerCommand>();

    tokio::spawn(async move {
        let mut acc = SentenceAccumulator::new();
        // Sentences awaiting synthesis (in order), and the bounded set of
        // in-flight synth requests (polled concurrently, yielded in push order).
        let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::new();
        let mut pending: FuturesOrdered<_> = FuturesOrdered::new();
        // Whether we've told the gate the reply is being spoken (and not yet that
        // it finished). Tracks contiguous-plus-gapped playback as one logical span.
        let mut speaking = false;
        // Mute state: after an Interrupt (new user message) we drop streamed text
        // until Resume, so the interrupted reply never resumes speaking.
        let mut state = SpeakerState::default();
        loop {
            // Keep at most MAX_INFLIGHT requests running: enough to stay ahead
            // of playback, few enough not to overload the TTS server (which
            // would cause timeouts and dropped — i.e. skipped — sentences).
            while pending.len() < MAX_INFLIGHT {
                let Some(sentence) = queue.pop_front() else {
                    break;
                };
                if let Some(fut) = synth_future(&client, &cfg, &sentence) {
                    pending.push_back(fut);
                }
            }
            tokio::select! {
                // Enqueue finished audio in order as it becomes ready.
                Some(result) = pending.next(), if !pending.is_empty() => match result {
                    Ok(bytes) => {
                        tracing::debug!(target: "conversation", "audio enqueued: {} bytes", bytes.len());
                        player.enqueue(bytes);
                    }
                    Err(e) => warn!(target: "conversation", "TTS synthesis failed: {e}"),
                },
                // Playback boundaries → tell the media gate when the spoken reply
                // begins and (once nothing is queued or synthesizing) ends.
                Some(edge) = edge_rx.recv() => match edge {
                    PlaybackEdge::Started if !speaking => {
                        speaking = true;
                        media_signal(&gate, MediaSignal::SpeakingStarted);
                    }
                    PlaybackEdge::Stopped if speaking && queue.is_empty() && pending.is_empty() => {
                        speaking = false;
                        media_signal(&gate, MediaSignal::SpeakingEnded);
                    }
                    // A gap mid-reply (more queued/synthesizing) or a redundant
                    // edge — keep the current span open.
                    _ => {}
                },
                cmd = rx.recv() => match cmd {
                    Some(cmd) => match state.on(&cmd) {
                        SpeakerEffect::Accumulate => {
                            if let SpeakerCommand::Chunk(text) = &cmd {
                                queue.extend(acc.push(text));
                            }
                        }
                        SpeakerEffect::FlushBuffer => {
                            if let Some(remainder) = acc.flush() {
                                queue.push_back(remainder);
                            }
                        }
                        SpeakerEffect::InterruptAndMute => {
                            acc.clear();
                            queue.clear();
                            pending = FuturesOrdered::new(); // cancel in-flight synth
                            player.stop();
                        }
                        // Unmute (Resume) and Ignore (dropped while muted) need no
                        // side effects — the state flag is already updated.
                        SpeakerEffect::Unmute | SpeakerEffect::Ignore => {}
                    },
                    None => break, // sender dropped → end the task (and audio)
                },
            }
        }
    });

    Ok(tx)
}

/// Build a self-contained synth future for one sentence, or `None` if nothing is
/// speakable (e.g. a code-only fragment). Owns its inputs so it can run
/// concurrently in a `FuturesOrdered`.
fn synth_future(
    client: &TtsClient,
    cfg: &ConversationConfig,
    sentence: &str,
) -> Option<impl std::future::Future<Output = Result<Vec<u8>, TtsError>> + 'static> {
    // Verbatim speaks markup as-is; every other scope strips to prose. (A
    // "final summary" can't be known mid-stream, so it degrades to prose here.)
    let text = match cfg.speak_scope {
        SpeakScope::Verbatim => sentence.to_string(),
        _ => spoken_text(&[sentence.to_string()], SpeakScope::ProseOnly)?,
    };
    if text.trim().is_empty() {
        return None;
    }
    tracing::debug!(target: "conversation", "speak: {:?}", text.chars().take(60).collect::<String>());
    let client = client.clone();
    let model = cfg.model.clone();
    let voice = cfg.voice.clone().unwrap_or_default();
    let response_format = cfg.response_format.clone();
    let speed = cfg.speed;
    let char_len = text.chars().count();
    Some(async move {
        let req = SpeechRequest {
            model: &model,
            input: &text,
            voice: &voice,
            response_format: &response_format,
            speed,
        };
        // Stage 3a timing: TTS synthesis request (text → encoded audio bytes).
        let t0 = std::time::Instant::now();
        let result = client.synthesize(&req).await;
        if let Ok(bytes) = &result {
            tracing::debug!(
                target: "conversation",
                "timing [tts] synth {char_len} chars -> {} bytes in {} ms",
                bytes.len(),
                t0.elapsed().as_millis()
            );
        }
        result
    })
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
    fn accumulator_speaks_list_lines_as_they_complete() {
        // A list with no sentence terminators must still be spoken line-by-line
        // (via colon + newline boundaries), not buffered until a closing period.
        let mut acc = SentenceAccumulator::new();
        let mut spoken = Vec::new();
        spoken.extend(acc.push("Here are the items:\n"));
        spoken.extend(acc.push("- first item\n"));
        spoken.extend(acc.push("- second item\n"));
        assert_eq!(
            spoken,
            vec!["Here are the items:", "- first item", "- second item"]
        );
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
    fn speaker_command_routing() {
        use crate::conversation::ConversationEvent as E;
        assert_eq!(
            speaker_command_for(&E::Delta("hi".into())),
            Some(SpeakerCommand::Chunk("hi".into()))
        );
        assert_eq!(speaker_command_for(&E::Break), Some(SpeakerCommand::Flush));
        assert_eq!(
            speaker_command_for(&E::TurnComplete),
            Some(SpeakerCommand::Flush)
        );
        assert_eq!(
            speaker_command_for(&E::Error("x".into())),
            Some(SpeakerCommand::Flush)
        );
        assert_eq!(
            speaker_command_for(&E::Started {
                session_id: "s".into()
            }),
            None
        );
        assert_eq!(speaker_command_for(&E::Exited), None);
    }

    #[test]
    fn mute_drops_chunks_until_resume() {
        let mut s = SpeakerState::default();
        // Normal streaming accumulates and flushes.
        assert_eq!(
            s.on(&SpeakerCommand::Chunk("hi".into())),
            SpeakerEffect::Accumulate
        );
        // The user interjects: stop + mute.
        assert_eq!(
            s.on(&SpeakerCommand::Interrupt),
            SpeakerEffect::InterruptAndMute
        );
        // Trailing deltas of the old reply are dropped while muted.
        assert_eq!(
            s.on(&SpeakerCommand::Chunk("more".into())),
            SpeakerEffect::Ignore
        );
        assert_eq!(s.on(&SpeakerCommand::Flush), SpeakerEffect::Ignore);
        // The new query is submitted → unmute.
        assert_eq!(s.on(&SpeakerCommand::Resume), SpeakerEffect::Unmute);
        // The new reply speaks normally.
        assert_eq!(
            s.on(&SpeakerCommand::Chunk("new".into())),
            SpeakerEffect::Accumulate
        );
    }

    #[test]
    fn interrupt_then_resume_clears_mute() {
        let mut s = SpeakerState::default();
        assert_eq!(
            s.on(&SpeakerCommand::Interrupt),
            SpeakerEffect::InterruptAndMute
        );
        assert_eq!(s.on(&SpeakerCommand::Resume), SpeakerEffect::Unmute);
        // Flush is honoured again once unmuted.
        assert_eq!(s.on(&SpeakerCommand::Flush), SpeakerEffect::FlushBuffer);
    }

    #[test]
    fn resume_without_interrupt_is_noop() {
        let mut s = SpeakerState::default();
        // Resume on a fresh (unmuted) state just clears the (already-clear) flag.
        assert_eq!(s.on(&SpeakerCommand::Resume), SpeakerEffect::Unmute);
        assert_eq!(
            s.on(&SpeakerCommand::Chunk("hi".into())),
            SpeakerEffect::Accumulate
        );
    }

    #[test]
    fn interrupt_while_idle_is_safe() {
        let mut s = SpeakerState::default();
        // Interrupting with nothing playing still mutes (the loop's clear/stop are
        // no-ops on empty state); a later Resume restores normal accumulation.
        assert_eq!(
            s.on(&SpeakerCommand::Interrupt),
            SpeakerEffect::InterruptAndMute
        );
        assert_eq!(
            s.on(&SpeakerCommand::Chunk("x".into())),
            SpeakerEffect::Ignore
        );
        assert_eq!(s.on(&SpeakerCommand::Resume), SpeakerEffect::Unmute);
        assert_eq!(
            s.on(&SpeakerCommand::Chunk("y".into())),
            SpeakerEffect::Accumulate
        );
    }

    #[test]
    fn accumulator_does_not_split_abbreviations() {
        let mut acc = SentenceAccumulator::new();
        // "e.g." must not be treated as a sentence end.
        let out = acc.push("Use it, e.g. like this. Next");
        assert_eq!(out, vec!["Use it, e.g. like this."]);
    }
}
