//! Integration tests: the client cdylib's route functions driven against a
//! **real in-process server** (through real tmux + git). This is the primary
//! app↔server contract coverage — the exact `pub fn`s the Flutter app calls,
//! exercising the server-handle registry, bearer auth, `404 → None`, and
//! DTO decoding end to end.
//!
//! ## Why a plain `#[test]` holding a `Runtime` (not `#[tokio::test]`)
//!
//! The cdylib's route fns now resolve a [`RemoteClient`] from the server handle
//! and `block_on` its async call on the cdylib's *own* shared runtime. A nested
//! `block_on` (calling one from inside another runtime's `block_on`) panics, so
//! each test owns a multi-thread `Runtime` used **only** for async setup/teardown
//! (tmux probe, repo, server spawn, core-service calls) and calls the cdylib fns
//! **directly on the test thread** — which is not a runtime worker, so the
//! cdylib's internal `block_on` is happy. `spawn_server`'s serve task keeps
//! running on the fixture runtime's background workers while the test thread makes
//! its calls.
//!
//! The shared hermetic-server + git/tmux harness lives in
//! `claude-commander-test-support`. Tests self-skip (runtime `tmux_available()`)
//! on a tmux-less box; never `#[ignore]`, so CI (tmux present) runs them.
//!
//! **Coverage boundary:** the live-terminal bridge + the poller-driven feeds
//! (`api::terminal`) need an frb `StreamSink` that only exists in the Dart bridge,
//! so they can't be driven from Rust. Their behavior is covered by the server's
//! own WS attach test, the client crate's attach/poller unit tests, and the Dart
//! e2e.

use std::path::PathBuf;
use std::time::Duration;

use claude_commander_client::{RemoteClient, RemoteServerSpec, SecretString};
use claude_commander_core::api::CreateSessionOpts;
use claude_commander_core::session::SessionId;
use claude_commander_protocol::api::{OperationKind, ProgramInfo};
use claude_commander_protocol::github::{
    CloneJob, CloneJobId, CloneRequest, CloneSource, CloneStatus,
};
use claude_commander_test_support::{
    create_test_repo, run_git, spawn_server, test_state, tmux_available,
};
use rust_lib_claude_commander_client::api::mirrors::OperationOutcomeKind;
use rust_lib_claude_commander_client::api::{diff, registry, review, simple};
use tempfile::TempDir;
use tokio::runtime::Runtime;
use uuid::Uuid;

/// Arbitrary — the test server runs with `AuthConfig::Disabled`, so any token is
/// accepted. Passing one still exercises the bearer path.
fn token() -> String {
    "test-token".to_string()
}

/// A booted hermetic server over a fresh committed git repo, with the project
/// registered and a connected cdylib server handle. Holds the `Runtime` (keeps
/// the serving task alive) and the temp dirs (kept until drop).
struct Fixture {
    rt: Runtime,
    base: String,
    /// The connected cdylib server handle every route call is keyed by.
    handle: String,
    service: claude_commander_core::api::CommanderService,
    repo_path: PathBuf,
    _repo: TempDir,
    _data: TempDir,
    _worktrees: TempDir,
}

