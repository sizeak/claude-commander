//! OpenAI-compatible TTS HTTP client.
//!
//! Posts to `{base_url}/audio/speech` (OpenAI's `POST /v1/audio/speech` shape),
//! so it works against any compatible engine — we dev against the local Kokoro
//! container on `http://127.0.0.1:8002/v1`.

use serde::Serialize;

use crate::error::TtsError;

/// A speech-synthesis request body (serializes to the OpenAI TTS JSON shape).
#[derive(Debug, Clone, Serialize)]
pub struct SpeechRequest<'a> {
    pub model: &'a str,
    pub input: &'a str,
    pub voice: &'a str,
    pub response_format: &'a str,
    pub speed: f32,
}

/// Thin client around `reqwest`. Cheap to clone (shares the connection pool).
#[derive(Debug, Clone)]
pub struct TtsClient {
    http: reqwest::Client,
    base_url: String,
}

impl TtsClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/audio/speech", self.base_url.trim_end_matches('/'))
    }

    /// Synthesize `req` and return the raw encoded audio bytes.
    pub async fn synthesize(&self, req: &SpeechRequest<'_>) -> Result<Vec<u8>, TtsError> {
        let resp = self.http.post(self.endpoint()).json(req).send().await?;
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
        Ok(resp.bytes().await?.to_vec())
    }
}

/// The JSON body that [`TtsClient::synthesize`] sends, exposed for testing
/// (single source of truth via serde).
pub fn build_speech_body(req: &SpeechRequest<'_>) -> serde_json::Value {
    serde_json::to_value(req).expect("SpeechRequest serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_has_openai_shape() {
        let req = SpeechRequest {
            model: "kokoro",
            input: "Hello.",
            voice: "af_sky",
            response_format: "wav",
            speed: 1.25,
        };
        let body = build_speech_body(&req);
        assert_eq!(body["model"], "kokoro");
        assert_eq!(body["input"], "Hello.");
        assert_eq!(body["voice"], "af_sky");
        assert_eq!(body["response_format"], "wav");
        assert_eq!(body["speed"], 1.25);
    }

    #[test]
    fn endpoint_trims_trailing_slash() {
        assert_eq!(
            TtsClient::new("http://127.0.0.1:8002/v1/").endpoint(),
            "http://127.0.0.1:8002/v1/audio/speech"
        );
        assert_eq!(
            TtsClient::new("http://127.0.0.1:8002/v1").endpoint(),
            "http://127.0.0.1:8002/v1/audio/speech"
        );
    }
}
