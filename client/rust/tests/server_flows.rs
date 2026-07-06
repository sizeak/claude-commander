//! Integration tests: the client cdylib's blocking HTTP functions driven against
//! a **real in-process server** (through real tmux + git). This is the primary
//! app↔server contract coverage — the exact `pub fn`s the Flutter app calls,
//! exercising base-url handling, bearer auth, `404 → None`, and DTO decoding end
//! to end.
//!
//! ## Why a plain `#[test]` holding a `Runtime` (not `#[tokio::test]`)
//!
//! The cdylib's HTTP functions use `reqwest::blocking`, which **panics if called
//! from within a Tokio runtime thread**. So we can't call them from inside
//! `#[tokio::test]`. Instead each test owns a multi-thread `Runtime`, uses
//! `rt.block_on(...)` only for async *setup/teardown* (tmux probe, repo, server
//! spawn, core-service calls), and calls the blocking cdylib fns **directly on
//! the test thread** — which is not a runtime worker, so `reqwest::blocking` is
//! happy. `spawn_server`'s serve task keeps running on the runtime's background
//! workers while the test thread makes blocking calls.
//!
//! The shared hermetic-server + git/tmux harness lives in
//! `claude-commander-test-support`. Tests self-skip (runtime `tmux_available()`)
//! on a tmux-less box; never `#[ignore]`, so CI (tmux present) runs them.
//!
//! **Coverage boundary:** the live-terminal bridge (`api::terminal`) needs an frb
//! `StreamSink` that only exists in the Dart bridge, so it can't be driven from
//! Rust. Its behavior is covered by the server's own
//! `ws_attach_streams_and_detach_keeps_session_alive` test plus the Dart e2e.

use std::path::PathBuf;

use claude_commander_core::api::CreateSessionOpts;
use claude_commander_core::session::SessionId;
use claude_commander_test_support::{create_test_repo, spawn_server, test_state, tmux_available};
use rust_lib_claude_commander_client::api::{review, simple};
use tempfile::TempDir;
use tokio::runtime::Runtime;
use uuid::Uuid;

/// Arbitrary — the test server runs with `AuthConfig::Disabled`, so any token is
/// accepted. Passing one still exercises the cdylib's `bearer_auth` path.
fn token() -> String {
    "test-token".to_string()
}

/// A booted hermetic server over a fresh committed git repo, with the project
/// registered. Holds the `Runtime` (keeps the serving task alive) and the temp
/// dirs (kept until drop).
struct Fixture {
    rt: Runtime,
    base: String,
    service: claude_commander_core::api::CommanderService,
    repo_path: PathBuf,
    _repo: TempDir,
    _data: TempDir,
    _worktrees: TempDir,
}

impl Fixture {
    /// Boot the server. Returns `None` when tmux is absent so the caller can
    /// self-skip with an early `return`.
    fn start() -> Option<Fixture> {
        let rt = Runtime::new().unwrap();
        if !rt.block_on(tmux_available()) {
            eprintln!("Skipping test: tmux not available");
            return None;
        }
        let (repo, repo_path) = rt.block_on(create_test_repo());
        let data = TempDir::new().unwrap();
        let worktrees = TempDir::new().unwrap();
        let state = test_state(&data, &worktrees);
        let service = state.service.clone();
        let addr = rt.block_on(spawn_server(state));
        rt.block_on(service.add_project(repo_path.clone()))
            .expect("register project");
        Some(Fixture {
            rt,
            base: format!("http://{addr}"),
            service,
            repo_path,
            _repo: repo,
            _data: data,
            _worktrees: worktrees,
        })
    }

    /// Create a session directly through the core service (a setup shortcut for
    /// tests that want an *existing* session to act on), returning its id.
    fn create_session(&self, title: &str) -> SessionId {
        self.rt
            .block_on(self.service.create_session(CreateSessionOpts {
                project_path: self.repo_path.clone(),
                title: title.to_string(),
                program: Some("bash".to_string()),
                initial_prompt: None,
                effort: None,
                mode: None,
                model: None,
                base_branch: None,
                section: None,
                stack_parent: None,
            }))
            .expect("create session")
    }

    /// The on-disk worktree directory for a session (so a test can write a file
    /// into it to produce a review diff). Mirrors the server's own
    /// `review_target` lookup.
    fn worktree_path(&self, id: &SessionId) -> PathBuf {
        self.rt.block_on(async {
            self.service
                .store()
                .read()
                .await
                .get_session(id)
                .expect("session exists")
                .worktree_path
                .clone()
        })
    }

    /// Best-effort tmux cleanup for a session left running by a test.
    fn kill(&self, id: &SessionId) {
        let _ = self.rt.block_on(self.service.kill_session(id));
    }
}

/// Parse the string id the cdylib returns back into a `SessionId` (for cleanup).
fn parse_id(id: &str) -> SessionId {
    SessionId::from_uuid(Uuid::parse_str(id).expect("valid session uuid"))
}

/// The full-UUID string for a `SessionId`. `SessionId`'s `Display` is only the
/// 8-char prefix (what the UI shows); routes like restart/delete/review require
/// the full id, so pass this — not `sid.to_string()`.
fn full_id(sid: &SessionId) -> String {
    sid.as_uuid().to_string()
}

/// Connect probes: unauthenticated `/health` and the authenticated tmux probe.
#[test]
fn connect_probes_health_and_tmux() {
    let Some(fx) = Fixture::start() else { return };

    assert!(
        simple::health(fx.base.clone()).unwrap(),
        "GET /health should report the server live"
    );
    assert!(
        simple::health_tmux(fx.base.clone(), token()).unwrap(),
        "GET /api/health/tmux should be true when tmux is present"
    );
}

