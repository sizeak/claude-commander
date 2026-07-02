//! End-to-end integration tests for `claude-commander-server`.
//!
//! These boot the real router on an ephemeral loopback port and drive it with
//! real HTTP and WebSocket clients, through real tmux + git. They require tmux
//! and use a **runtime** `tmux_available()` self-skip (an early `return`), *not*
//! `#[ignore]` — `#[ignore]` would make `cargo test` skip them everywhere, so
//! they'd never execute in CI. With the runtime check, `cargo test --workspace`
//! in the Nix dev shell (tmux present) actually runs them; on a tmux-less box
//! they self-skip.
//!
//! The hermetic-server + git/tmux fixtures live in the shared
//! `claude-commander-test-support` crate (also used by the Flutter cdylib's
//! `client/rust/tests`). All disk access goes through `tempfile::TempDir`;
//! nothing touches the real filesystem. The listener binds `127.0.0.1:0` so the
//! OS assigns a free port.

use std::time::Duration;

use claude_commander_core::tmux::TmuxExecutor;
use claude_commander_test_support::{create_test_repo, spawn_server, test_state, tmux_available};
use futures::{SinkExt, StreamExt};
use tempfile::TempDir;
use tokio_tungstenite::tungstenite::Message;

/// HTTP round-trip through real tmux + git: register a project, create a
/// session, then list/detail/pane/kill it over the wire.
#[tokio::test]
async fn http_session_lifecycle_round_trip() {
    if !tmux_available().await {
        eprintln!("Skipping test: tmux not available");
        return;
    }

    let (repo_temp_dir, repo_path) = create_test_repo().await;
    let data_dir = TempDir::new().unwrap();
    let worktrees_dir = TempDir::new().unwrap();
    let state = test_state(&data_dir, &worktrees_dir);
    // Hold a service clone so we can clean tmux up at the end regardless of the
    // HTTP path taken.
    let service = state.service.clone();
    let addr = spawn_server(state).await;
    let base = format!("http://{addr}/api");
    let client = reqwest::Client::new();

    // -- register the project --
    let resp = client
        .post(format!("{base}/projects"))
        .json(&serde_json::json!({ "path": repo_path }))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "POST /projects should succeed, got {}",
        resp.status()
    );

    // -- create a session -> 201 { id } --
    let resp = client
        .post(format!("{base}/sessions"))
        .json(&serde_json::json!({
            "project_path": repo_path,
            "title": "http-roundtrip",
            "program": "bash",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::CREATED,
        "POST /sessions should be 201"
    );
    let created: serde_json::Value = resp.json().await.unwrap();
    let id = created["id"]
        .as_str()
        .expect("id should be a string")
        .to_string();

    // -- GET /sessions shows it --
    let resp = client.get(format!("{base}/sessions")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let sessions: serde_json::Value = resp.json().await.unwrap();
    let arr = sessions.as_array().expect("sessions should be an array");
    assert!(
        arr.iter().any(|s| s["id"] == serde_json::json!(id)),
        "created session {id} should appear in the list: {sessions}"
    );

    // -- GET /sessions/{q}/detail -> 200 --
    // Query with the FULL UUID (exactly what `POST /sessions` returned), proving
    // detail/pane resolve the full id a real client holds (B1).
    let resp = client
        .get(format!("{base}/sessions/{id}/detail"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "detail should be 200");

    // -- GET /sessions/{q}/pane -> 200 --
    let resp = client
        .get(format!("{base}/sessions/{id}/pane"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "pane should be 200");

    // -- POST /sessions/{id}/kill -> 204 --
    let resp = client
        .post(format!("{base}/sessions/{id}/kill"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NO_CONTENT,
        "kill should be 204"
    );

    // -- cleanup: ensure the tmux session is gone --
    let id =
        claude_commander_core::session::SessionId::from_uuid(uuid::Uuid::parse_str(&id).unwrap());
    let _ = service.kill_session(&id).await;

    drop(repo_temp_dir);
    drop(data_dir);
    drop(worktrees_dir);
}

/// WebSocket attach: handshake (`auth` → `attach` → `resize`), assert `ready`,
/// send a keystroke as a BINARY frame, assert PTY output arrives as binary
/// frames, then `detach` and assert the tmux session **still exists** (detach ≠
/// kill). Finally kill it to clean up.
#[tokio::test]
async fn ws_attach_streams_and_detach_keeps_session_alive() {
    if !tmux_available().await {
        eprintln!("Skipping test: tmux not available");
        return;
    }

    let (repo_temp_dir, repo_path) = create_test_repo().await;
    let data_dir = TempDir::new().unwrap();
    let worktrees_dir = TempDir::new().unwrap();
    let state = test_state(&data_dir, &worktrees_dir);
    let service = state.service.clone();
    let addr = spawn_server(state).await;

    // Register project + create a session directly through the service (the HTTP
    // path is covered by the other test; here we focus on the WS contract).
    service.add_project(repo_path.clone()).await.unwrap();
    let session_id = service
        .create_session(claude_commander_core::api::CreateSessionOpts {
            project_path: repo_path.clone(),
            title: "ws-attach".to_string(),
            program: Some("bash".to_string()),
            initial_prompt: None,
            effort: None,
            mode: None,
            base_branch: None,
            section: None,
        })
        .await
        .unwrap();

    // The tmux session name we expect to survive a detach.
    let tmux_name = service
        .resolve_tmux_session(&session_id.to_string())
        .await
        .unwrap()
        .expect("session should resolve to a tmux name");

    // -- open the WebSocket --
    let url = format!("ws://{addr}/ws/attach");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url).await.unwrap();

    // -- handshake: auth (token is ignored under AuthConfig::Disabled, but the
    //    handler still expects an auth frame first), attach, resize --
    ws.send(Message::Text(
        r#"{"type":"auth","token":"unused"}"#.to_string(),
    ))
    .await
    .unwrap();
    ws.send(Message::Text(
        serde_json::json!({"type":"attach","session_id": session_id.to_string()}).to_string(),
    ))
    .await
    .unwrap();
    ws.send(Message::Text(
        r#"{"type":"resize","cols":100,"rows":30}"#.to_string(),
    ))
    .await
    .unwrap();

    // -- assert a `ready` server control frame (text/JSON) arrives --
    let ready = next_text_frame(&mut ws)
        .await
        .expect("should receive a control frame after handshake");
    let parsed: serde_json::Value = serde_json::from_str(&ready).unwrap();
    assert_eq!(
        parsed["type"], "ready",
        "first control frame should be `ready`, got: {ready}"
    );

    // -- type a keystroke (BINARY frame) and assert PTY output arrives as
    //    BINARY frames. A bare newline makes bash echo a prompt line. --
    ws.send(Message::Binary(b"echo cc_ws_marker\n".to_vec()))
        .await
        .unwrap();

    let got_binary = wait_for_binary_output(&mut ws, Duration::from_secs(5)).await;
    assert!(
        got_binary,
        "should receive PTY output as a binary frame after typing"
    );

    // -- detach: should leave the tmux session running --
    ws.send(Message::Text(r#"{"type":"detach"}"#.to_string()))
        .await
        .unwrap();
    // Drain until the socket closes so the server completes its teardown
    // (kills only the `tmux attach-session` child, not the session).
    drain_until_close(&mut ws, Duration::from_secs(5)).await;

    // -- the tmux session must STILL EXIST (detach ≠ kill) --
    // Pin this probe onto the same isolated socket dir the harness put the
    // session on (via `tmux_tmpdir`), or it would query the developer's real
    // tmux server and never see the session.
    let tmux = TmuxExecutor::new().with_tmux_tmpdir(service.read_config().tmux_tmpdir);
    let mut exists = false;
    for _ in 0..50 {
        if tmux.session_exists(&tmux_name).await.unwrap_or(false) {
            exists = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        exists,
        "tmux session {tmux_name} must survive a WS detach (detach is not a kill)"
    );

    // -- now actually kill it to clean up --
    service.kill_session(&session_id).await.unwrap();

    drop(repo_temp_dir);
    drop(data_dir);
    drop(worktrees_dir);
}

/// Receive frames until the next TEXT frame, returning its payload. `None` on
/// close/error/timeout.
async fn next_text_frame(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Option<String> {
    let fut = async {
        while let Some(msg) = ws.next().await {
            match msg {
                Ok(Message::Text(t)) => return Some(t.to_string()),
                Ok(Message::Close(_)) | Err(_) => return None,
                // skip binary / ping / pong while waiting for a control frame
                Ok(_) => continue,
            }
        }
        None
    };
    tokio::time::timeout(Duration::from_secs(5), fut)
        .await
        .ok()
        .flatten()
}

/// Wait up to `timeout` for at least one non-empty BINARY frame (PTY output).
async fn wait_for_binary_output(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    timeout: Duration,
) -> bool {
    let fut = async {
        while let Some(msg) = ws.next().await {
            match msg {
                Ok(Message::Binary(b)) if !b.is_empty() => return true,
                Ok(Message::Close(_)) | Err(_) => return false,
                Ok(_) => continue,
            }
        }
        false
    };
    tokio::time::timeout(timeout, fut).await.unwrap_or(false)
}

/// Drain frames until the socket closes (or `timeout` elapses).
async fn drain_until_close(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    timeout: Duration,
) {
    let fut = async {
        while let Some(msg) = ws.next().await {
            if matches!(msg, Ok(Message::Close(_)) | Err(_)) {
                break;
            }
        }
    };
    let _ = tokio::time::timeout(timeout, fut).await;
}
