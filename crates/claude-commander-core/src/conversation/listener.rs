//! Voice-input listener: owns the mic [`Recorder`] and [`SttClient`].
//!
//! Mirrors [`speaker::spawn_speaker`](crate::conversation::speaker::spawn_speaker),
//! but in the opposite direction — instead of turning text into audio, it turns
//! a finished recording into text. The app toggles capture with
//! [`ListenerCommand`]s; when a recording stops, the captured WAV is transcribed
//! and the resulting transcript is sent on `transcript_tx` for the app to feed
//! to the conversation session.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::config::SttConfig;
use crate::conversation::media::{MediaSignal, signal as media_signal};
use crate::conversation::recorder::Recorder;
use crate::conversation::speaker::{SpeakerCommand, SpeakerHandle};
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

/// What an external trigger wants the microphone to do. `Toggle` flips the
/// current state; `Start`/`Stop` request an absolute state (a no-op if already
/// there).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenAction {
    Toggle,
    Start,
    Stop,
}

/// Apply a [`ListenAction`] against the shared recording flag and listener.
///
/// This is the single shared entry point for *every* voice trigger — the in-app
/// Alt-V key path, the in-attach byte interceptor, and the external Unix-socket
/// toggle — so the recording state machine stays consistent no matter where the
/// toggle originates. Returns the new recording state. Sends a
/// [`ListenerCommand`] only when the state actually changes (so a redundant
/// `Start`/`Stop` doesn't restart capture or transcribe silence).
pub fn apply_listen_action(
    listener: &mpsc::UnboundedSender<ListenerCommand>,
    recording: &AtomicBool,
    action: ListenAction,
) -> bool {
    let now = match action {
        // Atomic flip so concurrent triggers can't both observe the old value.
        ListenAction::Toggle => !recording.fetch_xor(true, Ordering::AcqRel),
        ListenAction::Start | ListenAction::Stop => {
            let want = matches!(action, ListenAction::Start);
            if recording.swap(want, Ordering::AcqRel) == want {
                return want; // already in the desired state — nothing to send
            }
            want
        }
    };
    let _ = listener.send(if now {
        ListenerCommand::Start
    } else {
        ListenerCommand::Stop
    });
    now
}

/// Start the listener task. Returns a sender for [`ListenerCommand`]s, or an
/// error if no input device is available. Recognized transcripts are sent on
/// `transcript_tx`. Dropping the command sender ends the task and releases the
/// microphone.
pub fn spawn_listener(
    cfg: SttConfig,
    transcript_tx: mpsc::UnboundedSender<String>,
    gate: Option<mpsc::UnboundedSender<MediaSignal>>,
    speaker: SpeakerHandle,
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
                        // No reply is coming, so let any paused media resume now.
                        Ok(_) => {
                            debug!(
                                target: "conversation",
                                "timing [stt] transcribed {wav_bytes} byte WAV in {} ms (empty — silence)",
                                t0.elapsed().as_millis()
                            );
                            media_signal(&gate, MediaSignal::Silence);
                            // Nothing was said, so no new message will be submitted
                            // to lift the mute set on record-start — clear it here.
                            speaker.send(SpeakerCommand::Resume);
                        }
                        Err(e) => {
                            warn!("STT transcription failed: {e}");
                            // Failed round-trip → no reply either; don't strand media.
                            media_signal(&gate, MediaSignal::Silence);
                            // No submit will follow to unmute the speaker — do it here.
                            speaker.send(SpeakerCommand::Resume);
                        }
                    }
                },
                cmd = rx.recv() => match cmd {
                    Some(ListenerCommand::Start) => {
                        // The user is starting a new message — stop speaking the
                        // current reply at once and stay muted until the new query
                        // is submitted (a no-op when nothing is speaking).
                        speaker.send(SpeakerCommand::Interrupt);
                        // Signal *before* opening the mic: the gate snapshots the
                        // playing players concurrently, and on Bluetooth the mic
                        // opening only pauses playback ~300ms later — so the
                        // snapshot reads "Playing" before the device pause lands.
                        media_signal(&gate, MediaSignal::RecordStarted);
                        recorder.start();
                    }
                    Some(ListenerCommand::Stop) => {
                        recorder.stop();
                        media_signal(&gate, MediaSignal::RecordStopped);
                    }
                    None => break, // sender dropped → end the task (releases mic)
                },
            }
        }
    });

    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drain whatever commands are queued on the listener channel.
    fn drain(rx: &mut mpsc::UnboundedReceiver<ListenerCommand>) -> Vec<ListenerCommand> {
        let mut out = Vec::new();
        while let Ok(cmd) = rx.try_recv() {
            out.push(cmd);
        }
        out
    }

    #[test]
    fn toggle_alternates_start_and_stop() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let recording = AtomicBool::new(false);

        // First toggle starts recording.
        assert!(apply_listen_action(&tx, &recording, ListenAction::Toggle));
        assert!(recording.load(Ordering::Acquire));
        assert!(matches!(
            drain(&mut rx).as_slice(),
            [ListenerCommand::Start]
        ));

        // Second toggle stops it.
        assert!(!apply_listen_action(&tx, &recording, ListenAction::Toggle));
        assert!(!recording.load(Ordering::Acquire));
        assert!(matches!(drain(&mut rx).as_slice(), [ListenerCommand::Stop]));
    }

    #[test]
    fn explicit_start_stop_are_idempotent() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let recording = AtomicBool::new(false);

        // Start from idle records and sends Start.
        assert!(apply_listen_action(&tx, &recording, ListenAction::Start));
        assert!(matches!(
            drain(&mut rx).as_slice(),
            [ListenerCommand::Start]
        ));

        // A second Start is a no-op — no command, no restart.
        assert!(apply_listen_action(&tx, &recording, ListenAction::Start));
        assert!(drain(&mut rx).is_empty());

        // Stop sends Stop; a second Stop is a no-op.
        assert!(!apply_listen_action(&tx, &recording, ListenAction::Stop));
        assert!(matches!(drain(&mut rx).as_slice(), [ListenerCommand::Stop]));
        assert!(!apply_listen_action(&tx, &recording, ListenAction::Stop));
        assert!(drain(&mut rx).is_empty());
    }
}
