//! Integration tests for claude-commander
//!
//! These tests require tmux to be installed and available.
//! All tests use isolated state files to avoid polluting user data.

use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;

use claude_commander::SessionStatus;
use claude_commander::cli_args::cli_command;
use claude_commander::commander::{self, COMMANDER_TMUX_NAME};
use claude_commander::config::{AppState, Config, ConfigStore, StateStore};
use claude_commander::git::GitBackend;
use claude_commander::session::SessionManager;
use claude_commander::tmux::TmuxExecutor;

/// Helper to create an isolated StateStore that won't pollute user data
fn create_isolated_store(temp_dir: &TempDir) -> Arc<StateStore> {
    let state_path = temp_dir.path().join("state.json");
    let state = AppState::load_from(&state_path).unwrap();
    Arc::new(StateStore::with_path(state, state_path))
}

/// Helper to create an isolated ConfigStore for testing
fn create_isolated_config_store(temp_dir: &TempDir, config: Config) -> Arc<ConfigStore> {
    let config_path = temp_dir.path().join("config.toml");
    let toml = toml::to_string_pretty(&config).unwrap();
    std::fs::write(&config_path, toml).unwrap();
    Arc::new(ConfigStore::with_path(config, config_path))
}

/// Helper to create a test git repository
async fn create_test_repo() -> (TempDir, PathBuf) {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path().to_path_buf();

    // Initialize git repo
    tokio::process::Command::new("git")
        .current_dir(&repo_path)
        .args(["init"])
        .output()
        .await
        .unwrap();

    // Configure git user for commits
    tokio::process::Command::new("git")
        .current_dir(&repo_path)
        .args(["config", "user.email", "test@test.com"])
        .output()
        .await
        .unwrap();

    tokio::process::Command::new("git")
        .current_dir(&repo_path)
        .args(["config", "user.name", "Test User"])
        .output()
        .await
        .unwrap();

    // Create initial commit
    let readme_path = repo_path.join("README.md");
    tokio::fs::write(&readme_path, "# Test Repository\n")
        .await
        .unwrap();

    tokio::process::Command::new("git")
        .current_dir(&repo_path)
        .args(["add", "README.md"])
        .output()
        .await
        .unwrap();

    tokio::process::Command::new("git")
        .current_dir(&repo_path)
        .args(["commit", "-m", "Initial commit"])
        .output()
        .await
        .unwrap();

    (temp_dir, repo_path)
}

/// Initialise a minimal committed git repo at `path` (creating `path` and any
/// parent directories first). Used to build scan-directory fixtures.
async fn init_repo_at(path: &std::path::Path) {
    tokio::fs::create_dir_all(path).await.unwrap();
    run_git(path, &["init"]).await;
    run_git(path, &["config", "user.email", "test@test.com"]).await;
    run_git(path, &["config", "user.name", "Test User"]).await;
    tokio::fs::write(path.join("README.md"), "# repo\n")
        .await
        .unwrap();
    run_git(path, &["add", "."]).await;
    run_git(path, &["commit", "-m", "init"]).await;
}

