//! Where assembled events go. The production sink POSTs batches to
//! OpenObserve's `_json` bulk-ingest endpoint; tests use an in-memory sink.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

/// A single assembled event: a flat JSON object. Built only via the typed
/// constructors in [`super`], never from arbitrary user input.
pub type EventPayload = serde_json::Map<String, Value>;

/// Receiver of batched telemetry events. Implementations MUST swallow their own
/// errors — telemetry must never surface failures to the application.
#[async_trait]
pub trait EventSink: Send + Sync {
    async fn send_batch(&self, events: Vec<EventPayload>);
}

/// Posts batches to an OpenObserve `_json` ingest endpoint using a pre-encoded
/// HTTP Basic credential (`base64("<email>:<token>")`). Failures are logged at
/// `debug` and otherwise ignored.
pub struct HttpSink {
    http: reqwest::Client,
    endpoint: String,
    /// The value for the `Authorization: Basic …` header.
    basic_credential: String,
}

impl HttpSink {
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

    pub fn new(endpoint: impl Into<String>, basic_credential: impl Into<String>) -> Self {
        // A builder failure here would be a misconfigured TLS backend; fall
        // back to the default client rather than abort the app over telemetry.
        let http = reqwest::Client::builder()
            .timeout(Self::REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            http,
            endpoint: endpoint.into(),
            basic_credential: basic_credential.into(),
        }
    }
}

#[async_trait]
impl EventSink for HttpSink {
    async fn send_batch(&self, events: Vec<EventPayload>) {
        if events.is_empty() {
            return;
        }
        let body = Value::Array(events.into_iter().map(Value::Object).collect());
        let request = self
            .http
            .post(&self.endpoint)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Basic {}", self.basic_credential),
            )
            .json(&body)
            .send();
        // Backstop the request with an explicit timeout. The client is built
        // with `REQUEST_TIMEOUT`, but on a builder failure `new` falls back to a
        // timeout-less default client — this guarantees a bound regardless, so a
        // black-holed connection can never hang the single-threaded flush task.
        match tokio::time::timeout(Self::REQUEST_TIMEOUT, request).await {
            Ok(Ok(resp)) if resp.status().is_success() => {}
            Ok(Ok(resp)) => debug!("telemetry ingest returned {}", resp.status()),
            Ok(Err(e)) => debug!("telemetry ingest failed: {e}"),
            Err(_) => debug!("telemetry ingest timed out"),
        }
    }
}

#[cfg(test)]
#[derive(Default)]
pub struct MemorySink {
    events: std::sync::Mutex<Vec<EventPayload>>,
}

#[cfg(test)]
impl MemorySink {
    pub fn events(&self) -> Vec<EventPayload> {
        self.events.lock().unwrap().clone()
    }
}

#[cfg(test)]
#[async_trait]
impl EventSink for MemorySink {
    async fn send_batch(&self, mut events: Vec<EventPayload>) {
        self.events.lock().unwrap().append(&mut events);
    }
}
