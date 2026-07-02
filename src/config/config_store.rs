//! Concurrent-safe configuration store with file change detection
//!
//! Mirrors the `StateStore` pattern: wraps `Config` in an `RwLock` for
//! thread-safe access and tracks the config file's modification time to
//! detect external changes (e.g. the user editing `config.toml` by hand).

use std::path::PathBuf;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use tracing::debug;

use super::Config;
use crate::error::Result;

/// Snapshot of config values that are baked into subsystems at init time
/// and cannot be hot-reloaded. Used to detect when a restart is needed.
#[derive(Debug, Clone, PartialEq)]
struct InitSnapshot {
    max_concurrent_tmux: usize,
    capture_cache_ttl_ms: u64,
    diff_cache_ttl_ms: u64,
    ui_refresh_fps: u32,
    state_sync_interval_ms: u64,
    commander_enabled: bool,
    // The web UI server is bound once at startup, so every web_ui_* setting
    // needs a restart to take effect — track them all for restart_required.
    web_ui_enabled: bool,
    web_ui_port: u16,
    web_ui_auth: crate::config::WebUiAuth,
    web_ui_password: Option<String>,
    web_ui_tls_cert: Option<std::path::PathBuf>,
    web_ui_tls_key: Option<std::path::PathBuf>,
    web_ui_tls_client_ca: Option<std::path::PathBuf>,
}

impl InitSnapshot {
    fn capture(config: &Config) -> Self {
        Self {
            max_concurrent_tmux: config.max_concurrent_tmux,
            capture_cache_ttl_ms: config.capture_cache_ttl_ms,
            diff_cache_ttl_ms: config.diff_cache_ttl_ms,
            ui_refresh_fps: config.ui_refresh_fps,
            state_sync_interval_ms: config.state_sync_interval_ms,
            commander_enabled: config.commander_enabled,
            web_ui_enabled: config.web_ui_enabled,
            web_ui_port: config.web_ui_port,
            web_ui_auth: config.web_ui_auth,
            web_ui_password: config.web_ui_password.clone(),
            web_ui_tls_cert: config.web_ui_tls_cert.clone(),
            web_ui_tls_key: config.web_ui_tls_key.clone(),
            web_ui_tls_client_ca: config.web_ui_tls_client_ca.clone(),
        }
    }

    fn matches(&self, config: &Config) -> bool {
        self.max_concurrent_tmux == config.max_concurrent_tmux
            && self.capture_cache_ttl_ms == config.capture_cache_ttl_ms
            && self.diff_cache_ttl_ms == config.diff_cache_ttl_ms
            && self.ui_refresh_fps == config.ui_refresh_fps
            && self.state_sync_interval_ms == config.state_sync_interval_ms
            && self.commander_enabled == config.commander_enabled
            && self.web_ui_enabled == config.web_ui_enabled
            && self.web_ui_port == config.web_ui_port
            && self.web_ui_auth == config.web_ui_auth
            && self.web_ui_password == config.web_ui_password
            && self.web_ui_tls_cert == config.web_ui_tls_cert
            && self.web_ui_tls_key == config.web_ui_tls_key
            && self.web_ui_tls_client_ca == config.web_ui_tls_client_ca
    }
}

/// Concurrent-safe configuration store with mtime-based hot-reload.
///
/// # Hot-reload semantics
///
/// Values read at runtime (keybindings, dim settings, editor, theme, etc.)
/// pick up changes automatically after [`reload_if_changed`](Self::reload_if_changed)
/// detects a new mtime on the config file.
///
/// Values baked into subsystem constructors at init time require a restart:
/// - `max_concurrent_tmux` (TmuxExecutor semaphore size)
/// - `capture_cache_ttl_ms` / `diff_cache_ttl_ms` (cache durations)
/// - `ui_refresh_fps` (event loop tick rate)
/// - `state_sync_interval_ms` (state sync background task interval)
/// - `commander_enabled` (captured by the agent-state poll task at spawn)
///
/// Call [`restart_required`](Self::restart_required) to check whether any of
/// those init-time values have diverged from the running config. The flag
/// reverts to `false` if the values are changed back to match.
pub struct ConfigStore {
    config: RwLock<Config>,
    config_path: PathBuf,
    last_mtime: RwLock<Option<SystemTime>>,
    /// Snapshot of restart-required fields captured at construction time.
    init_snapshot: InitSnapshot,
    /// Bumped on every in-memory config change (a local [`mutate`](Self::mutate)
    /// or a disk [`reload_if_changed`](Self::reload_if_changed)). In-process
    /// readers that cache a `Config` copy (notably the TUI's `App`) compare this
    /// to detect a stale snapshot even when the change came from another
    /// subsystem sharing this store — e.g. the web UI writing config in the same
    /// process. The mtime guard alone can't catch that, since our own write
    /// updates the tracked mtime and suppresses the reload.
    generation: AtomicU64,
}

