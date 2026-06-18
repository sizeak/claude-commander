//! Voice-input listener: owns the mic [`Recorder`] and [`SttClient`].
//!
//! Mirrors [`speaker::spawn_speaker`](crate::conversation::speaker::spawn_speaker),
//! but in the opposite direction — instead of turning text into audio, it turns
//! a finished recording into text. The app toggles capture with
//! [`ListenerCommand`]s; when a recording stops, the captured WAV is transcribed
//! and the resulting transcript is sent on `transcript_tx` for the app to feed
//! to the conversation session.

use std::time::Instant;

use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::config::SttConfig;
use crate::conversation::recorder::Recorder;
use crate::conversation::stt::SttClient;
use crate::error::TtsError;

/// Commands to the listener task.
#[derive(Debug, Clone)]
pub enum ListenerCommand {
    /// Begin recording the microphone.
    Start,
    /// Stop recording and transcribe what was captured.
    Stop,
}

/// Start the listener task. Returns a sender for [`ListenerCommand`]s, or an
/// error if no input device is available. Recognized transcripts are sent on
/// `transcript_tx`. Dropping the command sender ends the task and releases the
/// microphone.
pub fn spawn_listener(
    cfg: SttConfig,
    transcript_tx: mpsc::UnboundedSender<String>,
) -> Result<mpsc::UnboundedSender<ListenerCommand>, TtsError> {
    let (wav_tx, mut wav_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let recorder = Recorder::new(wav_tx)?;
    let client = SttClient::new(&cfg);
    let (tx, mut rx) = mpsc::unbounded_channel::<ListenerCommand>();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                // A finished recording arrived from the recorder thread.
                Some(wav) = wav_rx.recv() => {
                    // Stage 1 timing: WAV bytes in → transcript out.
                    let wav_bytes = wav.len();
                    let t0 = Instant::now();
                    match client.transcribe(wav).await {
                        Ok(text) if !text.is_empty() => {
                            debug!(
                                target: "conversation",
                                "timing [stt] transcribed {wav_bytes} byte WAV in {} ms ({} chars)",
                                t0.elapsed().as_millis(),
                                text.len()
                            );
                            if transcript_tx.send(text).is_err() {
                                break; // app gone
                            }
                        }
                        // Empty transcript (silence) — nothing to send, but still
                        // worth timing so a slow "no speech" round-trip is visible.
                        Ok(_) => debug!(
                            target: "conversation",
                            "timing [stt] transcribed {wav_bytes} byte WAV in {} ms (empty — silence)",
                            t0.elapsed().as_millis()
                        ),
                        Err(e) => warn!("STT transcription failed: {e}"),
                    }
                },
                cmd = rx.recv() => match cmd {
                    Some(ListenerCommand::Start) => recorder.start(),
                    Some(ListenerCommand::Stop) => recorder.stop(),
                    None => break, // sender dropped → end the task (releases mic)
                },
            }
        }
    });

    Ok(tx)
}
