//! Concurrent-safe state store with file locking
//!
//! Provides `StateStore`, which wraps `AppState` with:
//! - Advisory file locking (`flock`) for cross-process safety
//! - Atomic writes (write-to-temp + rename) for crash safety
//! - An in-memory cache for fast reads on the render path
//! - Periodic reload to pick up changes from other instances

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use nix::fcntl::{Flock, FlockArg};
use tokio::sync::{RwLock, watch};
use tracing::debug;

use super::Config;
use super::storage::AppState;
use crate::error::{ConfigError, Result};

/// Concurrent-safe state persistence backed by a JSON file with advisory locking.
///
/// Multiple instances of the application can safely share the same state file.
/// Writes use a read-modify-write cycle under an exclusive file lock; reads
/// come from an in-memory cache that is periodically refreshed.
pub struct StateStore {
    /// Path to the state JSON file
    state_path: PathBuf,
    /// Path to the lock file (separate from state file because atomic rename changes inodes)
    lock_path: PathBuf,
    /// In-memory cached state for fast reads
    state: Arc<RwLock<AppState>>,
    /// Modification time of the state file at last read/write (for change detection)
    last_mtime: Arc<RwLock<Option<SystemTime>>>,
    /// Monotonic change counter, bumped after every successful mutation (and on
    /// a reload that picked up an external change). Consumers subscribe via
    /// [`Self::subscribe`] and re-read state on each bump — this is the local
    /// backend's change-feed, letting the TUI refresh on mutation rather than
    /// only on a fixed tick.
    generation: Arc<watch::Sender<u64>>,
}

