//! Integration tests for claude-commander
//!
//! These tests require tmux to be installed and available.
//! All tests use isolated state files to avoid polluting user data.

use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;
use tokio::sync::RwLock;

use claude_commander::config::{AppState, Config};
use claude_commander::git::GitBackend;
use claude_commander::session::SessionManager;
use claude_commander::SessionStatus;

/// Helper to create an isolated AppState that won't pollute user data
fn create_isolated_state(temp_dir: &TempDir) -> AppState {
    let state_path = temp_dir.path().join("state.json");
    AppState::load_from(&state_path).unwrap()
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
    assert!(backend.is_ok(), "Should discover git repository from subdirectory");
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
    let state = Arc::new(RwLock::new(create_isolated_state(&state_temp_dir)));
    let manager = SessionManager::new(config, state.clone());

    // Add project
    let result = manager.add_project(repo_path.clone()).await;
    assert!(result.is_ok(), "Should add project: {:?}", result.err());

    let project_id = result.unwrap();

    // Verify project was added
    let state = state.read().await;
    assert!(state.get_project(&project_id).is_some());

    // Keep temp dirs alive until end of test
    drop(repo_temp_dir);
    drop(state_temp_dir);
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
    let mut config = Config::default();
    config.worktrees_dir = Some(worktrees_dir.path().to_path_buf());

    let state = Arc::new(RwLock::new(create_isolated_state(&state_temp_dir)));
    let manager = SessionManager::new(config, state.clone());

    // Add project
    let project_id = manager.add_project(repo_path).await.unwrap();

    // Create session
    let result = manager
        .create_session(&project_id, "test-session".to_string(), Some("bash".to_string()))
        .await;

    if let Err(e) = &result {
        eprintln!("Error creating session: {}", e);
    }

    assert!(result.is_ok(), "Should create session");

    let session_id = result.unwrap();

    // Verify session was created
    {
        let state = state.read().await;
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
async fn test_session_manager_pause_resume() {
    if !tmux_available().await {
        eprintln!("Skipping test: tmux not available");
        return;
    }

    let (repo_temp_dir, repo_path) = create_test_repo().await;
    let state_temp_dir = TempDir::new().unwrap();

    let worktrees_dir = TempDir::new().unwrap();
    let mut config = Config::default();
    config.worktrees_dir = Some(worktrees_dir.path().to_path_buf());

    let state = Arc::new(RwLock::new(create_isolated_state(&state_temp_dir)));
    let manager = SessionManager::new(config, state.clone());

    // Add project and create session
    let project_id = manager.add_project(repo_path).await.unwrap();
    let session_id = manager
        .create_session(&project_id, "pause-test".to_string(), Some("bash".to_string()))
        .await
        .unwrap();

    // Verify initial status
    {
        let state = state.read().await;
        let session = state.get_session(&session_id).unwrap();
        assert!(session.status.can_pause(), "Should be able to pause");
    }

    // Pause
    let result = manager.pause_session(&session_id).await;
    assert!(result.is_ok(), "Should pause session");

    // Verify paused status
    {
        let state = state.read().await;
        let session = state.get_session(&session_id).unwrap();
        assert!(session.status.can_resume(), "Should be able to resume");
    }

    // Resume
    let result = manager.resume_session(&session_id).await;
    assert!(result.is_ok(), "Should resume session");

    // Cleanup
    let _ = manager.kill_session(&session_id, true).await;

    // Keep temp dirs alive until end of test
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
        let project = claude_commander::Project::new(
            "test-project",
            PathBuf::from("/tmp/test"),
            "main",
        );
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
    let mut config = Config::default();
    config.worktrees_dir = Some(worktrees_dir.path().to_path_buf());

    let state = Arc::new(RwLock::new(create_isolated_state(&state_temp_dir)));
    let manager = SessionManager::new(config, state.clone());

    // Add project (no worktrees yet)
    let project_id = manager.add_project(repo_path.clone()).await.unwrap();

    // Verify no sessions were imported (no external worktrees exist)
    {
        let st = state.read().await;
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
        let st = state.read().await;
        let project = st.get_project(&project_id).unwrap();
        assert_eq!(project.worktrees.len(), 1, "Should have 1 session");

        let session = st.get_session(&project.worktrees[0]).unwrap();
        assert_eq!(session.branch, "external-feature");
        assert_eq!(session.status, SessionStatus::Paused);
        assert!(session.base_commit.is_some());
    }

    // Run sync again - should be idempotent
    let imported_again = manager.sync_worktrees(&project_id).await.unwrap();
    assert_eq!(imported_again, 0, "Second sync should import 0 (idempotent)");

    // Keep temp dirs alive
    drop(repo_temp_dir);
    drop(state_temp_dir);
    drop(worktrees_dir);
}
