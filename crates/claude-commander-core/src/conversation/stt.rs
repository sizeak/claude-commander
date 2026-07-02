//! OpenAI-compatible speech-to-text (transcription) HTTP client.
//!
//! Posts a `multipart/form-data` body to `{base_url}/audio/transcriptions`
//! (OpenAI's `POST /v1/audio/transcriptions` shape), so it works against any
//! compatible engine — we dev against a local/LAN faster-whisper server.

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
        Ok(parsed.text.trim().to_string())
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
}