impl StateStore {
    /// Create a new StateStore from an already-loaded AppState.
    pub fn new(app_state: AppState) -> Result<Self> {
        let state_path = Config::state_file_path()?;
        let lock_path = state_path.with_extension("json.lock");

        // Ensure parent directory exists
        if let Some(parent) = state_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ConfigError::SaveFailed(format!("Failed to create state directory: {}", e))
            })?;
        }

        let mtime = std::fs::metadata(&state_path)
            .and_then(|m| m.modified())
            .ok();

        Ok(Self {
            state_path,
            lock_path,
            state: Arc::new(RwLock::new(app_state)),
            last_mtime: Arc::new(RwLock::new(mtime)),
            generation: Arc::new(watch::Sender::new(0)),
        })
    }

    /// Create a StateStore with a custom path (for testing).
    pub fn with_path(app_state: AppState, state_path: PathBuf) -> Self {
        let lock_path = state_path.with_extension("json.lock");
        Self {
            state_path,
            lock_path,
            state: Arc::new(RwLock::new(app_state)),
            last_mtime: Arc::new(RwLock::new(None)),
            generation: Arc::new(watch::Sender::new(0)),
        }
    }

    /// Subscribe to the change-feed. The returned receiver's value is the
    /// current generation counter; it changes (via [`watch::Receiver::changed`])
    /// after every successful mutation and after a reload that picked up an
    /// external change. This is the local backend's change notification — a
    /// consumer re-reads [`Self::read`] on each bump rather than polling on a
    /// fixed tick. See [`crate::backend`].
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.generation.subscribe()
    }

    /// The current change generation. Bumped on every persisted mutation.
    pub fn generation(&self) -> u64 {
        *self.generation.borrow()
    }

    /// Bump the change generation, waking every [`Self::subscribe`] receiver.
    fn bump_generation(&self) {
        self.generation.send_modify(|g| *g += 1);
    }

    /// The data directory the state file lives in — the parent of
    /// `state_path`. Sibling stores (comments, reviewed marks) are rooted here
    /// so they share the same data dir as the state, which keeps tests that
    /// inject a `TempDir`-backed store off the real filesystem. Falls back to
    /// `.` only if `state_path` has no parent (never on real paths).
    pub fn data_dir(&self) -> PathBuf {
        self.state_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Get a read lock on the in-memory state.
    ///
    /// This is fast (no disk I/O) and safe to call on every render frame.
    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, AppState> {
        self.state.read().await
    }

    /// Snapshot the persisted install id without awaiting, if the in-memory
    /// state is immediately readable. Used during (uncontended) startup to seed
    /// telemetry; returns `None` if absent or momentarily locked.
    pub fn try_install_id(&self) -> Option<String> {
        self.state
            .try_read()
            .ok()
            .and_then(|s| s.install_id.clone())
    }

    /// Snapshot the persisted session-list view mode without awaiting, if the
    /// in-memory state is immediately readable. Used at (uncontended) startup to
    /// seed telemetry; `None` if unset or momentarily locked.
    pub fn try_view_mode(&self) -> Option<crate::config::ViewMode> {
        self.state.try_read().ok().and_then(|s| s.view_mode)
    }

    /// Apply a mutation to the persisted state.
    ///
    /// Acquires an exclusive file lock, re-reads the current state from disk
    /// (to pick up changes from other instances), applies the closure, writes
    /// back atomically, and updates the in-memory cache.
    pub async fn mutate<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut AppState) -> R + Send + 'static,
        R: Send + 'static,
    {
        let state_path = self.state_path.clone();
        let lock_path = self.lock_path.clone();

        // Run the entire lock-read-modify-write cycle on a blocking thread
        // because flock() is a blocking syscall
        let (result, new_state, mtime) = tokio::task::spawn_blocking(move || {
            let lock_file = open_lock_file(&lock_path)?;
            let _lock = Flock::lock(lock_file, FlockArg::LockExclusive).map_err(|(_, e)| {
                crate::error::Error::Config(ConfigError::SaveFailed(format!(
                    "Failed to acquire file lock: {}",
                    e
                )))
            })?;

            let mut disk_state = read_state_from_disk(&state_path)?;
            let result = f(&mut disk_state);
            disk_state.version = env!("CARGO_PKG_VERSION").to_string();
            atomic_write(&state_path, &disk_state)?;

            let mtime = std::fs::metadata(&state_path)
                .and_then(|m| m.modified())
                .ok();

            // _lock dropped here → flock released
            Ok::<_, crate::error::Error>((result, disk_state, mtime))
        })
        .await
        .map_err(|e| ConfigError::SaveFailed(format!("Blocking task panicked: {}", e)))??;

        // Update in-memory cache
        *self.state.write().await = new_state;
        *self.last_mtime.write().await = mtime;
        self.bump_generation();

        Ok(result)
    }

    /// Apply a fallible mutation to the persisted state.
    ///
    /// Like `mutate()`, but the closure returns a `Result`. If the closure fails,
    /// the state is **not** written back to disk.
    pub async fn try_mutate<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut AppState) -> std::result::Result<R, crate::error::Error> + Send + 'static,
        R: Send + 'static,
    {
        let state_path = self.state_path.clone();
        let lock_path = self.lock_path.clone();

        let (result, new_state, mtime) =
            tokio::task::spawn_blocking(move || -> std::result::Result<_, crate::error::Error> {
                let lock_file = open_lock_file(&lock_path)?;
                let _lock = Flock::lock(lock_file, FlockArg::LockExclusive).map_err(|(_, e)| {
                    crate::error::Error::Config(ConfigError::SaveFailed(format!(
                        "Failed to acquire file lock: {}",
                        e
                    )))
                })?;

                let mut disk_state = read_state_from_disk(&state_path)?;

                // Apply the closure — if it fails, bail without writing
                let result = f(&mut disk_state)?;

                disk_state.version = env!("CARGO_PKG_VERSION").to_string();
                atomic_write(&state_path, &disk_state)?;

                let mtime = std::fs::metadata(&state_path)
                    .and_then(|m| m.modified())
                    .ok();

                Ok((result, disk_state, mtime))
            })
            .await
            .map_err(|e| ConfigError::SaveFailed(format!("Blocking task panicked: {}", e)))??;

        *self.state.write().await = new_state;
        *self.last_mtime.write().await = mtime;
        self.bump_generation();

        Ok(result)
    }

    /// Check if the on-disk state has changed and reload if so.
    ///
    /// Returns `true` if the in-memory state was updated.
    pub async fn reload_if_changed(&self) -> Result<bool> {
        let current_mtime = tokio::fs::metadata(&self.state_path)
            .await
            .and_then(|m| m.modified())
            .ok();

        let last = *self.last_mtime.read().await;

        if current_mtime == last {
            return Ok(false);
        }

        debug!("State file mtime changed, reloading");

        let state_path = self.state_path.clone();
        let lock_path = self.lock_path.clone();

        let (new_state, mtime) = tokio::task::spawn_blocking(move || {
            let lock_file = open_lock_file(&lock_path)?;
            let _lock = Flock::lock(lock_file, FlockArg::LockShared).map_err(|(_, e)| {
                crate::error::Error::Config(ConfigError::LoadFailed(format!(
                    "Failed to acquire shared lock: {}",
                    e
                )))
            })?;

            let state = read_state_from_disk(&state_path)?;

            let mtime = std::fs::metadata(&state_path)
                .and_then(|m| m.modified())
                .ok();

            Ok::<_, crate::error::Error>((state, mtime))
        })
        .await
        .map_err(|e| ConfigError::LoadFailed(format!("Blocking task panicked: {}", e)))??;

        *self.state.write().await = new_state;
        *self.last_mtime.write().await = mtime;
        // An external change was picked up — wake change-feed subscribers so the
        // TUI re-renders on another instance's mutation, not just our own.
        self.bump_generation();

        Ok(true)
    }
}

