//! Voice-input listener: owns the mic [`Recorder`] and [`SttClient`].
//!
//! Mirrors [`speaker::spawn_speaker`](crate::conversation::speaker::spawn_speaker),
//! but in the opposite direction — instead of turning text into audio, it turns
//! a finished recording into text. The app toggles capture with
//! [`ListenerCommand`]s; when a recording stops, the captured WAV is transcribed
//! and the resulting transcript is sent on `transcript_tx` for the app to feed
//! to the conversation session.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::config::SttConfig;
use crate::conversation::media::{MediaSignal, signal as media_signal};
use crate::conversation::recorder::Recorder;
use crate::conversation::speaker::{SpeakerCommand, SpeakerHandle};
use crate::conversation::stt::SttClient;

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

/// A respawn-stable handle to the current voice listener's command channel.
///
/// The listener's [`Recorder`] binds its microphone once at spawn time, so
/// changing the selected mic means tearing the listener down and building a
/// fresh one. Every voice trigger (the Alt-V key path, the in-attach byte
/// interceptor, and the Unix-socket toggle) reaches the listener *through* this
/// handle rather than holding a raw `Sender` clone, so a [`replace`] swaps the
/// device under all of them at once. Dropping the previous `Sender` (there is
/// only ever the one, held here) ends the old listener task — which in turn ends
/// its recorder thread, releasing the old device.
///
/// [`replace`]: ListenerHandle::replace
#[derive(Clone, Default)]
pub struct ListenerHandle(Arc<Mutex<Option<mpsc::UnboundedSender<ListenerCommand>>>>);

impl ListenerHandle {
    /// Install (or swap in) the current listener's command sender, dropping any
    /// previous one.
    pub fn replace(&self, tx: mpsc::UnboundedSender<ListenerCommand>) {
        *self.0.lock().unwrap() = Some(tx);
    }

    /// Whether a listener is currently installed.
    pub fn is_present(&self) -> bool {
        self.0.lock().unwrap().is_some()
    }

    /// Send a command to the current listener. Returns `false` if none is
    /// installed or the listener task has gone away.
    pub fn send(&self, cmd: ListenerCommand) -> bool {
        matches!(&*self.0.lock().unwrap(), Some(tx) if tx.send(cmd).is_ok())
    }
}

impl From<mpsc::UnboundedSender<ListenerCommand>> for ListenerHandle {
    fn from(tx: mpsc::UnboundedSender<ListenerCommand>) -> Self {
        Self(Arc::new(Mutex::new(Some(tx))))
    }
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
    listener: &ListenerHandle,
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

/// Start the listener task and return its [`ListenerCommand`] sender
/// *immediately* — the microphone device is opened inside the task (awaiting the
/// recorder's readiness), so a slow device never blocks the caller. Commands
/// sent before the device finishes opening queue on the channel and run once it
/// is ready. If no input device is available the task logs and exits, leaving
/// the returned sender inert. Recognized transcripts are sent on
/// `transcript_tx`; dropping the sender ends the task and releases the mic.
pub fn spawn_listener(
    cfg: SttConfig,
    transcript_tx: mpsc::UnboundedSender<String>,
    gate: Option<mpsc::UnboundedSender<MediaSignal>>,
    speaker: SpeakerHandle,
) -> mpsc::UnboundedSender<ListenerCommand> {
    let (tx, mut rx) = mpsc::unbounded_channel::<ListenerCommand>();

    tokio::spawn(async move {
        let (wav_tx, mut wav_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        // Open the mic off the caller's task; on failure the sender goes inert.
        let recorder = match Recorder::new(wav_tx, cfg.input_device.clone()).await {
            Ok(r) => r,
            Err(e) => {
                warn!("STT unavailable: {e}");
                return;
            }
        };
        let client = SttClient::new(&cfg);
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

    tx
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
        let handle = ListenerHandle::from(tx);
        let recording = AtomicBool::new(false);

        // First toggle starts recording.
        assert!(apply_listen_action(
            &handle,
            &recording,
            ListenAction::Toggle
        ));
        assert!(recording.load(Ordering::Acquire));
        assert!(matches!(
            drain(&mut rx).as_slice(),
            [ListenerCommand::Start]
        ));

        // Second toggle stops it.
        assert!(!apply_listen_action(
            &handle,
            &recording,
            ListenAction::Toggle
        ));
        assert!(!recording.load(Ordering::Acquire));
        assert!(matches!(drain(&mut rx).as_slice(), [ListenerCommand::Stop]));
    }

    #[test]
    fn explicit_start_stop_are_idempotent() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = ListenerHandle::from(tx);
        let recording = AtomicBool::new(false);

        // Start from idle records and sends Start.
        assert!(apply_listen_action(
            &handle,
            &recording,
            ListenAction::Start
        ));
        assert!(matches!(
            drain(&mut rx).as_slice(),
            [ListenerCommand::Start]
        ));

        // A second Start is a no-op — no command, no restart.
        assert!(apply_listen_action(
            &handle,
            &recording,
            ListenAction::Start
        ));
        assert!(drain(&mut rx).is_empty());

        // Stop sends Stop; a second Stop is a no-op.
        assert!(!apply_listen_action(
            &handle,
            &recording,
            ListenAction::Stop
        ));
        assert!(matches!(drain(&mut rx).as_slice(), [ListenerCommand::Stop]));
        assert!(!apply_listen_action(
            &handle,
            &recording,
            ListenAction::Stop
        ));
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn replace_swaps_the_delivered_to_sender() {
        // The core respawn guarantee: after `replace`, triggers routed through
        // the (cloned) handle reach the *new* listener, not the old one — this
        // is what lets a mic change take effect live under the IPC/attach
        // clones that still hold their original handle clone.
        let (tx1, mut rx1) = mpsc::unbounded_channel();
        let handle = ListenerHandle::from(tx1);
        let shared = handle.clone(); // as the IPC/attach tasks would hold it
        let recording = AtomicBool::new(false);

        let (tx2, mut rx2) = mpsc::unbounded_channel();
        handle.replace(tx2);

        // A trigger via the pre-existing clone now lands on the new receiver.
        assert!(apply_listen_action(
            &shared,
            &recording,
            ListenAction::Start
        ));
        assert!(drain(&mut rx1).is_empty());
        assert!(matches!(
            drain(&mut rx2).as_slice(),
            [ListenerCommand::Start]
        ));
    }
}
