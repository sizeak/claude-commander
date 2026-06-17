//! Conversation mode: speak the commander agent's replies via an
//! OpenAI-compatible TTS engine.
//!
//! The [`ConversationWorker`] tails the commander's Claude Code transcript (see
//! [`transcript`]), extracts speakable prose from each new assistant turn (see
//! [`extract`]), and synthesizes it sentence-by-sentence ([`tts`]), queuing each
//! clip for gapless, interruptible playback ([`audio`]). All the parsing/text
//! logic is pure and unit-tested; only [`audio`] touches a device.

pub mod audio;
pub mod extract;
pub mod transcript;
pub mod tts;

pub use extract::{SpeakScope, split_sentences, spoken_text};
pub use transcript::{
    AssistantTurn, TranscriptTail, encode_project_dir, latest_transcript, transcript_dir,
};
pub use tts::{SpeechRequest, TtsClient, build_speech_body};

use std::path::PathBuf;

use tracing::debug;

use crate::config::ConversationConfig;
use crate::error::TtsError;
use audio::Player;

/// Owns the warm TTS client + audio player and tails the transcript. Created
/// when conversation mode is switched on, dropped when switched off.
pub struct ConversationWorker {
    tail: TranscriptTail,
    client: TtsClient,
    player: Player,
    dir: PathBuf,
    cfg: ConversationConfig,
}

impl ConversationWorker {
    /// Build a worker for the commander transcript at `dir`. Seeds the tail to
    /// the end of the current transcript so existing backlog isn't replayed.
    pub fn new(cfg: ConversationConfig, dir: PathBuf) -> Result<Self, TtsError> {
        let player = Player::new(cfg.volume)?;
        let client = TtsClient::new(cfg.base_url.clone());
        let mut tail = TranscriptTail::new();
        tail.seed_to_end(&dir);
        Ok(Self {
            tail,
            client,
            player,
            dir,
            cfg,
        })
    }

    /// One poll. If a new assistant turn appeared, interrupt any current
    /// playback and speak it sentence-by-sentence (synthesis pipelines ahead of
    /// playback). A newer turn arriving mid-reply preempts at a sentence
    /// boundary. Returns `Ok(true)` if anything was spoken.
    pub async fn tick(&mut self) -> Result<bool, TtsError> {
        let mut next = self.tail.poll(&self.dir);
        let mut spoke = false;
        let voice = self.cfg.voice.clone().unwrap_or_default();

        while let Some(turn) = next.take() {
            let Some(text) = spoken_text(&turn.text_blocks, self.cfg.speak_scope) else {
                // Nothing speakable (e.g. pure code) — see if a newer turn waits.
                next = self.tail.poll(&self.dir);
                continue;
            };
            // New turn → interrupt whatever is playing.
            self.player.stop();
            spoke = true;
            debug!(uuid = %turn.uuid, "speaking commander turn");

            for sentence in split_sentences(&text) {
                // A newer turn appeared → preempt and restart with it.
                if let Some(newer) = self.tail.poll(&self.dir) {
                    next = Some(newer);
                    break;
                }
                let req = SpeechRequest {
                    model: &self.cfg.model,
                    input: &sentence,
                    voice: &voice,
                    response_format: &self.cfg.response_format,
                    speed: self.cfg.speed,
                };
                match self.client.synthesize(&req).await {
                    Ok(bytes) => self.player.enqueue(bytes),
                    // Degrade gracefully: log and keep going (don't block the UI).
                    Err(e) => tracing::warn!("TTS synthesis failed: {e}"),
                }
            }
        }
        Ok(spoke)
    }
}
