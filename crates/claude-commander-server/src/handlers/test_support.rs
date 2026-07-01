//! Shared helpers for handler unit tests: build a hermetic [`AppState`] over a
//! `tempfile::TempDir` (empty state, no tmux, no real-filesystem writes) and a
//! small harness for driving a `Router` with `tower::ServiceExt::oneshot`.
//!
//! Construction is hermetic: the `ConfigStore`/`StateStore` use their
//! `with_path` test constructors rooted in the temp dir, so the service never
//! reads or writes the user's real config/state. The comment/reviewed stores
//! derive their directory from the injected `StateStore`'s data dir (see
//! `CommanderService::new`), so even persisting routes stay under the temp dir.

#![cfg(test)]

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, Response, StatusCode},
};
use claude_commander_core::api::CommanderService;
use claude_commander_core::config::storage::AppState as CoreState;
use claude_commander_core::config::{Config, ConfigStore, StateStore};
use claude_commander_core::telemetry::FrontendInfo;
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

use crate::auth::AuthConfig;
use crate::state::AppState;

/// Build a hermetic [`AppState`] backed by empty core state under `dir`.
pub fn test_state(dir: &TempDir) -> AppState {
    // Telemetry is opt-out by default with a baked ingest token, so a plain
    // `CommanderService::new` would post events to the production OpenObserve
    // instance from the test suite (incl. CI). Disable it for tests.
    let mut config = Config::default();
    config.telemetry.enabled = false;
    // Isolate tmux onto a throwaway socket dir under `dir`. These handler tests
    // never spawn tmux, but pinning the knob keeps the fixture safe by default
    // if one ever does (and matches the shared `test-support` harness).
    let tmux_tmpdir = dir.path().join("tmux");
    std::fs::create_dir_all(&tmux_tmpdir).expect("create isolated tmux socket dir");
    config.tmux_tmpdir = Some(tmux_tmpdir);
    let config_store = Arc::new(ConfigStore::with_path(
        config,
        dir.path().join("config.toml"),
    ));
    let store = Arc::new(StateStore::with_path(
        CoreState::default(),
        dir.path().join("state.json"),
    ));
    let frontend = FrontendInfo::new("claude-commander-server-test", "0.0.0");
    let service = CommanderService::new(config_store, store, frontend);
    AppState::new(service, AuthConfig::Disabled)
}

/// Drive `router` with a single request and return its status + decoded body.
pub async fn send(router: Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp: Response<Body> = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, bytes.to_vec())
}

/// Convenience: GET `uri` (no auth, since test state uses `AuthConfig::Disabled`).
pub async fn get(router: Router, uri: &str) -> (StatusCode, Vec<u8>) {
    send(router, Request::get(uri).body(Body::empty()).unwrap()).await
}

/// Convenience for asserting that a JSON body deserializes to `T`.
pub fn json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> T {
    serde_json::from_slice(bytes).unwrap_or_else(|e| {
        panic!(
            "body was not valid JSON for the expected type: {e}; body={}",
            String::from_utf8_lossy(bytes)
        )
    })
}

/// Guard: the test fixture must NOT emit telemetry. Telemetry is opt-out by
/// default with a baked ingest token, so an un-disabled test service would post
/// events to the production OpenObserve instance from `cargo test` / CI. This
/// fails if someone re-enables it.
#[tokio::test]
async fn test_state_disables_telemetry() {
    let dir = TempDir::new().unwrap();
    let state = test_state(&dir);
    assert!(
        !state.service.telemetry().is_active(),
        "test fixtures must not emit telemetry (would pollute production OpenObserve)"
    );
}
