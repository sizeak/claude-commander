//! Microphone capture via `cpal`, on a dedicated thread.
//!
//! Mirrors [`audio::Player`](crate::conversation::audio::Player): a `cpal`
//! stream is `!Send`, so it can't live in the async listener task. Instead the
//! input device + stream live on their own thread driven by commands over a
//! channel. The [`Recorder`] handle is `Send` (it only holds a channel sender),
//! so the listener can hold it across `.await`. Dropping the `Recorder` ends the
//! thread and stops capture.
//!
//! We record at the device's native sample rate, downmix to mono, and encode
//! 16-bit PCM WAV — the transcription server resamples as needed, so we don't
//! force a (possibly unsupported) capture rate on the device.

use tokio::sync::mpsc::UnboundedSender;

use crate::error::TtsError;

#[cfg(feature = "audio")]
use std::io::Cursor;
#[cfg(feature = "audio")]
use std::sync::mpsc::{Receiver, Sender, channel};
#[cfg(feature = "audio")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "audio")]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
#[cfg(feature = "audio")]
use cpal::{FromSample, Sample, SizedSample};
#[cfg(feature = "audio")]
use tracing::warn;

#[cfg(feature = "audio")]
enum Command {
    Start,
    Stop,
}

/// A `Send` handle to the recorder thread. `start` begins capture; `stop` ends
/// it and sends the encoded WAV bytes on the channel given to [`Recorder::new`].
#[cfg(feature = "audio")]
pub struct Recorder {
    tx: Sender<Command>,
}

#[cfg(feature = "audio")]
impl Recorder {
    /// Start the recorder thread. Returns an error synchronously if no input
    /// device is available. Finished recordings are delivered as WAV bytes on
    /// `wav_tx`. `input_device` names the microphone to capture from; `None`
    /// (or a name that isn't present) falls back to the system default.
    pub fn new(
        wav_tx: UnboundedSender<Vec<u8>>,
        input_device: Option<String>,
    ) -> Result<Self, TtsError> {
        let (tx, rx) = channel::<Command>();
        let (ready_tx, ready_rx) = channel::<Result<(), String>>();
        std::thread::Builder::new()
            .name("cc-stt-audio".into())
            .spawn(move || recorder_thread(rx, wav_tx, ready_tx, input_device))
            .map_err(|e| TtsError::Audio(e.to_string()))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { tx }),
            Ok(Err(e)) => Err(TtsError::Audio(e)),
            Err(e) => Err(TtsError::Audio(e.to_string())),
        }
    }

    /// Begin (or restart) capturing into a fresh buffer.
    pub fn start(&self) {
        let _ = self.tx.send(Command::Start);
    }

    /// Stop capturing and emit the recording as WAV on the bytes channel.
    pub fn stop(&self) {
        let _ = self.tx.send(Command::Stop);
    }
}

/// Human-readable name of a cpal device, or `None` if it can't be queried.
/// cpal 0.18 exposes this via `Device::description().name()` (the old `name()`
/// accessor was removed). Under the PipeWire host this is a friendly label
/// (e.g. "Built-in Audio Analog Stereo") rather than a raw ALSA alias.
#[cfg(feature = "audio")]
fn device_display_name(device: &cpal::Device) -> Option<String> {
    device.description().ok().map(|d| d.name().to_string())
}

/// Given the names of the available input devices and an optionally-requested
/// device name, decide which name to open. Returns `None` — meaning "use the
/// system default input device" — when nothing was requested, or when the
/// requested device is no longer present (graceful fallback rather than error).
#[cfg(feature = "audio")]
fn resolve_input_device_name(available: &[String], requested: Option<&str>) -> Option<String> {
    match requested {
        Some(name) if available.iter().any(|a| a == name) => Some(name.to_string()),
        _ => None,
    }
}

/// Names of the available input (capture) devices, for the settings picker.
/// Empty when the `audio` feature is off, or on an enumeration error. This
/// synchronously queries the audio host, so it's called only on demand (when
/// the user opens the microphone picker), not on a hot path.
#[cfg(feature = "audio")]
pub fn input_device_names() -> Vec<String> {
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(devices) => devices.filter_map(|d| device_display_name(&d)).collect(),
        Err(e) => {
            warn!("failed to enumerate input devices: {e}");
            Vec::new()
        }
    }
}

/// Placeholder used when the `audio` feature is off. The `tui` settings module
/// is compiled in every build (server, client cdylib) and calls this, so it
/// must exist even where cpal isn't linked; there are simply no devices.
#[cfg(not(feature = "audio"))]
pub fn input_device_names() -> Vec<String> {
    Vec::new()
}