/// Create a session through the cdylib, then see it in the list, fetch its
/// detail + pane, and kill it — the core lifecycle a client drives.
#[test]
fn list_create_detail_kill_round_trip() {
    let Some(fx) = Fixture::start() else { return };

    let id = simple::create_session(
        fx.base.clone(),
        token(),
        fx.repo_path.to_string_lossy().into_owned(),
        "cdylib-roundtrip".to_string(),
        Some("bash".to_string()),
        None,
        None,
        None,
        None,
    )
    .expect("create_session");

    let sessions = simple::list_sessions(fx.base.clone(), token(), true).unwrap();
    assert!(
        sessions.iter().any(|s| s.id == id),
        "created session {id} should appear in list_sessions"
    );

    let detail =
        simple::get_session_detail(fx.base.clone(), token(), id.clone(), Some(50)).unwrap();
    assert!(detail.is_some(), "detail should resolve the full id");
    assert_eq!(detail.unwrap().info.id, id);

    let pane = simple::get_pane(fx.base.clone(), token(), id.clone(), Some(50)).unwrap();
    assert!(pane.is_some(), "pane should resolve the full id");

    simple::kill_session(fx.base.clone(), token(), id.clone()).expect("kill_session");

    fx.kill(&parse_id(&id));
}

/// A running session survives a restart and is gone from the list after delete.
#[test]
fn restart_then_delete() {
    let Some(fx) = Fixture::start() else { return };
    let sid = fx.create_session("cdylib-restart");
    let id = full_id(&sid);

    simple::restart_session(fx.base.clone(), token(), id.clone()).expect("restart_session");
    simple::delete_session(fx.base.clone(), token(), id.clone()).expect("delete_session");

    let sessions = simple::list_sessions(fx.base.clone(), token(), true).unwrap();
    assert!(
        !sessions.iter().any(|s| s.id == id),
        "deleted session {id} must not appear in list_sessions"
    );
}

/// Join an existing session (created out-of-band) by the 8-char id prefix — the
/// loose match a re-joining client relies on.
#[test]
fn join_existing_by_prefix() {
    let Some(fx) = Fixture::start() else { return };
    let sid = fx.create_session("cdylib-join");
    // `SessionId`'s Display is the 8-char prefix a client sees in the UI.
    let prefix = sid.to_string();

    let detail =
        simple::get_session_detail(fx.base.clone(), token(), prefix.clone(), Some(20)).unwrap();
    let detail = detail.expect("an 8-char prefix should resolve the existing session");
    assert!(
        detail.info.id.starts_with(&prefix),
        "prefix {prefix} must resolve to a session whose full id starts with it (got {})",
        detail.info.id
    );

    fx.kill(&sid);
}

/// Full review round-trip: produce a diff by writing a file into the worktree,
/// then open the review, comment, mark reviewed, apply, and confirm an unchanged
/// refresh short-circuits (204 → None).
#[test]
fn review_round_trip() {
    let Some(fx) = Fixture::start() else { return };
    let sid = fx.create_session("cdylib-review");
    let id = full_id(&sid);

    // Produce a working-tree-vs-base diff: a brand-new file in the worktree.
    let worktree = fx.worktree_path(&sid);
    std::fs::write(worktree.join("newfile.txt"), "hello from test\n").expect("write worktree file");

    // -- open_review shows the new file (and caches its FileDiff for toggle) --
    let snap = review::open_review(fx.base.clone(), token(), id.clone()).expect("open_review");
    let file = snap
        .files
        .iter()
        .find(|f| f.display_path == "newfile.txt")
        .expect("newfile.txt should appear in the diff");
    assert!(!file.is_binary, "a text file should not be flagged binary");

    // -- create a comment on the added line, then list it --
    let comment_id = review::create_comment(
        fx.base.clone(),
        token(),
        id.clone(),
        "newfile.txt".to_string(),
        "new".to_string(),
        1,
        1,
        "hello from test".to_string(),
        "looks good".to_string(),
    )
    .expect("create_comment");
    let comments = review::list_comments(fx.base.clone(), token(), id.clone()).unwrap();
    assert!(
        comments.iter().any(|c| c.id == comment_id),
        "the created comment should be listed"
    );

    // -- toggle the file's reviewed mark on (by display path; the server
    //    resolves the FileDiff itself) --
    let reviewed = review::toggle_file_reviewed(
        fx.base.clone(),
        token(),
        id.clone(),
        "newfile.txt".to_string(),
    )
    .expect("toggle_file_reviewed");
    assert!(
        reviewed,
        "toggling an un-reviewed file should mark it reviewed"
    );

    // -- a path that isn't in the current diff is a 404, not a silent no-op --
    assert!(
        review::toggle_file_reviewed(
            fx.base.clone(),
            token(),
            id.clone(),
            "no-such-file.txt".to_string(),
        )
        .is_err(),
        "toggling a file absent from the diff must error"
    );

    // -- apply the staged comment: composed + delivered (or deferred), not
    //    blocked, and one comment counted --
    let result = review::apply_comments(fx.base.clone(), token(), id.clone()).expect("apply");
    assert!(
        matches!(
            result.kind,
            review::ApplyResultKind::Applied | review::ApplyResultKind::Deferred
        ),
        "a single fresh comment should apply or defer, not block/no-op"
    );
    assert_eq!(result.count, 1, "exactly one comment should be composed");

    // -- an unchanged refresh short-circuits (204 → None) --
    let latest = review::open_review(fx.base.clone(), token(), id.clone()).unwrap();
    let refreshed =
        review::refresh_review(fx.base.clone(), token(), id.clone(), latest.content_hash).unwrap();
    assert!(
        refreshed.is_none(),
        "refresh with the current content hash should report no change"
    );

    fx.kill(&sid);
}