/// Open (or create) the lock file.
fn open_lock_file(lock_path: &PathBuf) -> Result<File> {
    File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .map_err(|e| ConfigError::SaveFailed(format!("Failed to open lock file: {}", e)).into())
}

/// Read state from disk, returning a fresh default if the file doesn't exist.
///
/// Applies the same migrations as [`AppState::load_from`] — notably
/// [`AppState::backfill_base_branch`] — because every runtime read-modify-write
/// (`mutate`/`try_mutate`/`reload_if_changed`) flows through here and overwrites
/// the in-memory cache with the result. Without the backfill, the first mutation
/// after startup would strip the derived `base_branch` (which isn't persisted on
/// older records), reverting the review diff to the stale frozen `base_commit`.
fn read_state_from_disk(state_path: &PathBuf) -> Result<AppState> {
    if !state_path.exists() {
        return Ok(AppState::new());
    }

    let content = std::fs::read_to_string(state_path)
        .map_err(|e| ConfigError::LoadFailed(format!("Failed to read state file: {}", e)))?;

    let mut state: AppState = serde_json::from_str(&content)
        .map_err(|e| ConfigError::LoadFailed(format!("Failed to parse state file: {}", e)))?;

    state.backfill_base_branch();

    Ok(state)
}

/// Atomically write state to a file (write to temp + fsync + rename).
fn atomic_write(path: &PathBuf, state: &AppState) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ConfigError::SaveFailed(format!("Failed to create state directory: {}", e))
        })?;
    }

    let content = serde_json::to_string_pretty(state)
        .map_err(|e| ConfigError::SaveFailed(format!("Failed to serialize state: {}", e)))?;

    let tmp_path = path.with_extension(format!("json.tmp.{}", std::process::id()));

    {
        let mut tmp_file = File::create(&tmp_path)
            .map_err(|e| ConfigError::SaveFailed(format!("Failed to create temp file: {}", e)))?;
        tmp_file
            .write_all(content.as_bytes())
            .map_err(|e| ConfigError::SaveFailed(format!("Failed to write temp file: {}", e)))?;
        tmp_file
            .sync_all()
            .map_err(|e| ConfigError::SaveFailed(format!("Failed to fsync temp file: {}", e)))?;
    }

    std::fs::rename(&tmp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        ConfigError::SaveFailed(format!("Failed to rename temp file: {}", e))
    })?;

    Ok(())
}

impl Clone for StateStore {
    fn clone(&self) -> Self {
        Self {
            state_path: self.state_path.clone(),
            lock_path: self.lock_path.clone(),
            state: self.state.clone(),
            last_mtime: self.last_mtime.clone(),
            generation: self.generation.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Project;
    use tempfile::TempDir;

    fn create_test_store(temp_dir: &TempDir) -> StateStore {
        let state_path = temp_dir.path().join("state.json");
        StateStore::with_path(AppState::new(), state_path)
    }

    #[tokio::test]
    async fn test_mutate_persists_to_disk() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_test_store(&temp_dir);

        let project = Project::new("test", PathBuf::from("/tmp/test"), "main");
        let project_id = project.id;

        store
            .mutate(move |state| {
                state.add_project(project);
            })
            .await
            .unwrap();

        // Verify in-memory state
        {
            let state = store.read().await;
            assert_eq!(state.project_count(), 1);
        }

        // Verify on-disk state by loading fresh
        let state_path = temp_dir.path().join("state.json");
        let disk_state = AppState::load_from(&state_path).unwrap();
        assert_eq!(disk_state.project_count(), 1);
        assert!(disk_state.get_project(&project_id).is_some());
    }

    #[tokio::test]
    async fn mutate_preserves_backfilled_base_branch() {
        // A session persisted before `base_branch` existed carries only a frozen
        // `base_commit`. The startup backfill derives `base_branch`, but every
        // mutate()/reload re-reads disk and overwrites the in-memory cache — so
        // unless the read path also backfills, the first mutation strips the
        // derived branch and the review diff reverts to the stale frozen SHA.
        use crate::session::WorktreeSession;

        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state.json");

        // Seed disk with an "old" record: base_commit set, base_branch absent.
        let mut seed = AppState::new();
        let project = Project::new("p", PathBuf::from("/tmp/p"), "main");
        let project_id = project.id;
        seed.add_project(project);
        let mut session = WorktreeSession::new(
            project_id,
            "S",
            "feature",
            PathBuf::from("/tmp/p/wt"),
            "claude",
        );
        session.base_commit = Some("deadbeef".to_string());
        session.base_branch = None;
        let session_id = session.id;
        seed.add_session(session);
        std::fs::write(&state_path, serde_json::to_string(&seed).unwrap()).unwrap();

        // A no-op mutation must not strip the derived base_branch.
        let store = StateStore::with_path(AppState::new(), state_path.clone());
        store.mutate(|_| {}).await.unwrap();

        let state = store.read().await;
        assert_eq!(
            state
                .get_session(&session_id)
                .unwrap()
                .base_branch
                .as_deref(),
            Some("main"),
            "mutate() must re-apply the base_branch backfill, not revert to the on-disk null"
        );
    }

    #[tokio::test]
    async fn test_concurrent_mutates_no_lost_updates() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state.json");

        // Two stores pointing at the same file (simulating two instances)
        let store_a = StateStore::with_path(AppState::new(), state_path.clone());
        let store_b = StateStore::with_path(AppState::new(), state_path.clone());

        // Instance A adds a project
        let project_a = Project::new("project-a", PathBuf::from("/tmp/a"), "main");
        let id_a = project_a.id;
        store_a
            .mutate(move |state| {
                state.add_project(project_a);
            })
            .await
            .unwrap();

        // Instance B adds a different project (reads A's state from disk first)
        let project_b = Project::new("project-b", PathBuf::from("/tmp/b"), "main");
        let id_b = project_b.id;
        store_b
            .mutate(move |state| {
                state.add_project(project_b);
            })
            .await
            .unwrap();

        // Verify both projects exist on disk
        let disk_state = AppState::load_from(&state_path).unwrap();
        assert_eq!(disk_state.project_count(), 2);
        assert!(disk_state.get_project(&id_a).is_some());
        assert!(disk_state.get_project(&id_b).is_some());
    }