impl Fixture {
    /// Boot the server + connect a handle. Returns `None` when tmux is absent so
    /// the caller can self-skip with an early `return`.
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
        let base = format!("http://{addr}");
        // Connect through the cdylib registry — validates the URL and spawns the
        // background poller on the cdylib runtime. Reachability isn't checked
        // here, so this succeeds regardless of poll timing.
        let handle = registry::connect_server(base.clone(), Some(token())).expect("connect_server");
        Some(Fixture {
            rt,
            base,
            handle,
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

    /// Best-effort tmux cleanup for a session left running by a test.
    fn kill(&self, id: &SessionId) {
        let _ = self.rt.block_on(self.service.kill_session(id));
    }

    /// A [`RemoteClient`] pointed at this fixture's server.
    ///
    /// The clone flow below drives the transport crate directly rather than a
    /// cdylib `api::` fn, because Task 9 adds the `RemoteClient` methods and the
    /// frb bridge that wraps them is Task 10's. It is still the same server, the
    /// same routes and the same DTO decode this file exists to pin.
    fn remote_client(&self) -> RemoteClient {
        RemoteClient::new(RemoteServerSpec {
            name: "fixture".to_string(),
            base_url: self.base.clone(),
            token: Some(SecretString::new(token())),
        })
        .expect("build RemoteClient")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Drop the poller so it stops polling a server that's about to go away.
        registry::disconnect_server(self.handle.clone());
    }
}

/// Parse the string id the cdylib returns back into a `SessionId` (for cleanup).
fn parse_id(id: &str) -> SessionId {
    SessionId::from_uuid(Uuid::parse_str(id).expect("valid session uuid"))
}

/// The full-UUID string for a `SessionId`. `SessionId`'s `Display` is only the
/// 8-char prefix (what the UI shows); routes require the full id, so pass this.
fn full_id(sid: &SessionId) -> String {
    sid.as_uuid().to_string()
}

/// Connect probes: unauthenticated `/health` and the authenticated tmux probe.
/// These take a raw base URL/token (the connect screen calls them before a
/// handle exists), so they don't use the fixture handle.
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
/// detail, and kill it — the core lifecycle a client drives.
#[test]
fn list_create_detail_kill_round_trip() {
    let Some(fx) = Fixture::start() else { return };

    let id = simple::create_session(
        fx.handle.clone(),
        fx.repo_path.to_string_lossy().into_owned(),
        "cdylib-roundtrip".to_string(),
        Some("bash".to_string()),
        None,
        None,
        None,
        None,
    )
    .expect("create_session");

    let sessions = simple::list_sessions(fx.handle.clone(), true).unwrap();
    assert!(
        sessions.iter().any(|s| s.id == id),
        "created session {id} should appear in list_sessions"
    );

    let detail = simple::get_session_detail(fx.handle.clone(), id.clone(), Some(50)).unwrap();
    assert!(detail.is_some(), "detail should resolve the full id");
    assert_eq!(detail.unwrap().info.id, id);

    simple::kill_session(fx.handle.clone(), id.clone()).expect("kill_session");

    fx.kill(&parse_id(&id));
}

/// A running session survives a restart and is gone from the list after delete.
#[test]
fn restart_then_delete() {
    let Some(fx) = Fixture::start() else { return };
    let sid = fx.create_session("cdylib-restart");
    let id = full_id(&sid);

    simple::restart_session(fx.handle.clone(), id.clone()).expect("restart_session");
    simple::delete_session(fx.handle.clone(), id.clone()).expect("delete_session");

    let sessions = simple::list_sessions(fx.handle.clone(), true).unwrap();
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

    let detail = simple::get_session_detail(fx.handle.clone(), prefix.clone(), Some(20)).unwrap();
    let detail = detail.expect("an 8-char prefix should resolve the existing session");
    assert!(
        detail.info.id.starts_with(&prefix),
        "prefix {prefix} must resolve to a session whose full id starts with it (got {})",
        detail.info.id
    );

    fx.kill(&sid);
}

/// The workspace snapshot carries the registered project and any live sessions.
#[test]
fn workspace_snapshot_lists_projects_and_sessions() {
    let Some(fx) = Fixture::start() else { return };
    let sid = fx.create_session("cdylib-snapshot");
    let id = full_id(&sid);

    let snap = simple::workspace_snapshot(fx.handle.clone()).expect("workspace_snapshot");
    assert!(
        snap.projects
            .iter()
            .any(|p| p.repo_path == fx.repo_path.to_string_lossy()),
        "the registered repo must appear in the snapshot's projects"
    );
    assert!(
        snap.sessions.iter().any(|s| s.id == id),
        "the created session must appear in the snapshot's sessions"
    );
    // Server health is populated from the server's own environment.
    assert!(snap.server.tmux_ok, "tmux is present in this test run");

    fx.kill(&sid);
}

/// The bulk agent-state snapshot round-trips (and the commander sentinel entry
/// is never surfaced — the DTO strips it).
#[test]
fn agent_states_snapshot_round_trips() {
    let Some(fx) = Fixture::start() else { return };
    let sid = fx.create_session("cdylib-agent-states");

    let snap = simple::agent_states(fx.handle.clone(), true).expect("agent_states");
    // Every surfaced entry is a real session id — the commander sentinel
    // (0xc03adecc…) is filtered out by the DTO conversion.
    let sentinel = Uuid::from_u128(0xc0_3a_de_cc_00_00_00_00_00_00_00_00_00_00_00_00);
    assert!(
        snap.states
            .iter()
            .all(|e| *e.session_id.as_uuid() != sentinel),
        "the commander sentinel must never appear in the flattened states"
    );

    fx.kill(&sid);
}

/// `create_options` reflects a `set_programs` write — the new-session picker
/// config round-trips through the config route group.
#[test]
fn create_options_reflects_set_programs() {
    let Some(fx) = Fixture::start() else { return };

    // A baseline read works.
    let before = simple::create_options(fx.handle.clone()).expect("create_options");
    assert!(
        !before.default_program.is_empty(),
        "the server should report a default program"
    );

    simple::set_programs(
        fx.handle.clone(),
        vec![ProgramInfo {
            label: "Custom cdylib".to_string(),
            command: "bash -l".to_string(),
        }],
    )
    .expect("set_programs");

    let after = simple::create_options(fx.handle.clone()).expect("create_options");
    assert!(
        after
            .programs
            .iter()
            .any(|p| p.label == "Custom cdylib" && p.command == "bash -l"),
        "create_options must reflect the programs just written"
    );
}

/// Renaming a session shows up in the snapshot; set_section (set then clear)
/// round-trips through the session-patch route.
#[test]
fn rename_and_set_section_round_trip() {
    let Some(fx) = Fixture::start() else { return };
    let sid = fx.create_session("cdylib-rename");
    let id = full_id(&sid);

    simple::rename_session(fx.handle.clone(), id.clone(), "renamed-title".to_string())
        .expect("rename_session");
    let snap = simple::workspace_snapshot(fx.handle.clone()).unwrap();
    assert_eq!(
        snap.sessions
            .iter()
            .find(|s| s.id == id)
            .expect("session present")
            .title,
        "renamed-title",
        "the rename must be reflected in the snapshot"
    );

    // Set then clear a section override — both are 204s that must not error.
    simple::set_section(fx.handle.clone(), id.clone(), Some("Open PRs".to_string()))
        .expect("set_section (set)");
    simple::set_section(fx.handle.clone(), id.clone(), None).expect("set_section (clear)");

    fx.kill(&sid);
}

/// mark_unread then mark_read flip the session's unread flag in the snapshot.
#[test]
fn mark_unread_then_read_round_trip() {
    let Some(fx) = Fixture::start() else { return };
    let sid = fx.create_session("cdylib-unread");
    let id = full_id(&sid);

    let unread = |fx: &Fixture| -> bool {
        simple::workspace_snapshot(fx.handle.clone())
            .unwrap()
            .sessions
            .iter()
            .find(|s| s.id == id)
            .expect("session present")
            .unread
    };

    simple::mark_unread(fx.handle.clone(), vec![id.clone()]).expect("mark_unread");
    assert!(unread(&fx), "mark_unread must set the unread flag");

    simple::mark_read(fx.handle.clone(), id.clone()).expect("mark_read");
    assert!(!unread(&fx), "mark_read must clear the unread flag");

    fx.kill(&sid);
}

/// toggle_keep_alive flips the exemption and reports the new state each time.
#[test]
fn keep_alive_toggles() {
    let Some(fx) = Fixture::start() else { return };
    let sid = fx.create_session("cdylib-keepalive");
    let id = full_id(&sid);

    let first = simple::toggle_keep_alive(fx.handle.clone(), id.clone()).expect("toggle 1");
    let second = simple::toggle_keep_alive(fx.handle.clone(), id.clone()).expect("toggle 2");
    assert_ne!(first, second, "two toggles must return opposite states");

    fx.kill(&sid);
}

/// Add a second project, see both in the snapshot, list a project's branches,
/// and scan a directory — the projects route group.
#[test]
fn projects_add_list_branches_and_scan() {
    let Some(fx) = Fixture::start() else { return };

    // A second repo registered through the cdylib.
    let (second_repo, second_path) = fx.rt.block_on(create_test_repo());
    let new_id = simple::add_project(
        fx.handle.clone(),
        second_path.to_string_lossy().into_owned(),
    )
    .expect("add_project");
    assert!(
        Uuid::parse_str(&new_id).is_ok(),
        "add_project must return a valid project id"
    );

    let snap = simple::workspace_snapshot(fx.handle.clone()).unwrap();
    assert!(
        snap.projects.len() >= 2,
        "both the fixture repo and the added repo should be registered (got {})",
        snap.projects.len()
    );

    // Branches for the original project (its default branch must be present).
    let original = snap
        .projects
        .iter()
        .find(|p| p.repo_path == fx.repo_path.to_string_lossy())
        .expect("original project present");
    let branches =
        simple::list_branches(fx.handle.clone(), original.id.as_uuid().to_string(), false)
            .expect("list_branches");
    assert!(
        !branches.is_empty(),
        "a committed repo must have at least one branch"
    );

    // Scanning the added repo's parent directory must not error (it finds the
    // already-registered repo → reported as skipped).
    let parent = second_path
        .parent()
        .expect("repo has a parent dir")
        .to_string_lossy()
        .into_owned();
    let scan = simple::scan_directory(fx.handle.clone(), parent).expect("scan_directory");
    assert!(
        scan.added + scan.skipped >= 1,
        "scanning the parent dir should see the repo (added or skipped)"
    );

    drop(second_repo);
}

/// Seed a bare repo with one commit on `main` inside `dir` and return its
/// **plain local path** — the string a user pastes into the clone box, and what
/// `validate_clone_url` admits absolute local paths for.
///
/// Network-free by construction: the "remote" is a directory in a `TempDir`, so
/// this flow never reaches GitHub (and needs no `gh`, no credentials and no
/// connectivity in CI). A third copy of a fixture core also has in `git::clone`
/// and `api.rs` — both of those live in `#[cfg(test)]` modules, so neither is
/// reachable from an integration test in another crate.
async fn seed_bare_repo(dir: &TempDir) -> PathBuf {
    let remote = dir.path().join("remote.git");
    let seed = dir.path().join("seed");
    tokio::fs::create_dir_all(&seed).await.unwrap();
    run_git(dir.path(), &["init", "--bare", "-b", "main", "remote.git"]).await;
    run_git(&seed, &["init", "-b", "main"]).await;
    run_git(&seed, &["config", "user.email", "t@t.t"]).await;
    run_git(&seed, &["config", "user.name", "t"]).await;
    run_git(&seed, &["config", "commit.gpgsign", "false"]).await;
    tokio::fs::write(seed.join("README"), "v1\n").await.unwrap();
    run_git(&seed, &["add", "."]).await;
    run_git(&seed, &["commit", "-m", "initial"]).await;
    run_git(
        &seed,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    )
    .await;
    run_git(&seed, &["push", "origin", "main"]).await;
    remote
}

/// Poll a clone job through the client until it leaves `Running`, bounded so a
/// wedged clone fails the test rather than hanging it (~20s of real time).
///
/// The polling lives here, not in the client: `clone_job` is a single request by
/// design, and every frontend drives its own cadence.
async fn await_clone(client: &RemoteClient, id: CloneJobId) -> CloneJob {
    for _ in 0..1_000 {
        let job = client
            .clone_job(id)
            .await
            .expect("clone_job must not error")
            .expect("an in-flight job must not read as absent");
        if !matches!(job.status, CloneStatus::Running) {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("clone job {id} never reached a terminal status");
}

/// The clone route group end to end: start a clone from a local bare repo, poll
/// it to `Succeeded`, and confirm the checkout was registered as a project the
/// client can see.
///
/// Also pins the two answers a poller depends on: an unknown job id is
/// `Ok(None)` (not an error — Task 6's registry prunes terminal jobs, so a client
/// polling a pruned job must not see a hard failure), and `GET /github/repos`
/// resolves as a *route* even where `gh` is absent.
#[test]
fn clone_from_a_local_bare_repo_registers_a_project() {
    let Some(fx) = Fixture::start() else { return };
    let client = fx.remote_client();

    let remote_dir = TempDir::new().unwrap();
    let remote = fx.rt.block_on(seed_bare_repo(&remote_dir));

    // -- 202 Accepted carries the whole job, so the first status arrives with the
    // id rather than needing a second round trip.
    let started = fx
        .rt
        .block_on(client.start_clone(CloneRequest {
            source: CloneSource::Url {
                url: remote.to_string_lossy().into_owned(),
            },
            dest_name: Some("cloned-by-client".to_string()),
        }))
        .expect("start_clone");
    assert!(
        started.dest.ends_with("cloned-by-client"),
        "the job must report the destination it is writing to, got {:?}",
        started.dest
    );

    // -- poll to a terminal status --
    let done = fx.rt.block_on(await_clone(&client, started.id));
    let CloneStatus::Succeeded { project_id } = done.status else {
        panic!("clone did not succeed: {:?}", done.status)
    };
    assert_eq!(
        std::fs::read_to_string(done.dest.join("README")).unwrap(),
        "v1\n",
        "the checkout must contain the seeded commit's content"
    );

    // -- the clone is a registered project the client can see --
    //
    // Read through the workspace snapshot rather than a `GET /projects` call:
    // `RemoteClient` has no method for that route (projects reach a client in the
    // workspace snapshot), and adding one is outside this task's surface.
    let snap = simple::workspace_snapshot(fx.handle.clone()).expect("workspace_snapshot");
    // `ProjectInfoDto` is not `Debug` (it is an frb mirror), so report the paths.
    let paths: Vec<&str> = snap.projects.iter().map(|p| p.repo_path.as_str()).collect();
    assert!(
        snap.projects.iter().any(|p| p.id == project_id),
        "the cloned repo must be registered as a project, got {paths:?}"
    );

    // -- an unknown (or pruned) job id is a normal `None`, not an error --
    let unknown = fx
        .rt
        .block_on(client.clone_job(CloneJobId::new()))
        .expect("an unknown job id must not be an error");
    assert!(unknown.is_none(), "an unknown job id must read as absent");

    // -- `GET /github/repos` resolves. Its *contents* need an authenticated `gh`,
    // which a CI box need not have, so assert only that the route exists: a
    // `NotFound` would mean the client and the server disagree on the path,
    // which is exactly the drift this file catches.
    if let Err(err) = fx.rt.block_on(client.github_repos()) {
        assert!(
            !matches!(err, claude_commander_client::ClientError::NotFound),
            "GET /github/repos must resolve to the repos route, got {err:?}"
        );
    }
}

/// The cascade / push-stack operation-status route group: push-stack records an
/// operation, a bare resume records a failed operation, abandon with nothing
/// paused errors, and pr-refresh is a plain 202.
#[test]
fn cascade_push_stack_and_pr_refresh_plumbing() {
    let Some(fx) = Fixture::start() else { return };
    let sid = fx.create_session("cdylib-cascade");
    let id = full_id(&sid);

    // push_stack returns a recorded PushStack operation (the git push fails —
    // the test repo has no remote — but the route still records + returns it).
    let op = simple::push_stack(fx.handle.clone(), id.clone()).expect("push_stack");
    assert!(
        matches!(op.kind, OperationKind::PushStack),
        "push_stack must record a PushStack operation"
    );

    // Resuming with no cascade in progress is recorded as a *failed* Cascade op
    // (202 + status), not an error.
    let resumed = simple::cascade_resume(fx.handle.clone()).expect("cascade_resume");
    assert!(matches!(resumed.kind, OperationKind::Cascade));
    assert!(
        matches!(resumed.outcome.kind, OperationOutcomeKind::Failed),
        "resuming with nothing paused must record a failed operation"
    );

    // Abandoning with no cascade in progress surfaces a server error.
    assert!(
        simple::cascade_abandon(fx.handle.clone()).is_err(),
        "abandon with nothing paused must error"
    );

    // pr-refresh is a fire-and-forget 202.
    simple::request_pr_refresh(fx.handle.clone()).expect("request_pr_refresh");

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
    let worktree = fx.rt.block_on(async {
        fx.service
            .store()
            .read()
            .await
            .get_session(&sid)
            .expect("session exists")
            .worktree_path
            .clone()
    });
    std::fs::write(worktree.join("newfile.txt"), "hello from test\n").expect("write worktree file");

    // -- open_review shows the new file --
    let snap = review::open_review(fx.handle.clone(), id.clone()).expect("open_review");
    let file = snap
        .files
        .iter()
        .find(|f| f.display_path == "newfile.txt")
        .expect("newfile.txt should appear in the diff");
    assert!(!file.is_binary, "a text file should not be flagged binary");

    // -- create a comment on the added line, then list it --
    let comment_id = review::create_comment(
        fx.handle.clone(),
        id.clone(),
        "newfile.txt".to_string(),
        "new".to_string(),
        1,
        1,
        "hello from test".to_string(),
        "looks good".to_string(),
    )
    .expect("create_comment");
    let comments = review::list_comments(fx.handle.clone(), id.clone()).unwrap();
    assert!(
        comments.iter().any(|c| c.id == comment_id),
        "the created comment should be listed"
    );

    // -- toggle the file's reviewed mark on --
    let reviewed =
        review::toggle_file_reviewed(fx.handle.clone(), id.clone(), "newfile.txt".to_string())
            .expect("toggle_file_reviewed");
    assert!(
        reviewed,
        "toggling an un-reviewed file should mark it reviewed"
    );

    // -- a path that isn't in the current diff is a 404, not a silent no-op --
    assert!(
        review::toggle_file_reviewed(
            fx.handle.clone(),
            id.clone(),
            "no-such-file.txt".to_string(),
        )
        .is_err(),
        "toggling a file absent from the diff must error"
    );

    // -- apply the staged comment: composed + delivered (or deferred) --
    let result = review::apply_comments(fx.handle.clone(), id.clone()).expect("apply");
    assert!(
        matches!(
            result.kind,
            review::ApplyResultKind::Applied | review::ApplyResultKind::Deferred
        ),
        "a single fresh comment should apply or defer, not block/no-op"
    );
    assert_eq!(result.count, 1, "exactly one comment should be composed");

    // -- the raw composition reaches the client, and lays out --
    //
    // The whole point of shipping `raw`: the client can run the layout engine
    // itself rather than re-deriving rows from the lossier wire model. Proving
    // it against a live server is the only place the protocol field, the HTTP
    // decode and `diff_rows` are exercised together.
    let snap = review::open_review(fx.handle.clone(), id.clone()).expect("re-open");
    let raw = snap.raw.clone().expect("the server must ship the raw diff");
    assert!(
        raw.contains("+++ b/newfile.txt"),
        "unexpected raw diff: {raw}"
    );
    let file = snap
        .files
        .into_iter()
        .find(|f| f.display_path == "newfile.txt")
        .expect("newfile.txt is in the diff");
    let layout = diff::diff_rows(
        Some(raw),
        file,
        diff::DiffLayoutMode::Inline,
        None,
        Vec::new(),
        4,
    )
    .expect("diff_rows");
    let added: Vec<String> = layout
        .rows
        .iter()
        .filter(|r| matches!(r.left.origin, Some(review::ReviewLineOrigin::Addition)))
        .map(|r| r.left.spans.iter().map(|s| s.text.as_str()).collect())
        .collect();
    assert_eq!(added, vec!["hello from test".to_string()]);

    // -- an unchanged refresh short-circuits (204 → None) --
    let latest = review::open_review(fx.handle.clone(), id.clone()).unwrap();
    let refreshed =
        review::refresh_review(fx.handle.clone(), id.clone(), latest.content_hash).unwrap();
    assert!(
        refreshed.is_none(),
        "refresh with the current content hash should report no change"
    );

    fx.kill(&sid);
}
