//! OpenAI-compatible speech-to-text (transcription) HTTP client.
//!
//! Posts a `multipart/form-data` body to `{base_url}/audio/transcriptions`
//! (OpenAI's `POST /v1/audio/transcriptions` shape), so it works against any
//! compatible engine — we dev against a local/LAN faster-whisper server.

use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

use crate::config::SttConfig;
use crate::error::TtsError;

/// Thin client around `reqwest`. Cheap to clone (shares the connection pool).
#[derive(Debug, Clone)]
pub struct SttClient {
    http: reqwest::Client,
    base_url: String,
    model: String,
    language: Option<String>,
    prompt: Option<String>,
    api_key: Option<String>,
}

/// Per-request timeout. Generous enough for a slow CPU transcribe of a long
/// utterance, but bounded so a hung server can't wedge voice input — the
/// request errors out, is logged, and the user can try again.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// The transcription response shape (OpenAI returns `{ "text": "..." }`).
#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    text: String,
}

/// The leading `language <LanguageName><asr_text>` framing Qwen3-ASR decodes,
/// e.g. `language English<asr_text>The north wind and the sun...`. The language
/// name is `[^<]*` so the match can't run past the marker into the transcript.
static QWEN_ASR_PREFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*language\s+[^<]*<asr_text>\s*").unwrap());

/// Stray `<asr_text>` / `</asr_text>` tags anywhere else in the text, in case a
/// future build emits a closing tag as well as the opening one.
static QWEN_ASR_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)</?asr_text>").unwrap());

/// Strip Qwen3-ASR's own control framing from a transcript.
///
/// llama.cpp returns the model's raw decoded output for Qwen3-ASR without
/// stripping its control tokens (ggml-org/llama.cpp#26749), so the `text` field
/// arrives as `language English<asr_text>the real transcript`. Left alone that
/// prefix is typed verbatim into the user's input.
///
/// This is deliberately a no-op for text that carries no marker: the client
/// targets *any* OpenAI-compatible server and almost none of them emit this.
/// Delete this function once llama.cpp fixes #26749 upstream.
fn sanitize_transcript(text: &str) -> String {
    let stripped = QWEN_ASR_PREFIX_RE.replace(text, "");
    QWEN_ASR_TAG_RE
        .replace_all(&stripped, "")
        .trim()
        .to_string()
}

impl SttClient {
    pub fn new(cfg: &SttConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            http,
            base_url: cfg.base_url.clone(),
            model: cfg.model.clone(),
            language: cfg.language.clone(),
            prompt: cfg.prompt.clone(),
            api_key: cfg.api_key.clone(),
        }
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/audio/transcriptions",
            self.base_url.trim_end_matches('/')
        )
    }

    /// Transcribe a WAV clip and return the recognized text (trimmed).
    pub async fn transcribe(&self, wav: Vec<u8>) -> Result<String, TtsError> {
        let file = reqwest::multipart::Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| TtsError::Request(e.to_string()))?;
        let mut form = reqwest::multipart::Form::new()
            .text("model", self.model.clone())
            .text("response_format", "json")
            .part("file", file);
        if let Some(language) = &self.language {
            form = form.text("language", language.clone());
        }
        if let Some(prompt) = &self.prompt {
            form = form.text("prompt", prompt.clone());
        }

        let mut req = self.http.post(self.endpoint()).multipart(form);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body: String = resp
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(500)
                .collect();
            return Err(TtsError::Status {
                status: status.as_u16(),
                body,
            });
        }
        let parsed: TranscriptionResponse = resp.json().await?;
        Ok(sanitize_transcript(&parsed.text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(base_url: &str) -> SttClient {
        SttClient::new(&SttConfig {
            base_url: base_url.to_string(),
            ..SttConfig::default()
        })
    }

    #[test]
    fn endpoint_trims_trailing_slash() {
        assert_eq!(
            client("http://127.0.0.1:8000/v1/").endpoint(),
            "http://127.0.0.1:8000/v1/audio/transcriptions"
        );
        assert_eq!(
            client("http://127.0.0.1:8000/v1").endpoint(),
            "http://127.0.0.1:8000/v1/audio/transcriptions"
        );
    }

    #[test]
    fn strips_qwen_asr_prefix() {
        assert_eq!(
            sanitize_transcript(
                "language English<asr_text>The north wind and the sun were disputing which was the stronger."
            ),
            "The north wind and the sun were disputing which was the stronger."
        );
    }

    #[test]
    fn strips_prefix_for_a_non_english_language() {
        assert_eq!(
            sanitize_transcript("language Mandarin Chinese<asr_text>  你好，世界。"),
            "你好，世界。"
        );
    }

    #[test]
    fn strips_a_stray_closing_tag() {
        assert_eq!(
            sanitize_transcript("language English<asr_text>Hello there.</asr_text>"),
            "Hello there."
        );
    }

    #[test]
    fn leaves_unmarked_text_alone() {
        // The common case: any other OpenAI-compatible server, and llama.cpp
        // once #26749 is fixed. Must be a plain trim and nothing more.
        assert_eq!(
            sanitize_transcript("  Just a normal transcript.  "),
            "Just a normal transcript."
        );
    }

    #[test]
    fn leaves_empty_text_alone() {
        assert_eq!(sanitize_transcript(""), "");
        assert_eq!(sanitize_transcript("   "), "");
    }

    #[test]
    fn does_not_mangle_a_transcript_that_talks_about_language() {
        assert_eq!(
            sanitize_transcript("Language models are good at this sort of thing."),
            "Language models are good at this sort of thing."
        );
        assert_eq!(
            sanitize_transcript("language is the topic of today's lecture"),
            "language is the topic of today's lecture"
        );
    }
}