impl ConfigStore {
    /// Create a new ConfigStore from an already-loaded Config.
    pub fn new(config: Config) -> Result<Self> {
        let config_path = Config::config_file_path()?;
        let mtime = std::fs::metadata(&config_path)
            .and_then(|m| m.modified())
            .ok();
        let init_snapshot = InitSnapshot::capture(&config);
        Ok(Self {
            config: RwLock::new(config),
            config_path,
            last_mtime: RwLock::new(mtime),
            init_snapshot,
            generation: AtomicU64::new(0),
        })
    }

    /// Create a ConfigStore with a custom path (for testing).
    pub fn with_path(config: Config, config_path: PathBuf) -> Self {
        let mtime = std::fs::metadata(&config_path)
            .and_then(|m| m.modified())
            .ok();
        let init_snapshot = InitSnapshot::capture(&config);
        Self {
            config: RwLock::new(config),
            config_path,
            last_mtime: RwLock::new(mtime),
            init_snapshot,
            generation: AtomicU64::new(0),
        }
    }

    /// Get a read guard on the current config.
    ///
    /// This is fast (no disk I/O) and safe to call on every render frame.
    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, Config> {
        self.config.read().expect("config lock poisoned")
    }

    /// Apply a mutation to the config, then persist to disk.
    ///
    /// Before applying `f`, this re-reads the file if it changed on disk since we
    /// last touched it, so an edit made elsewhere (another instance, the web UI,
    /// a hand-edit) is folded in rather than clobbered by our stale in-memory
    /// copy. Without this, a long-running instance would overwrite external
    /// changes every time it persisted. The reload + mutate + write all happen
    /// under the same write lock so the cycle is atomic within this process.
    ///
    /// Updates the tracked mtime so that `reload_if_changed()` won't
    /// immediately re-read our own write.
    pub fn mutate<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut Config) -> R,
    {
        let result = {
            let mut config = self.config.write().expect("config lock poisoned");

            // Fold in any external change before mutating, so we build on the
            // current on-disk state instead of overwriting it.
            let current_mtime = std::fs::metadata(&self.config_path)
                .and_then(|m| m.modified())
                .ok();
            let last = *self.last_mtime.read().expect("mtime lock poisoned");
            if current_mtime != last
                && let Ok(disk_config) = self.load_from_disk()
            {
                *config = disk_config;
            }

            let result = f(&mut config);
            self.save_to_disk(&config)?;
            // Signal in-process cache holders that the config changed, even
            // though save_to_disk just moved the mtime past their reload guard.
            self.generation.fetch_add(1, Ordering::Relaxed);
            result
        };

        Ok(result)
    }

    /// Monotonic counter bumped on every in-memory config change. Callers that
    /// cache a `Config` snapshot compare this against the value they last synced
    /// at to know when their copy is stale (see the field docs on `generation`).
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// Write the given config to `self.config_path` and update the tracked mtime.
    fn save_to_disk(&self, config: &Config) -> Result<()> {
        use crate::error::ConfigError;

        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                crate::error::Error::Config(ConfigError::SaveFailed(format!(
                    "Failed to create config directory: {}",
                    e
                )))
            })?;
        }

        let toml =
            toml::to_string_pretty(config).map_err(|e| ConfigError::SaveFailed(e.to_string()))?;
        std::fs::write(&self.config_path, toml)
            .map_err(|e| ConfigError::SaveFailed(e.to_string()))?;

        let mtime = std::fs::metadata(&self.config_path)
            .and_then(|m| m.modified())
            .ok();
        *self.last_mtime.write().expect("mtime lock poisoned") = mtime;

        Ok(())
    }

    /// Check if the config file has been modified externally and reload if so.
    ///
    /// Returns `true` if the in-memory config was updated.
    pub fn reload_if_changed(&self) -> Result<bool> {
        let current_mtime = std::fs::metadata(&self.config_path)
            .and_then(|m| m.modified())
            .ok();

        let last = *self.last_mtime.read().expect("mtime lock poisoned");

        if current_mtime == last {
            return Ok(false);
        }

        debug!("Config file mtime changed, reloading");

        let new_config = self.load_from_disk()?;
        *self.config.write().expect("config lock poisoned") = new_config;
        *self.last_mtime.write().expect("mtime lock poisoned") = current_mtime;
        self.generation.fetch_add(1, Ordering::Relaxed);

        Ok(true)
    }

    /// Check whether any restart-required config values have diverged from
    /// the values that were active when the application started.
    ///
    /// Returns `false` if the values have been changed back to match, so the
    /// indicator self-heals without a restart.
    pub fn restart_required(&self) -> bool {
        let config = self.config.read().expect("config lock poisoned");
        !self.init_snapshot.matches(&config)
    }

    /// Load config from `self.config_path` using the standard layered resolution.
    fn load_from_disk(&self) -> Result<Config> {
        use crate::error::ConfigError;
        use figment::{
            Figment,
            providers::{Format, Serialized, Toml},
        };

        let config: Config = Figment::new()
            .merge(Serialized::defaults(Config::default()))
            .merge(Toml::file(&self.config_path))
            .extract()
            .map_err(|e| ConfigError::LoadFailed(e.to_string()))?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_config(path: &std::path::Path, config: &Config) {
        let toml = toml::to_string_pretty(config).expect("serialize config");
        std::fs::write(path, toml).expect("write config file");
    }

    #[test]
    fn test_reload_if_changed_detects_external_edit() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");

        let config = Config::default();
        write_config(&config_path, &config);

        let store = ConfigStore::with_path(config, config_path.clone());

        // No change yet
        assert!(!store.reload_if_changed().unwrap());

        // Simulate external edit — change a field and rewrite the file
        // Sleep briefly so mtime differs (filesystem granularity)
        std::thread::sleep(std::time::Duration::from_millis(50));
        let edited = Config {
            default_program: "external-edit".to_string(),
            ..Config::default()
        };
        write_config(&config_path, &edited);

        // Should detect the change
        assert!(store.reload_if_changed().unwrap());
        assert_eq!(store.read().default_program, "external-edit");

        // Second call should not reload again
        assert!(!store.reload_if_changed().unwrap());
    }

    #[test]
    fn test_mutate_persists_and_updates_mtime() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");

        let config = Config::default();
        write_config(&config_path, &config);

        let store = ConfigStore::with_path(config, config_path.clone());

        store
            .mutate(|c| {
                c.default_program = "mutated".to_string();
            })
            .unwrap();

        // In-memory value updated
        assert_eq!(store.read().default_program, "mutated");

        // On-disk value updated
        let disk_content = std::fs::read_to_string(&config_path).unwrap();
        assert!(disk_content.contains("mutated"));

        // No spurious reload after our own write
        assert!(!store.reload_if_changed().unwrap());
    }

    /// Read the config straight from a file via the same layered resolution the
    /// store uses, without going through a ConfigStore (whose constructor would
    /// capture the current mtime and make reload_if_changed a no-op).
    fn read_config_file(path: &std::path::Path) -> Config {
        use figment::{
            Figment,
            providers::{Format, Serialized, Toml},
        };
        Figment::new()
            .merge(Serialized::defaults(Config::default()))
            .merge(Toml::file(path))
            .extract()
            .expect("load config from disk")
    }

    #[test]
    fn test_mutate_folds_in_external_edit_instead_of_clobbering() {
        // Regression: a long-running instance must not overwrite a setting that
        // another instance / the web UI / a hand-edit wrote to disk. mutate()
        // reloads the changed file before applying its own change, so both
        // survive.
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");

        let config = Config::default();
        write_config(&config_path, &config);
        let store = ConfigStore::with_path(config, config_path.clone());

        // This store first persists something so its mtime is in sync with disk.
        store
            .mutate(|c| c.branch_prefix = "mine".to_string())
            .unwrap();

        // Meanwhile, an external writer changes a DIFFERENT field on disk.
        // Sleep so the mtime is guaranteed to differ (filesystem granularity).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let external = Config {
            branch_prefix: "mine".to_string(),
            default_program: "set-by-other-instance".to_string(),
            ..Config::default()
        };
        write_config(&config_path, &external);

        // Our next mutate touches yet another field. It must fold in the external
        // edit (reload-before-mutate), not wipe it.
        store.mutate(|c| c.web_ui_port = 9999).unwrap();

        let disk = read_config_file(&config_path);
        assert_eq!(
            disk.default_program, "set-by-other-instance",
            "external edit must be preserved, not clobbered"
        );
        assert_eq!(disk.web_ui_port, 9999, "our own edit must be applied");
        assert_eq!(disk.branch_prefix, "mine", "earlier edit preserved");
    }

    #[test]
    fn test_same_process_write_hidden_from_mtime_but_caught_by_generation() {
        // Regression: the TUI caches a Config snapshot and used to resync only
        // when the *mtime* guard (`reload_if_changed`) reported a change. But a
        // web-UI write goes through this same shared store, and its own
        // `save_to_disk` advances the tracked mtime — so the mtime guard saw
        // "no change" and the TUI never refreshed. Its next settings-save then
        // clobbered the web write (reverting web_ui_enabled / dropping the
        // password). The generation counter must advance on the write so a cache
        // holder can detect it even though the mtime guard hides it.
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        let config = Config::default();
        write_config(&config_path, &config);
        let store = ConfigStore::with_path(config, config_path);

        let cached_gen = store.generation();

        // Simulate the web UI enabling the server + setting a password.
        store
            .mutate(|c| {
                c.web_ui_enabled = true;
                c.web_ui_password = Some("secret".to_string());
            })
            .unwrap();

        // The mtime guard hides our own write, so a cache holder relying on it
        // alone would NOT resync...
        assert!(
            !store.reload_if_changed().unwrap(),
            "our own write must not trigger a disk reload"
        );
        // ...but the generation advanced, which is how the cache holder knows.
        assert_ne!(
            store.generation(),
            cached_gen,
            "mutate must advance the generation counter"
        );
        assert!(store.read().web_ui_enabled);
        assert_eq!(store.read().web_ui_password.as_deref(), Some("secret"));
    }

    #[test]
    fn test_generation_advances_on_external_reload() {
        // An external edit reloaded from disk must also advance the generation
        // so cache holders resync to the hand-edited values.
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        let config = Config::default();
        write_config(&config_path, &config);
        let store = ConfigStore::with_path(config, config_path.clone());

        let before = store.generation();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let edited = Config {
            default_program: "edited".to_string(),
            ..Config::default()
        };
        write_config(&config_path, &edited);

        assert!(store.reload_if_changed().unwrap());
        assert_ne!(
            store.generation(),
            before,
            "external reload must advance the generation counter"
        );
    }

    #[test]
    fn test_restart_required_false_when_unchanged() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");

        let config = Config::default();
        write_config(&config_path, &config);

        let store = ConfigStore::with_path(config, config_path);
        assert!(!store.restart_required());
    }

    #[test]
    fn test_restart_required_true_when_init_field_changes() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");

        let config = Config::default();
        write_config(&config_path, &config);

        let store = ConfigStore::with_path(config, config_path);

        // Change a restart-required field
        store
            .mutate(|c| {
                c.ui_refresh_fps = 60;
            })
            .unwrap();

        assert!(store.restart_required());
    }

    #[test]
    fn test_restart_required_true_when_commander_enabled_changes() {
        // The agent-state poll task captures `commander_enabled` at spawn, so
        // toggling it at runtime must surface the restart-required warning
        // (otherwise the chip/row would silently never update).
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");

        let config = Config::default();
        write_config(&config_path, &config);

        let store = ConfigStore::with_path(config, config_path);

        store
            .mutate(|c| {
                c.commander_enabled = true;
            })
            .unwrap();

        assert!(store.restart_required());
    }

    #[test]
    fn test_restart_required_reverts_when_changed_back() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");

        let config = Config::default();
        let original_fps = config.ui_refresh_fps;
        write_config(&config_path, &config);

        let store = ConfigStore::with_path(config, config_path);

        // Change it
        store
            .mutate(|c| {
                c.ui_refresh_fps = 60;
            })
            .unwrap();
        assert!(store.restart_required());

        // Change it back
        store
            .mutate(|c| {
                c.ui_refresh_fps = original_fps;
            })
            .unwrap();
        assert!(!store.restart_required());
    }

    #[test]
    fn test_restart_required_ignores_hot_reloadable_fields() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");

        let config = Config::default();
        write_config(&config_path, &config);

        let store = ConfigStore::with_path(config, config_path);

        // Change only hot-reloadable fields — should NOT require restart
        store
            .mutate(|c| {
                c.default_program = "different".to_string();
                c.dim_unfocused_preview = false;
                c.leader_key = "f1".to_string();
            })
            .unwrap();

        assert!(!store.restart_required());
    }

    #[test]
    fn test_read_returns_current_config() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");

        let config = Config {
            default_program: "test-program".to_string(),
            ..Config::default()
        };
        write_config(&config_path, &config);

        let store = ConfigStore::with_path(config, config_path);
        assert_eq!(store.read().default_program, "test-program");
    }
}
