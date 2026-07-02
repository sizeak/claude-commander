//! In-process audio playback via `rodio`, on a dedicated thread.
//!
//! rodio's `OutputStream` (a cpal stream) is `!Send`, so it can't live inside
//! the async watcher task. Instead the stream + sink live on their own thread
//! and we drive them with commands over a channel. The [`Player`] handle is
//! `Send` (it only holds a channel sender), so the worker can hold it across
//! `.await` points. Dropping the `Player` ends the thread and stops audio.

use crate::error::TtsError;

#[cfg(feature = "audio")]
use std::io::Cursor;
#[cfg(feature = "audio")]
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
#[cfg(feature = "audio")]
use std::time::{Duration, Instant};

#[cfg(feature = "audio")]
use rodio::{OutputStream, Sink};
#[cfg(feature = "audio")]
use tracing::{debug, warn};

/// A playback span boundary, reported (if a listener was given) so a caller can
/// track when audio is actually coming out of the speaker. `Started` fires when
/// audio begins from an idle sink; `Stopped` when the sink drains (or is
/// stopped). Synthesis gaps produce separate spans, so these can repeat within
/// one logical reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackEdge {
    Started,
    Stopped,
}

#[cfg(feature = "audio")]
enum Command {
    Enqueue(Vec<u8>),
    Stop,
}

/// A `Send` handle to the audio thread. Clips enqueued play back-to-back; `stop`
/// clears the queue (used to interrupt on a new reply / when toggled off).
#[cfg(feature = "audio")]
pub struct Player {
    tx: Sender<Command>,
}

#[cfg(feature = "audio")]
impl Player {
    /// Start the audio thread. Returns an error synchronously if no output
    /// device is available.
    pub fn new(volume: f32) -> Result<Self, TtsError> {
        Self::with_edges(volume, None)
    }

    /// Like [`Player::new`], but reports playback span boundaries on `edges` so
    /// the caller can tell when audio is actually playing.
    pub fn with_edges(
        volume: f32,
        edges: Option<tokio::sync::mpsc::UnboundedSender<PlaybackEdge>>,
    ) -> Result<Self, TtsError> {
        let (tx, rx) = channel::<Command>();
        let (ready_tx, ready_rx) = channel::<Result<(), String>>();
        std::thread::Builder::new()
            .name("cc-tts-audio".into())
            .spawn(move || audio_thread(rx, volume, ready_tx, edges))
            .map_err(|e| TtsError::Audio(e.to_string()))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { tx }),
            Ok(Err(e)) => Err(TtsError::Audio(e)),
            Err(e) => Err(TtsError::Audio(e.to_string())),
        }
    }

    /// Queue an encoded audio clip (wav/mp3/…) for gapless playback.
    pub fn enqueue(&self, bytes: Vec<u8>) {
        let _ = self.tx.send(Command::Enqueue(bytes));
    }

    /// Stop and clear anything queued or playing.
    pub fn stop(&self) {
        let _ = self.tx.send(Command::Stop);
    }
}

#[cfg(feature = "audio")]
fn audio_thread(
    rx: Receiver<Command>,
    volume: f32,
    ready: Sender<Result<(), String>>,
    edges: Option<tokio::sync::mpsc::UnboundedSender<PlaybackEdge>>,
) {
    let emit = |edge: PlaybackEdge| {
        if let Some(tx) = &edges {
            let _ = tx.send(edge);
        }
    };
    let stream = match OutputStream::try_default() {
        Ok((stream, handle)) => {
            // Keep `stream` alive for the thread's lifetime; build the first sink.
            match Sink::try_new(&handle) {
                Ok(sink) => {
                    sink.set_volume(volume);
                    let _ = ready.send(Ok(()));
                    (stream, handle, sink)
                }
                Err(e) => {
                    let _ = ready.send(Err(e.to_string()));
                    return;
                }
            }
        }
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    let (_stream, handle, mut sink) = stream;

    // Stage 3b timing: track each contiguous playback span. `playing_since` is
    // set when audio starts coming out of an idle sink and cleared (with a trace)
    // when the sink drains — so gaps from synthesis falling behind show up as
    // separate spans. While playing we poll on a short timeout to detect the
    // drain; while idle we block normally so there's no busy-wait.
    let mut playing_since: Option<Instant> = None;
    loop {
        let cmd = if playing_since.is_some() {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(cmd) => Some(cmd),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(cmd) => Some(cmd),
                Err(_) => break, // all senders dropped
            }
        };

        if let Some(cmd) = cmd {
            match cmd {
                Command::Enqueue(bytes) => match rodio::Decoder::new(Cursor::new(bytes)) {
                    Ok(decoder) => sink.append(decoder),
                    Err(e) => warn!("TTS audio decode failed: {e}"),
                },
                Command::Stop => {
                    // A stopped Sink can't accept new sources, so replace it.
                    sink.stop();
                    if playing_since.take().is_some() {
                        emit(PlaybackEdge::Stopped);
                    }
                    match Sink::try_new(&handle) {
                        Ok(fresh) => {
                            fresh.set_volume(volume);
                            sink = fresh;
                        }
                        Err(e) => warn!("TTS audio sink reset failed: {e}"),
                    }
                }
            }
        }

        if sink.empty() {
            if let Some(t0) = playing_since.take() {
                debug!(
                    target: "conversation",
                    "timing [tts] playback finished: {} ms of audio played",
                    t0.elapsed().as_millis()
                );
                emit(PlaybackEdge::Stopped);
            }
        } else if playing_since.is_none() {
            playing_since = Some(Instant::now());
            emit(PlaybackEdge::Started);
        }
    }
}

/// Placeholder [`Player`] compiled when the `audio` feature is off (headless
/// server, client cdylib). Construction fails rather than silently no-opping, so
/// voice can't appear to work on a build with no audio backend. These builds
/// never spawn the voice runtime, so the error path is unreachable in practice.
#[cfg(not(feature = "audio"))]
pub struct Player;

#[cfg(not(feature = "audio"))]
impl Player {
    pub fn new(_volume: f32) -> Result<Self, TtsError> {
        Err(TtsError::Audio("audio support not compiled in".into()))
    }

    pub fn with_edges(
        _volume: f32,
        _edges: Option<tokio::sync::mpsc::UnboundedSender<PlaybackEdge>>,
    ) -> Result<Self, TtsError> {
        Err(TtsError::Audio("audio support not compiled in".into()))
    }

    pub fn enqueue(&self, _bytes: Vec<u8>) {}

    pub fn stop(&self) {}
}
