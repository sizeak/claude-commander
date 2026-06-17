//! Conversation mode: a dedicated headless `claude` session whose replies are
//! streamed to a TTS engine (and rendered in a full-screen overlay).
//!
//! The clean, supported way to get the assistant's text incrementally is the
//! stream-json headless protocol (`claude -p --input-format stream-json
//! --output-format stream-json --include-partial-messages`) — the interactive
//! TUI is render-only. [`session`] drives that subprocess; [`extract`] turns the
//! streamed text into spoken-ready sentences; [`tts`] synthesizes them and
//! [`audio`] plays them. All the text logic is pure and unit-tested; only
//! [`audio`] and [`session`] touch the outside world.

pub mod audio;
pub mod extract;
pub mod session;
pub mod speaker;
pub mod tts;

pub use extract::{SpeakScope, split_sentences, spoken_text};
pub use session::{ConversationEvent, ConversationSession, parse_event, user_message_line};
pub use speaker::{SentenceAccumulator, SpeakerCommand, spawn_speaker};
pub use tts::{SpeechRequest, TtsClient, build_speech_body};