/// Run a git command in `dir`, asserting it succeeds.
async fn run_git(dir: &std::path::Path, args: &[&str]) {
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

/// Run a git command in `dir` and return its trimmed stdout.
async fn git_stdout(dir: &std::path::Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Helper to check if tmux is available
async fn tmux_available() -> bool {
    tokio::process::Command::new("tmux")
        .arg("-V")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn test_git_backend_open() {
    let (_temp_dir, repo_path) = create_test_repo().await;

    let backend = GitBackend::open(&repo_path);
    assert!(backend.is_ok(), "Should open git repository");

    let backend = backend.unwrap();
    assert!(!backend.repo_name().is_empty());
}

#[tokio::test]
async fn test_git_backend_discover() {
    let (_temp_dir, repo_path) = create_test_repo().await;

    // Create a subdirectory
    let subdir = repo_path.join("subdir");
    tokio::fs::create_dir_all(&subdir).await.unwrap();

    // Discover from subdirectory
    let backend = GitBackend::discover(&subdir);
    assert!(
        backend.is_ok(),
        "Should discover git repository from subdirectory"
    );
}

#[tokio::test]
async fn test_git_backend_branch_detection() {
    let (_temp_dir, repo_path) = create_test_repo().await;

    let backend = GitBackend::open(&repo_path).unwrap();

    // Should detect main branch (git init uses 'master' or 'main' depending on config)
    let branch = backend.current_branch();
    assert!(branch.is_ok(), "Should get current branch");

    let main_branch = backend.detect_main_branch();
    assert!(main_branch.is_ok(), "Should detect main branch");
}

#[tokio::test]
async fn test_session_manager_add_project() {
    if !tmux_available().await {
        eprintln!("Skipping test: tmux not available");
        return;
    }

    let (repo_temp_dir, repo_path) = create_test_repo().await;
    let state_temp_dir = TempDir::new().unwrap();

    let config = Config::default();
    let config_store = create_isolated_config_store(&state_temp_dir, config);
    let store = create_isolated_store(&state_temp_dir);
    let manager = SessionManager::new(config_store, store.clone(), "");

    // Add project
    let result = manager.add_project(repo_path.clone()).await;
    assert!(result.is_ok(), "Should add project: {:?}", result.err());

    let project_id = result.unwrap();

    // Verify project was added
    let state = store.read().await;
    assert!(state.get_project(&project_id).is_some());

    // Keep temp dirs alive until end of test
    drop(repo_temp_dir);
    drop(state_temp_dir);
}

/// `scan_directory` must walk a tree, register each repo once, prune repos
/// nested inside another repo, and skip duplicates on re-scan. This exercises
/// the `spawn_blocking` directory walk (`find_git_repos`) and the
/// canonical-path dedup end-to-end.
#[tokio::test]
async fn test_scan_directory_discovers_dedupes_and_prunes() {
    if !tmux_available().await {
        eprintln!("Skipping test: tmux not available");
        return;
    }

    let scan_root = TempDir::new().unwrap();
    let root = scan_root.path();

    // Two repos at different depths — both must be discovered.
    init_repo_at(&root.join("repo_a")).await;
    init_repo_at(&root.join("nested/repo_b")).await;
    // A repo nested *inside* repo_a — must be pruned (walk never descends into
    // a discovered repo), so it must NOT be registered separately.
    init_repo_at(&root.join("repo_a/inner_repo")).await;
    // A plain non-repo directory with a file — must be ignored entirely.
    tokio::fs::create_dir_all(root.join("plain")).await.unwrap();
    tokio::fs::write(root.join("plain/file.txt"), "x")
        .await
        .unwrap();

    let state_temp_dir = TempDir::new().unwrap();
    let config_store = create_isolated_config_store(&state_temp_dir, Config::default());
    let store = create_isolated_store(&state_temp_dir);
    let manager = SessionManager::new(config_store, store.clone(), "");

    // First scan: discovers repo_a and nested/repo_b; inner_repo is pruned.
    let result = manager.scan_directory(root).await.unwrap();
    assert_eq!(
        result.added, 2,
        "should add only repo_a and nested/repo_b (inner repo pruned)"
    );
    assert_eq!(result.skipped, 0);
    assert_eq!(
        store.read().await.project_count(),
        2,
        "exactly the two top-level repos must be registered"
    );

    // Second scan over the same tree: every repo is now a known duplicate.
    let result = manager.scan_directory(root).await.unwrap();
    assert_eq!(result.added, 0, "re-scan must add nothing");
    assert_eq!(
        result.skipped, 2,
        "both existing repos must be skipped as duplicates"
    );
    assert_eq!(
        store.read().await.project_count(),
        2,
        "re-scan must not create duplicate projects"
    );
}

#[tokio::test]
async fn test_session_manager_create_session() {
    if !tmux_available().await {
        eprintln!("Skipping test: tmux not available");
        return;
    }

    let (repo_temp_dir, repo_path) = create_test_repo().await;
    let state_temp_dir = TempDir::new().unwrap();

    // Create temp worktrees dir
    let worktrees_dir = TempDir::new().unwrap();
    let config = Config {
        worktrees_dir: Some(worktrees_dir.path().to_path_buf()),
        ..Config::default()
    };

    let config_store = create_isolated_config_store(&state_temp_dir, config);
    let store = create_isolated_store(&state_temp_dir);
    let manager = SessionManager::new(config_store, store.clone(), "");

    // Add project
    let project_id = manager.add_project(repo_path).await.unwrap();

    // Create session (prepare + finalize)
    let session_id = manager
        .prepare_session(
            &project_id,
            "test-session".to_string(),
            Some("bash".to_string()),
            None,
        )
        .await
        .expect("prepare_session should succeed");

    let result = manager.finalize_session(&session_id, None, None).await;

    if let Err(e) = &result {
        eprintln!("Error finalizing session: {}", e);
    }

    assert!(result.is_ok(), "Should finalize session");

    let session_id = result.unwrap();

    // Verify session was created
    {
        let state = store.read().await;
        let session = state.get_session(&session_id);
        assert!(session.is_some(), "Session should exist in state");

        let session = session.unwrap();
        assert_eq!(session.title, "test-session");
        assert_eq!(session.program, "bash");
    }

    // Cleanup: kill the tmux session
    let _ = manager.kill_session(&session_id, true).await;

    // Keep temp dirs alive until end of test
    drop(repo_temp_dir);
    drop(state_temp_dir);
    drop(worktrees_dir);
}

#[tokio::test]
async fn test_session_manager_restart() {
    if !tmux_available().await {
        eprintln!("Skipping test: tmux not available");
        return;
    }

    let (repo_temp_dir, repo_path) = create_test_repo().await;
    let state_temp_dir = TempDir::new().unwrap();

    let worktrees_dir = TempDir::new().unwrap();
    let config = Config {
        worktrees_dir: Some(worktrees_dir.path().to_path_buf()),
        ..Config::default()
    };

    let store = create_isolated_store(&state_temp_dir);
    let config_store = Arc::new(ConfigStore::new(config).unwrap());
    let manager = SessionManager::new(config_store, store.clone(), "");

    // Add project and create session (prepare + finalize)
    let project_id = manager.add_project(repo_path).await.unwrap();
    let session_id = manager
        .prepare_session(
            &project_id,
            "restart-test".to_string(),
            Some("bash".to_string()),
            None,
        )
        .await
        .unwrap();
    manager
        .finalize_session(&session_id, None, None)
        .await
        .unwrap();

    // Verify initial status is Running
    {
        let state = store.read().await;
        let session = state.get_session(&session_id).unwrap();
        assert_eq!(session.status, SessionStatus::Running);
    }

    // Restart from Running state
    manager
        .restart_session(&session_id)
        .await
        .expect("Should restart running session");

    // Verify still Running after restart
    {
        let state = store.read().await;
        let session = state.get_session(&session_id).unwrap();
        assert_eq!(session.status, SessionStatus::Running);
    }

    // Kill (-> Stopped), then restart from Stopped state
    manager.kill_session(&session_id, false).await.unwrap();
    {
        let state = store.read().await;
        let session = state.get_session(&session_id).unwrap();
        assert_eq!(session.status, SessionStatus::Stopped);
    }

    manager
        .restart_session(&session_id)
        .await
        .expect("Should restart stopped session");

    {
        let state = store.read().await;
        let session = state.get_session(&session_id).unwrap();
        assert_eq!(session.status, SessionStatus::Running);
    }

    // Cleanup
    let _ = manager.kill_session(&session_id, true).await;

    drop(repo_temp_dir);
    drop(state_temp_dir);
    drop(worktrees_dir);
}

#[tokio::test]
async fn test_state_persistence() {
    let temp_dir = TempDir::new().unwrap();
    let state_path = temp_dir.path().join("state.json");

    // Create and save state
    {
        let mut state = AppState::new();
        let project =
            claude_commander::Project::new("test-project", PathBuf::from("/tmp/test"), "main");
        state.add_project(project);
        state.save_to(&state_path).unwrap();
    }

    // Load and verify
    {
        let state = AppState::load_from(&state_path).unwrap();
        assert_eq!(state.project_count(), 1);
    }
}

#[tokio::test]
async fn test_config_defaults() {
    let config = Config::default();

    assert_eq!(config.default_program, "claude");
    assert_eq!(config.branch_prefix, "");
    assert_eq!(config.max_concurrent_tmux, 16);
    assert_eq!(config.capture_cache_ttl_ms, 50);
    assert_eq!(config.diff_cache_ttl_ms, 500);
    assert_eq!(config.ui_refresh_fps, 30);
}

#[tokio::test]
async fn test_sync_worktrees_imports_external() {
    if !tmux_available().await {
        eprintln!("Skipping test: tmux not available");
        return;
    }

    let (repo_temp_dir, repo_path) = create_test_repo().await;
    let state_temp_dir = TempDir::new().unwrap();

    let worktrees_dir = TempDir::new().unwrap();
    let config = Config {
        worktrees_dir: Some(worktrees_dir.path().to_path_buf()),
        ..Config::default()
    };

    let config_store = create_isolated_config_store(&state_temp_dir, config);
    let store = create_isolated_store(&state_temp_dir);
    let manager = SessionManager::new(config_store, store.clone(), "");

    // Add project (no worktrees yet)
    let project_id = manager.add_project(repo_path.clone()).await.unwrap();

    // Verify no sessions were imported (no external worktrees exist)
    {
        let st = store.read().await;
        let project = st.get_project(&project_id).unwrap();
        assert_eq!(project.worktrees.len(), 0, "No sessions should exist yet");
    }

    // Create an external worktree via git CLI (simulating Claude Code /worktree or manual creation)
    let external_wt_path = worktrees_dir.path().join("external-feature");
    let output = tokio::process::Command::new("git")
        .current_dir(&repo_path)
        .args([
            "worktree",
            "add",
            "-b",
            "external-feature",
            external_wt_path.to_str().unwrap(),
        ])
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "git worktree add should succeed");

    // Run sync_worktrees - should import the external worktree
    let imported = manager.sync_worktrees(&project_id).await.unwrap();
    assert_eq!(imported, 1, "Should import 1 external worktree");

    // Verify the imported session
    {
        let st = store.read().await;
        let project = st.get_project(&project_id).unwrap();
        assert_eq!(project.worktrees.len(), 1, "Should have 1 session");

        let session = st.get_session(&project.worktrees[0]).unwrap();
        assert_eq!(session.branch, "external-feature");
        assert_eq!(session.status, SessionStatus::Stopped);
        assert!(session.base_commit.is_some());
    }

    // Run sync again - should be idempotent
    let imported_again = manager.sync_worktrees(&project_id).await.unwrap();
    assert_eq!(
        imported_again, 0,
        "Second sync should import 0 (idempotent)"
    );

    // Keep temp dirs alive
    drop(repo_temp_dir);
    drop(state_temp_dir);
    drop(worktrees_dir);
}

/// Helper to create a bare repo as "origin" and a working repo with that remote configured.
async fn create_test_repo_with_remote() -> (TempDir, PathBuf, TempDir, PathBuf) {
    // Create bare "origin" repo
    let bare_dir = TempDir::new().unwrap();
    let bare_path = bare_dir.path().to_path_buf();

    tokio::process::Command::new("git")
        .current_dir(&bare_path)
        .args(["init", "--bare"])
        .output()
        .await
        .unwrap();

    // Create working repo
    let work_dir = TempDir::new().unwrap();
    let work_path = work_dir.path().to_path_buf();

    tokio::process::Command::new("git")
        .current_dir(&work_path)
        .args(["init"])
        .output()
        .await
        .unwrap();

    // Configure git user
    for args in [
        vec!["config", "user.email", "test@test.com"],
        vec!["config", "user.name", "Test User"],
    ] {
        tokio::process::Command::new("git")
            .current_dir(&work_path)
            .args(&args)
            .output()
            .await
            .unwrap();
    }

    // Add remote
    tokio::process::Command::new("git")
        .current_dir(&work_path)
        .args(["remote", "add", "origin", bare_path.to_str().unwrap()])
        .output()
        .await
        .unwrap();

    // Create initial commit and push
    tokio::fs::write(work_path.join("README.md"), "# Test\n")
        .await
        .unwrap();

    tokio::process::Command::new("git")
        .current_dir(&work_path)
        .args(["add", "README.md"])
        .output()
        .await
        .unwrap();

    tokio::process::Command::new("git")
        .current_dir(&work_path)
        .args(["commit", "-m", "Initial commit"])
        .output()
        .await
        .unwrap();

    tokio::process::Command::new("git")
        .current_dir(&work_path)
        .args(["push", "-u", "origin", "HEAD"])
        .output()
        .await
        .unwrap();

    (bare_dir, bare_path, work_dir, work_path)
}

#[tokio::test]
async fn test_detect_main_branch_with_remote() {
    let (_bare_dir, _bare_path, _work_dir, work_path) = create_test_repo_with_remote().await;

    // Set origin/HEAD so remote_default_branch() can resolve it
    tokio::process::Command::new("git")
        .current_dir(&work_path)
        .args(["remote", "set-head", "origin", "--auto"])
        .output()
        .await
        .unwrap();

    let backend = GitBackend::open(&work_path).unwrap();
    let main = backend.detect_main_branch().unwrap();

    // The default branch should be whatever the working repo's HEAD is
    let current = backend.current_branch().unwrap();
    assert_eq!(main, current);
}

#[tokio::test]
async fn test_create_session_no_remote_falls_back() {
    if !tmux_available().await {
        eprintln!("Skipping test: tmux not available");
        return;
    }

    // Repo with no remote — fetch_before_create: true should still succeed
    let (repo_temp_dir, repo_path) = create_test_repo().await;
    let state_temp_dir = TempDir::new().unwrap();
    let worktrees_dir = TempDir::new().unwrap();

    let config = Config {
        worktrees_dir: Some(worktrees_dir.path().to_path_buf()),
        fetch_before_create: true,
        ..Config::default()
    };

    let config_store = create_isolated_config_store(&state_temp_dir, config);
    let store = create_isolated_store(&state_temp_dir);
    let manager = SessionManager::new(config_store, store.clone(), "");

    let project_id = manager.add_project(repo_path).await.unwrap();

    let session_id = manager
        .prepare_session(
            &project_id,
            "fallback-test".to_string(),
            Some("bash".to_string()),
            None,
        )
        .await
        .expect("prepare_session should succeed");

    let result = manager.finalize_session(&session_id, None, None).await;

    assert!(
        result.is_ok(),
        "Session finalization should succeed without remote: {:?}",
        result.err()
    );

    let session_id = result.unwrap();
    let _ = manager.kill_session(&session_id, true).await;

    drop(repo_temp_dir);
    drop(state_temp_dir);
    drop(worktrees_dir);
}

/// When `--base-branch` matches an existing session's branch in the same
/// project, the new session should be linked as stacked via
/// `stack_parent_session_id`. This mirrors the TUI's stacked-create flow.
#[tokio::test]
async fn test_base_branch_links_stack_parent_when_session_matches() {
    if !tmux_available().await {
        eprintln!("Skipping test: tmux not available");
        return;
    }

    let (repo_temp_dir, repo_path) = create_test_repo().await;
    let state_temp_dir = TempDir::new().unwrap();
    let worktrees_dir = TempDir::new().unwrap();
    let config = Config {
        worktrees_dir: Some(worktrees_dir.path().to_path_buf()),
        ..Config::default()
    };

    let config_store = create_isolated_config_store(&state_temp_dir, config);
    let store = create_isolated_store(&state_temp_dir);
    let manager = SessionManager::new(config_store, store.clone(), "");

    let project_id = manager.add_project(repo_path).await.unwrap();

    // Create parent session
    let parent_id = manager
        .prepare_session(
            &project_id,
            "parent-session".to_string(),
            Some("bash".to_string()),
            None,
        )
        .await
        .unwrap();
    manager
        .finalize_session(&parent_id, None, None)
        .await
        .unwrap();

    let parent_branch = {
        let state = store.read().await;
        state.get_session(&parent_id).unwrap().branch.clone()
    };

    // Create child session and link it to the parent via branch name
    let child_id = manager
        .prepare_session(
            &project_id,
            "child-session".to_string(),
            Some("bash".to_string()),
            None,
        )
        .await
        .unwrap();
    manager
        .link_stack_parent_by_branch(&child_id, Some(&parent_branch))
        .await
        .unwrap();
    manager
        .finalize_session(&child_id, None, None)
        .await
        .unwrap();

    // Verify the child is linked to the parent
    {
        let state = store.read().await;
        let child = state.get_session(&child_id).unwrap();
        assert_eq!(
            child.stack_parent_session_id,
            Some(parent_id),
            "child session should be linked to parent via stack_parent_session_id"
        );
    }

    let _ = manager.kill_session(&child_id, true).await;
    let _ = manager.kill_session(&parent_id, true).await;
    drop(repo_temp_dir);
    drop(state_temp_dir);
    drop(worktrees_dir);
}

/// When `--base-branch` doesn't match any existing session's branch,
/// `stack_parent_session_id` should remain None.
#[tokio::test]
async fn test_base_branch_no_link_when_no_session_matches() {
    if !tmux_available().await {
        eprintln!("Skipping test: tmux not available");
        return;
    }

    let (repo_temp_dir, repo_path) = create_test_repo().await;
    let state_temp_dir = TempDir::new().unwrap();
    let worktrees_dir = TempDir::new().unwrap();
    let config = Config {
        worktrees_dir: Some(worktrees_dir.path().to_path_buf()),
        ..Config::default()
    };

    let config_store = create_isolated_config_store(&state_temp_dir, config);
    let store = create_isolated_store(&state_temp_dir);
    let manager = SessionManager::new(config_store, store.clone(), "");

    let project_id = manager.add_project(repo_path).await.unwrap();

    // Create session with base_branch that doesn't match any session
    let session_id = manager
        .prepare_session(
            &project_id,
            "standalone-session".to_string(),
            Some("bash".to_string()),
            Some("develop".to_string()),
        )
        .await
        .unwrap();

    // Link attempt — should be a no-op since no session has branch "develop"
    manager
        .link_stack_parent_by_branch(&session_id, Some("develop"))
        .await
        .unwrap();
    manager
        .finalize_session(&session_id, None, None)
        .await
        .unwrap();

    // Verify no stack link
    {
        let state = store.read().await;
        let session = state.get_session(&session_id).unwrap();
        assert_eq!(
            session.stack_parent_session_id, None,
            "session should not be linked when base_branch doesn't match any session"
        );
    }

    let _ = manager.kill_session(&session_id, true).await;
    drop(repo_temp_dir);
    drop(state_temp_dir);
    drop(worktrees_dir);
}

/// `--base-branch <branch>` for a plain branch not owned by any session (e.g.
/// `develop`) must create a NEW branch for the session, forked off the base —
/// not reuse the base branch as the session's own branch. Replicates the
/// corrected main.rs CLI flow: generate a fresh branch (None to
/// prepare_session), attempt a (no-op) stack link, then fork off the base in
/// finalize_session.
#[tokio::test]
async fn test_base_branch_forks_new_branch_off_base() {
    if !tmux_available().await {
        eprintln!("Skipping test: tmux not available");
        return;
    }

    let (repo_temp_dir, repo_path) = create_test_repo().await;

    // Create a `develop` branch with a commit that is NOT on the default
    // branch, so we can prove the new worktree was forked from develop's tip.
    let default_branch = git_stdout(&repo_path, &["rev-parse", "--abbrev-ref", "HEAD"]).await;
    run_git(&repo_path, &["checkout", "-b", "develop"]).await;
    tokio::fs::write(repo_path.join("develop.txt"), "develop\n")
        .await
        .unwrap();
    run_git(&repo_path, &["add", "develop.txt"]).await;
    run_git(&repo_path, &["commit", "-m", "develop commit"]).await;
    let develop_tip = git_stdout(&repo_path, &["rev-parse", "HEAD"]).await;
    // Leave develop un-checked-out so it can't be confused with the session's
    // own branch.
    run_git(&repo_path, &["checkout", &default_branch]).await;

    let state_temp_dir = TempDir::new().unwrap();
    let worktrees_dir = TempDir::new().unwrap();
    let config = Config {
        worktrees_dir: Some(worktrees_dir.path().to_path_buf()),
        ..Config::default()
    };

    let config_store = create_isolated_config_store(&state_temp_dir, config);
    let store = create_isolated_store(&state_temp_dir);
    let manager = SessionManager::new(config_store, store.clone(), "");

    let project_id = manager.add_project(repo_path.clone()).await.unwrap();

    let session_id = manager
        .prepare_session(
            &project_id,
            "my-feature".to_string(),
            Some("bash".to_string()),
            None,
        )
        .await
        .unwrap();
    manager
        .link_stack_parent_by_branch(&session_id, Some("develop"))
        .await
        .unwrap();
    manager
        .finalize_session(&session_id, None, Some("develop".to_string()))
        .await
        .unwrap();

    let (branch, worktree_path) = {
        let state = store.read().await;
        let s = state.get_session(&session_id).unwrap();
        (s.branch.clone(), s.worktree_path.clone())
    };

    // The session must get its own generated branch, not reuse "develop".
    assert_ne!(
        branch, "develop",
        "session should get its own generated branch, not the base branch"
    );

    // The new branch must be forked from develop's tip (not the default branch).
    let worktree_tip = git_stdout(&worktree_path, &["rev-parse", "HEAD"]).await;
    assert_eq!(
        worktree_tip, develop_tip,
        "new session branch should be forked from the base branch (develop) tip"
    );

    let _ = manager.kill_session(&session_id, true).await;
    drop(repo_temp_dir);
    drop(state_temp_dir);
    drop(worktrees_dir);
}

/// When `--base-branch` matches a session's branch, the child must get its
/// own branch (not the parent's) to avoid git rejecting a second worktree on
/// the same branch. This replicates the full main.rs flow: detect match →
/// withhold base_branch from prepare_session → link → finalize.
#[tokio::test]
async fn test_stacked_session_gets_own_branch_not_parents() {
    if !tmux_available().await {
        eprintln!("Skipping test: tmux not available");
        return;
    }

    let (repo_temp_dir, repo_path) = create_test_repo().await;
    let state_temp_dir = TempDir::new().unwrap();
    let worktrees_dir = TempDir::new().unwrap();
    let config = Config {
        worktrees_dir: Some(worktrees_dir.path().to_path_buf()),
        ..Config::default()
    };

    let config_store = create_isolated_config_store(&state_temp_dir, config);
    let store = create_isolated_store(&state_temp_dir);
    let manager = SessionManager::new(config_store, store.clone(), "");

    let project_id = manager.add_project(repo_path).await.unwrap();

    // Create parent session (gets branch "parent-session")
    let parent_id = manager
        .prepare_session(
            &project_id,
            "parent-session".to_string(),
            Some("bash".to_string()),
            None,
        )
        .await
        .unwrap();
    manager
        .finalize_session(&parent_id, None, None)
        .await
        .unwrap();

    let parent_branch = {
        let state = store.read().await;
        state.get_session(&parent_id).unwrap().branch.clone()
    };

    // Replicate main.rs logic: detect that base_branch matches a session,
    // so withhold it from prepare_session (child gets own branch from title)
    let base_branch = Some(parent_branch.clone());
    let is_stacked = {
        let state = store.read().await;
        base_branch.as_ref().is_some_and(|base| {
            state
                .sessions
                .values()
                .any(|s| s.project_id == project_id && s.branch == *base)
        })
    };
    assert!(is_stacked, "base_branch should match parent session");
    let branch_for_prepare = if is_stacked {
        None
    } else {
        base_branch.clone()
    };

    let child_id = manager
        .prepare_session(
            &project_id,
            "child-session".to_string(),
            Some("bash".to_string()),
            branch_for_prepare,
        )
        .await
        .unwrap();
    manager
        .link_stack_parent_by_branch(&child_id, base_branch.as_deref())
        .await
        .unwrap();

    // This would fail with "branch already used by worktree" if we had
    // passed the parent's branch to prepare_session
    manager
        .finalize_session(&child_id, None, None)
        .await
        .unwrap();

    // Verify child has its own branch, not the parent's
    {
        let state = store.read().await;
        let child = state.get_session(&child_id).unwrap();
        let parent = state.get_session(&parent_id).unwrap();
        assert_ne!(
            child.branch, parent.branch,
            "child should have its own branch, not the parent's"
        );
        assert_eq!(
            child.stack_parent_session_id,
            Some(parent_id),
            "child should be linked to parent"
        );
    }

    let _ = manager.kill_session(&child_id, true).await;
    let _ = manager.kill_session(&parent_id, true).await;
    drop(repo_temp_dir);
    drop(state_temp_dir);
    drop(worktrees_dir);
}

/// Full commander session lifecycle in one test so the scenarios run
/// sequentially against the single global `cc-commander` tmux session (Rust
/// runs separate test fns concurrently, which would collide on the name).
#[tokio::test]
async fn test_commander_session_lifecycle() {
    if !tmux_available().await {
        eprintln!("Skipping test: tmux not available");
        return;
    }

    let tmux = TmuxExecutor::new();
    // Never touch a real commander the developer may be running.
    if tmux
        .session_exists(COMMANDER_TMUX_NAME)
        .await
        .unwrap_or(false)
    {
        eprintln!("Skipping test: a `{COMMANDER_TMUX_NAME}` session already exists");
        return;
    }

    // Best-effort cleanup so a panicking assertion can't leak the global
    // `cc-commander` session into later tests or the developer's tmux. Drop
    // can't await, so shell out to tmux synchronously. Instantiated only after
    // the "already exists" guard, so it never kills a session we didn't create.
    struct KillOnDrop;
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = std::process::Command::new("tmux")
                .args(["kill-session", "-t", COMMANDER_TMUX_NAME])
                .status();
        }
    }
    let _cleanup = KillOnDrop;

    let dir = TempDir::new().unwrap();
    let cmd = cli_command();
    let live_config = Config {
        commander_enabled: true,
        commander_dir: Some(dir.path().to_path_buf()),
        commander_program: Some("sleep 60".to_string()),
        ..Config::default()
    };

    // --- Create + priming files ---
    let name = commander::ensure_session(&live_config, &tmux, &cmd)
        .await
        .unwrap();
    assert_eq!(name, COMMANDER_TMUX_NAME);
    assert!(dir.path().join("CLAUDE.md").exists(), "CLAUDE.md written");
    assert!(dir.path().join("NOTES.md").exists(), "NOTES.md seeded");
    assert!(commander::is_running(&tmux).await, "live session runs");

    // --- Idempotent reuse: second call must not error or double-create ---
    commander::ensure_session(&live_config, &tmux, &cmd)
        .await
        .unwrap();
    assert!(
        commander::is_running(&tmux).await,
        "session still running after idempotent second call"
    );

    tmux.kill_session(COMMANDER_TMUX_NAME).await.unwrap();

    // --- Dead-pane revival: the corpse-reattach regression ---
    // A program that exits immediately leaves a dead-but-existing pane
    // (remain-on-exit is on globally).
    let dead_config = Config {
        commander_program: Some("true".to_string()),
        ..live_config.clone()
    };
    commander::ensure_session(&dead_config, &tmux, &cmd)
        .await
        .unwrap();

    let mut dead = false;
    for _ in 0..100 {
        if tmux
            .is_pane_dead(COMMANDER_TMUX_NAME)
            .await
            .unwrap_or(false)
        {
            dead = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(dead, "commander pane should die after `true` exits");
    assert!(
        !commander::is_running(&tmux).await,
        "a dead pane must not report as running"
    );

    // ensure_session must KILL the corpse and recreate a live session.
    commander::ensure_session(&live_config, &tmux, &cmd)
        .await
        .unwrap();
    assert!(
        commander::is_running(&tmux).await,
        "ensure_session must revive a dead commander into a running one"
    );

    // `_cleanup` (KillOnDrop) tears down the session as the scope unwinds —
    // on success here and on a panic at any assertion above.
    drop(dir);
}

/// Helper to check if git-lfs is available.
async fn git_lfs_available() -> bool {
    tokio::process::Command::new("git")
        .args(["lfs", "version"])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// git-LFS pointer resolution: a committed LFS-tracked PNG is stored as a
/// pointer, so `read_base_blob` must smudge it back to the real image bytes
/// rather than returning the pointer text. Skips gracefully when `git lfs`
/// isn't installed (mirrors the tmux-guarded tests).
#[tokio::test]
async fn read_base_blob_resolves_lfs_pointer_to_real_bytes() {
    if !git_lfs_available().await {
        eprintln!("Skipping test: git-lfs not available");
        return;
    }

    let (_temp, repo) = create_test_repo().await;

    // A minimal 1x1 PNG: signature + IHDR. Embedded NULs make git treat it as
    // binary, and the leading magic lets us assert smudge returned real bytes.
    let png: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR\
\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89";

    run_git(&repo, &["lfs", "install", "--local"]).await;
    run_git(&repo, &["lfs", "track", "*.png"]).await;
    tokio::fs::write(repo.join("img.png"), png).await.unwrap();
    run_git(&repo, &["add", ".gitattributes", "img.png"]).await;
    run_git(&repo, &["commit", "-m", "add lfs image"]).await;

    // Sanity: the committed blob is the LFS pointer text, not the PNG.
    let stored = git_stdout(&repo, &["show", "HEAD:img.png"]).await;
    assert!(
        stored.starts_with("version https://git-lfs.github.com/spec/"),
        "expected committed blob to be an LFS pointer, got: {stored:.40}"
    );

    // read_base_blob must resolve the pointer to the real PNG bytes.
    let base = claude_commander::git::read_base_blob(&repo, "HEAD", "img.png")
        .await
        .expect("read_base_blob should succeed");
    assert_eq!(
        &base[..4],
        b"\x89PNG",
        "base side should be smudged PNG bytes"
    );
    assert_eq!(base, png, "base side should round-trip the original image");

    // The working-tree file is smudged on checkout, so it's already real bytes;
    // read_worktree_file passes them through unchanged.
    let new = claude_commander::git::read_worktree_file(&repo, "img.png")
        .await
        .expect("read_worktree_file should succeed");
    assert_eq!(new, png, "new side should be the real image bytes");
}

#[tokio::test]
async fn test_hibernate_session_keeps_worktree_and_wakes_with_resume() {
    if !tmux_available().await {
        eprintln!("Skipping test: tmux not available");
        return;
    }

    let (repo_temp_dir, repo_path) = create_test_repo().await;
    let state_temp_dir = TempDir::new().unwrap();
    let worktrees_dir = TempDir::new().unwrap();

    // resume_session = false so the test proves hibernation wakes a session
    // *regardless* of the global flag (the `hibernated` marker forces it).
    let config = Config {
        worktrees_dir: Some(worktrees_dir.path().to_path_buf()),
        resume_session: false,
        ..Config::default()
    };

    let store = create_isolated_store(&state_temp_dir);
    let config_store = Arc::new(ConfigStore::new(config).unwrap());
    let manager = SessionManager::new(config_store, store.clone(), "");

    let project_id = manager.add_project(repo_path).await.unwrap();
    let session_id = manager
        .prepare_session(
            &project_id,
            "hibernate-test".to_string(),
            Some("bash".to_string()),
            None,
        )
        .await
        .unwrap();
    manager
        .finalize_session(&session_id, None, None)
        .await
        .unwrap();

    let (tmux_name, worktree_path) = {
        let state = store.read().await;
        let s = state.get_session(&session_id).unwrap();
        assert_eq!(s.status, SessionStatus::Running);
        assert!(!s.hibernated, "fresh session must not be marked hibernated");
        (s.tmux_session_name.clone(), s.worktree_path.clone())
    };
    assert!(
        manager.tmux.session_exists(&tmux_name).await.unwrap(),
        "tmux session should be live before hibernation"
    );

    // Hibernate: tmux process is stopped, but the worktree and metadata stay.
    manager.hibernate_session(&session_id).await.unwrap();
    {
        let state = store.read().await;
        let s = state.get_session(&session_id).unwrap();
        assert_eq!(s.status, SessionStatus::Stopped, "should be Stopped");
        assert!(s.hibernated, "should be marked hibernated");
    }
    assert!(
        !manager.tmux.session_exists(&tmux_name).await.unwrap(),
        "tmux session should be killed (memory freed) by hibernation"
    );
    assert!(
        worktree_path.exists(),
        "worktree must be preserved across hibernation (not a delete)"
    );

    // Wake via the attach path: recreates tmux, flips back to Running, and
    // clears the marker — even though resume_session is false.
    let attach_cmd = manager.get_attach_command(&session_id).await.unwrap();
    assert!(
        attach_cmd.contains(&tmux_name),
        "attach command should target the session's tmux name"
    );
    {
        let state = store.read().await;
        let s = state.get_session(&session_id).unwrap();
        assert_eq!(s.status, SessionStatus::Running, "should be Running again");
        assert!(!s.hibernated, "hibernation marker must be cleared on wake");
    }
    assert!(
        manager.tmux.session_exists(&tmux_name).await.unwrap(),
        "tmux session should be recreated on wake"
    );

    // Cleanup
    let _ = manager.kill_session(&session_id, true).await;
    drop(repo_temp_dir);
    drop(state_temp_dir);
    drop(worktrees_dir);
}
