//! Shared integration-test harness for the HTTP/WebSocket server.
//!
//! Both the server's own integration tests (`claude-commander-server/tests/`)
//! and the Flutter cdylib's tests (`client/rust/tests/`) boot the real router on
//! an ephemeral loopback port and drive it through real tmux + git. This crate
//! holds that harness once so the two suites share a single copy, rather than
//! duplicating it (CLAUDE.md: "Minimise duplication").
//!
//! `publish = false`: this is test scaffolding, never shipped. It is a normal
//! (non-`dev`) crate so downstream **dev-dependencies** can reach its `pub` API.
//! `claude-commander-server` dev-depends on this crate while this crate depends
//! on the server — a cycle Cargo permits because it only closes through the
//! server's test targets, never its library.
//!
//! All disk access goes through `tempfile::TempDir`; nothing touches the real
//! filesystem. Listeners bind `127.0.0.1:0` so the OS assigns a free port.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use claude_commander_core::api::CommanderService;
use claude_commander_core::config::storage::AppState as CoreState;
use claude_commander_core::config::{Config, ConfigStore, StateStore};
use claude_commander_core::telemetry::FrontendInfo;
use claude_commander_server::{AppState, AuthConfig, build_router};
use tempfile::TempDir;

/// Whether tmux is installed. Callers self-skip (an early `return`) when this is
/// false — never `#[ignore]`, which would hide the test from `cargo test`
/// everywhere and so never run it in CI.
pub async fn tmux_available() -> bool {
    tokio::process::Command::new("tmux")
        .arg("-V")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run a git command in `dir`, asserting it succeeds.
pub async fn run_git(dir: &Path, args: &[&str]) {
    let output = tokio::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Create a minimal committed git repo in a fresh `TempDir`.
pub async fn create_test_repo() -> (TempDir, PathBuf) {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path().to_path_buf();
    run_git(&repo_path, &["init"]).await;
    run_git(&repo_path, &["config", "user.email", "test@test.com"]).await;
    run_git(&repo_path, &["config", "user.name", "Test User"]).await;
    tokio::fs::write(repo_path.join("README.md"), "# Test Repository\n")
        .await
        .unwrap();
    run_git(&repo_path, &["add", "README.md"]).await;
    run_git(&repo_path, &["commit", "-m", "Initial commit"]).await;
    (temp_dir, repo_path)
}

/// Build a hermetic [`AppState`]: empty core state under `data_dir`, a temp
/// worktrees dir, auth disabled, wrapping a real `CommanderService`.
pub fn test_state(data_dir: &TempDir, worktrees_dir: &TempDir) -> AppState {
    let config = Config {
        worktrees_dir: Some(worktrees_dir.path().to_path_buf()),
        ..Config::default()
    };
    let config_store = Arc::new(ConfigStore::with_path(
        config,
        data_dir.path().join("config.toml"),
    ));
    let store = Arc::new(StateStore::with_path(
        CoreState::default(),
        data_dir.path().join("state.json"),
    ));
    let frontend = FrontendInfo::new("claude-commander-server-test", "0.0.0");
    let service = CommanderService::new(config_store, store, frontend);
    AppState::new(service, AuthConfig::Disabled)
}

/// Boot the router on `127.0.0.1:0` and return the bound address. The serving
/// task is spawned onto the current runtime and left running for the lifetime of
/// the process (or until the enclosing runtime is dropped).
pub async fn spawn_server(state: AppState) -> SocketAddr {
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}