    #[tokio::test]
    async fn test_reload_if_changed() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state.json");

        let store = StateStore::with_path(AppState::new(), state_path.clone());

        // Write initial state
        store.mutate(|_| {}).await.unwrap();

        // External modification (simulating another instance)
        let mut external_state = AppState::load_from(&state_path).unwrap();
        let project = Project::new("external", PathBuf::from("/tmp/ext"), "main");
        external_state.add_project(project);
        atomic_write(&state_path, &external_state).unwrap();

        // Trigger reload
        let changed = store.reload_if_changed().await.unwrap();
        assert!(changed);

        // Verify the store picked up the external change
        let state = store.read().await;
        assert_eq!(state.project_count(), 1);
    }

    #[tokio::test]
    async fn mutate_bumps_generation_and_wakes_subscribers() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_test_store(&temp_dir);

        let mut rx = store.subscribe();
        assert_eq!(store.generation(), 0);
        assert_eq!(*rx.borrow_and_update(), 0);

        store.mutate(|_| {}).await.unwrap();

        // The counter advanced and the subscriber observes the change.
        assert_eq!(store.generation(), 1);
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow_and_update(), 1);

        // A second mutation bumps again.
        store.mutate(|_| {}).await.unwrap();
        assert_eq!(store.generation(), 2);
    }

    #[tokio::test]
    async fn reload_if_changed_bumps_only_on_external_change() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state.json");
        let store = StateStore::with_path(AppState::new(), state_path.clone());

        // Seed the file (bumps to 1) and record the mtime.
        store.mutate(|_| {}).await.unwrap();
        let gen_after_seed = store.generation();

        // No external change → no reload, no bump.
        assert!(!store.reload_if_changed().await.unwrap());
        assert_eq!(store.generation(), gen_after_seed);

        // External modification → reload picks it up and bumps.
        let mut external = AppState::load_from(&state_path).unwrap();
        external.add_project(Project::new("external", PathBuf::from("/tmp/ext"), "main"));
        atomic_write(&state_path, &external).unwrap();
        assert!(store.reload_if_changed().await.unwrap());
        assert_eq!(store.generation(), gen_after_seed + 1);
    }

    #[tokio::test]
    async fn cloned_store_shares_change_feed() {
        let temp_dir = TempDir::new().unwrap();
        let store = create_test_store(&temp_dir);
        let clone = store.clone();
        let mut rx = clone.subscribe();

        // A mutation through the original wakes a subscriber taken from the clone.
        store.mutate(|_| {}).await.unwrap();
        rx.changed().await.unwrap();
        assert_eq!(clone.generation(), store.generation());
    }

    #[tokio::test]
    async fn test_atomic_write_crash_safety() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state.json");

        // Write initial valid state
        let mut state = AppState::new();
        let project = Project::new("original", PathBuf::from("/tmp/orig"), "main");
        state.add_project(project);
        atomic_write(&state_path, &state).unwrap();

        // Verify it's valid JSON
        let loaded = AppState::load_from(&state_path).unwrap();
        assert_eq!(loaded.project_count(), 1);
    }
}