#[cfg(feature = "audio")]
fn recorder_thread(
    rx: Receiver<Command>,
    wav_tx: UnboundedSender<Vec<u8>>,
    ready: Sender<Result<(), String>>,
    input_device: Option<String>,
) {
    let host = cpal::default_host();
    // Enumerate once: derive the name list from the same devices we search, so a
    // device whose `name()` errors is simply never matched (no index skew), and
    // we don't probe the (slow) audio host twice.
    let devices: Vec<cpal::Device> = host
        .input_devices()
        .map(|it| it.collect())
        .unwrap_or_default();
    let names: Vec<String> = devices.iter().filter_map(device_display_name).collect();
    let chosen = resolve_input_device_name(&names, input_device.as_deref());
    if input_device.is_some() && chosen.is_none() {
        warn!("STT: requested microphone not found, using default input device");
    }
    let device = match &chosen {
        Some(name) => devices
            .into_iter()
            .find(|d| device_display_name(d).as_deref() == Some(name.as_str()))
            .or_else(|| host.default_input_device()),
        None => host.default_input_device(),
    };
    let Some(device) = device else {
        let _ = ready.send(Err("no input audio device available".into()));
        return;
    };
    let supported = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    let _ = ready.send(Ok(()));

    let sample_rate = supported.sample_rate();
    let channels = supported.channels();
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();

    // Captured mono f32 samples for the in-progress recording.
    let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
    let mut stream: Option<cpal::Stream> = None;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            Command::Start => {
                buffer.lock().unwrap().clear();
                match build_stream(&device, &config, sample_format, channels, buffer.clone()) {
                    Ok(s) => match s.play() {
                        Ok(()) => stream = Some(s),
                        Err(e) => warn!("STT mic playback failed: {e}"),
                    },
                    Err(e) => warn!("STT mic capture failed: {e}"),
                }
            }
            Command::Stop => {
                // Drop the active stream to stop the capture callback before
                // draining (`.take()` so the held guard counts as read).
                drop(stream.take());
                let samples = std::mem::take(&mut *buffer.lock().unwrap());
                match encode_wav(&samples, sample_rate) {
                    Ok(bytes) => {
                        if wav_tx.send(bytes).is_err() {
                            break; // listener gone
                        }
                    }
                    Err(e) => warn!("STT WAV encode failed: {e}"),
                }
            }
        }
    }
}

/// Build an input stream for the device's sample format, downmixing to mono.
#[cfg(feature = "audio")]
fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    format: cpal::SampleFormat,
    channels: u16,
    buffer: Arc<Mutex<Vec<f32>>>,
) -> Result<cpal::Stream, String> {
    match format {
        cpal::SampleFormat::F32 => build_typed::<f32>(device, config, channels, buffer),
        cpal::SampleFormat::I16 => build_typed::<i16>(device, config, channels, buffer),
        cpal::SampleFormat::U16 => build_typed::<u16>(device, config, channels, buffer),
        other => return Err(format!("unsupported sample format: {other:?}")),
    }
    .map_err(|e| e.to_string())
}

#[cfg(feature = "audio")]
fn build_typed<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: u16,
    buffer: Arc<Mutex<Vec<f32>>>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let ch = channels.max(1) as usize;
    device.build_input_stream(
        *config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let mut buf = buffer.lock().unwrap();
            for frame in data.chunks(ch) {
                let sum: f32 = frame.iter().map(|s| f32::from_sample(*s)).sum();
                buf.push(sum / ch as f32);
            }
        },
        |err| warn!("STT audio stream error: {err}"),
        None,
    )
}

/// Encode mono f32 samples (`-1.0..=1.0`) as 16-bit PCM WAV bytes.
#[cfg(feature = "audio")]
pub(crate) fn encode_wav(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>, String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::<u8>::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec).map_err(|e| e.to_string())?;
        for &s in samples {
            let amplitude = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            writer.write_sample(amplitude).map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;
    }
    Ok(cursor.into_inner())
}

/// Placeholder [`Recorder`] compiled when the `audio` feature is off (headless
/// server, client cdylib). Construction fails rather than silently no-opping;
/// these builds never spawn the voice listener, so the error path is unreachable
/// in practice.
#[cfg(not(feature = "audio"))]
pub struct Recorder;

#[cfg(not(feature = "audio"))]
impl Recorder {
    pub fn new(
        _wav_tx: UnboundedSender<Vec<u8>>,
        _input_device: Option<String>,
    ) -> Result<Self, TtsError> {
        Err(TtsError::Audio("audio support not compiled in".into()))
    }

    pub fn start(&self) {}

    pub fn stop(&self) {}
}

#[cfg(all(test, feature = "audio"))]
mod tests {
    use super::*;

    #[test]
    fn encode_wav_roundtrips_spec_and_samples() {
        let samples = [0.0_f32, 0.5, -0.5, 1.0, -1.0];
        let bytes = encode_wav(&samples, 16_000).expect("encode");

        let reader = hound::WavReader::new(Cursor::new(bytes)).expect("read");
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 16_000);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);

        let read: Vec<i16> = reader
            .into_samples::<i16>()
            .collect::<Result<_, _>>()
            .expect("samples");
        let expected: Vec<i16> = samples
            .iter()
            .map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect();
        assert_eq!(read, expected);
    }

    #[test]
    fn encode_wav_empty_is_valid() {
        let bytes = encode_wav(&[], 16_000).expect("encode empty");
        let reader = hound::WavReader::new(Cursor::new(bytes)).expect("read");
        assert_eq!(reader.len(), 0);
    }

    #[test]
    fn resolve_input_device_name_cases() {
        let available = vec!["Built-in".to_string(), "USB Mic".to_string()];
        // Requested device present → selected verbatim.
        assert_eq!(
            resolve_input_device_name(&available, Some("USB Mic")),
            Some("USB Mic".to_string())
        );
        // Requested device absent → None (fall back to system default).
        assert_eq!(resolve_input_device_name(&available, Some("Gone")), None);
        // Nothing requested → None (system default).
        assert_eq!(resolve_input_device_name(&available, None), None);
        // Empty device list with a request → None.
        assert_eq!(resolve_input_device_name(&[], Some("USB Mic")), None);
    }
}
