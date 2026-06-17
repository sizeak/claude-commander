//! In-process audio playback via `rodio`, on a dedicated thread.
//!
//! rodio's `OutputStream` (a cpal stream) is `!Send`, so it can't live inside
//! the async watcher task. Instead the stream + sink live on their own thread
//! and we drive them with commands over a channel. The [`Player`] handle is
//! `Send` (it only holds a channel sender), so the worker can hold it across
//! `.await` points. Dropping the `Player` ends the thread and stops audio.

use std::io::Cursor;
use std::sync::mpsc::{Receiver, Sender, channel};

use rodio::{OutputStream, Sink};
use tracing::warn;

use crate::error::TtsError;

enum Command {
    Enqueue(Vec<u8>),
    Stop,
}

/// A `Send` handle to the audio thread. Clips enqueued play back-to-back; `stop`
/// clears the queue (used to interrupt on a new reply / when toggled off).
pub struct Player {
    tx: Sender<Command>,
}

impl Player {
    /// Start the audio thread. Returns an error synchronously if no output
    /// device is available.
    pub fn new(volume: f32) -> Result<Self, TtsError> {
        let (tx, rx) = channel::<Command>();
        let (ready_tx, ready_rx) = channel::<Result<(), String>>();
        std::thread::Builder::new()
            .name("cc-tts-audio".into())
            .spawn(move || audio_thread(rx, volume, ready_tx))
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

fn audio_thread(rx: Receiver<Command>, volume: f32, ready: Sender<Result<(), String>>) {
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

    while let Ok(cmd) = rx.recv() {
        match cmd {
            Command::Enqueue(bytes) => match rodio::Decoder::new(Cursor::new(bytes)) {
                Ok(decoder) => sink.append(decoder),
                Err(e) => warn!("TTS audio decode failed: {e}"),
            },
            Command::Stop => {
                // A stopped Sink can't accept new sources, so replace it.
                sink.stop();
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
}
