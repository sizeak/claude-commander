//! Concurrent-safe configuration store with file change detection
//!
//! Mirrors the `StateStore` pattern: wraps `Config` in an `RwLock` for
//! thread-safe access and tracks the config file's modification time to
//! detect external changes (e.g. the user editing `config.toml` by hand).

use std::path::PathBuf;
use std::sync::RwLock;
use std::time::SystemTime;

use tracing::debug;

use super::Config;
use crate::error::Result;

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
pub struct ConfigStore {
    config: RwLock<Config>,
    config_path: PathBuf,
    last_mtime: RwLock<Option<SystemTime>>,
}

impl ConfigStore {
    /// Create a new ConfigStore from an already-loaded Config.
    pub fn new(config: Config) -> Result<Self> {
        let config_path = Config::config_file_path()?;
        let mtime = std::fs::metadata(&config_path)
            .and_then(|m| m.modified())
            .ok();
        Ok(Self {
            config: RwLock::new(config),
            config_path,
            last_mtime: RwLock::new(mtime),
        })
    }

    /// Create a ConfigStore with a custom path (for testing).
    pub fn with_path(config: Config, config_path: PathBuf) -> Self {
        let mtime = std::fs::metadata(&config_path)
            .and_then(|m| m.modified())
            .ok();
        Self {
            config: RwLock::new(config),
            config_path,
            last_mtime: RwLock::new(mtime),
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
    /// Updates the tracked mtime so that `reload_if_changed()` won't
    /// immediately re-read our own write.
    pub fn mutate<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut Config) -> R,
    {
        let result = {
            let mut config = self.config.write().expect("config lock poisoned");
            let result = f(&mut config);
            self.save_to_disk(&config)?;
            result
        };

        Ok(result)
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

        let toml = toml::to_string_pretty(config)
            .map_err(|e| ConfigError::SaveFailed(e.to_string()))?;
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

        Ok(true)
    }

    /// Load config from `self.config_path` using the standard layered resolution.
    fn load_from_disk(&self) -> Result<Config> {
        use crate::error::ConfigError;
        use figment::{Figment, providers::{Env, Format, Serialized, Toml}};

        let config: Config = Figment::new()
            .merge(Serialized::defaults(Config::default()))
            .merge(Toml::file(&self.config_path))
            .merge(Env::prefixed("CC_").split("_"))
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
        let toml =
            toml::to_string_pretty(config).expect("serialize config");
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
        let mut edited = Config::default();
        edited.default_program = "external-edit".to_string();
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

    #[test]
    fn test_read_returns_current_config() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");

        let mut config = Config::default();
        config.default_program = "test-program".to_string();
        write_config(&config_path, &config);

        let store = ConfigStore::with_path(config, config_path);
        assert_eq!(store.read().default_program, "test-program");
    }
}
