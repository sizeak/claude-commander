//! User configuration settings
//!
//! Layered configuration: environment variables → config file → CLI args

use std::path::PathBuf;

use directories::ProjectDirs;
use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};

use crate::error::{ConfigError, Error, Result};

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Default program to run in new sessions
    pub default_program: String,

    /// Branch name prefix for new sessions (empty string means no prefix)
    pub branch_prefix: String,

    /// Maximum concurrent tmux commands
    pub max_concurrent_tmux: usize,

    /// Content capture cache TTL in milliseconds
    pub capture_cache_ttl_ms: u64,

    /// Diff cache TTL in milliseconds
    pub diff_cache_ttl_ms: u64,

    /// UI refresh rate in FPS
    pub ui_refresh_fps: u32,

    /// Path to worktrees directory
    pub worktrees_dir: Option<PathBuf>,

    /// Enable debug logging
    pub debug: bool,

    /// Log file path (if set, logs to file instead of stderr)
    pub log_file: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_program: "claude".to_string(),
            branch_prefix: String::new(),
            max_concurrent_tmux: 16,
            capture_cache_ttl_ms: 50,
            diff_cache_ttl_ms: 500,
            ui_refresh_fps: 30,
            worktrees_dir: None,
            debug: false,
            log_file: None,
        }
    }
}

impl Config {
    /// Load configuration from all sources
    pub fn load() -> Result<Self> {
        let config_path = Self::config_file_path()?;

        let config: Config = Figment::new()
            // Start with defaults
            .merge(Serialized::defaults(Config::default()))
            // Layer config file if it exists
            .merge(Toml::file(&config_path))
            // Layer environment variables (CC_DEFAULT_PROGRAM, etc.)
            .merge(Env::prefixed("CC_").split("_"))
            .extract()
            .map_err(|e| ConfigError::LoadFailed(e.to_string()))?;

        Ok(config)
    }

    /// Get the configuration file path
    pub fn config_file_path() -> Result<PathBuf> {
        let dirs = Self::project_dirs()?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Get the data directory path
    pub fn data_dir() -> Result<PathBuf> {
        let dirs = Self::project_dirs()?;
        Ok(dirs.data_dir().to_path_buf())
    }

    /// Get the state file path
    pub fn state_file_path() -> Result<PathBuf> {
        Ok(Self::data_dir()?.join("state.json"))
    }

    /// Get the worktrees directory path
    pub fn worktrees_dir(&self) -> Result<PathBuf> {
        if let Some(ref dir) = self.worktrees_dir {
            Ok(dir.clone())
        } else {
            Ok(Self::data_dir()?.join("worktrees"))
        }
    }

    /// Ensure all required directories exist
    pub fn ensure_directories(&self) -> Result<()> {
        let dirs = Self::project_dirs()?;

        std::fs::create_dir_all(dirs.config_dir()).map_err(|e| {
            Error::Config(ConfigError::DirectoryCreationFailed(
                dirs.config_dir().to_path_buf(),
            ))
        })?;

        std::fs::create_dir_all(dirs.data_dir()).map_err(|e| {
            Error::Config(ConfigError::DirectoryCreationFailed(
                dirs.data_dir().to_path_buf(),
            ))
        })?;

        let worktrees_dir = self.worktrees_dir()?;
        std::fs::create_dir_all(&worktrees_dir)
            .map_err(|_| Error::Config(ConfigError::DirectoryCreationFailed(worktrees_dir)))?;

        Ok(())
    }

    /// Save current configuration to file
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_file_path()?;

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                Error::Config(ConfigError::DirectoryCreationFailed(parent.to_path_buf()))
            })?;
        }

        let toml = toml::to_string_pretty(self)
            .map_err(|e| ConfigError::SaveFailed(e.to_string()))?;

        std::fs::write(&config_path, toml)
            .map_err(|e| ConfigError::SaveFailed(e.to_string()))?;

        Ok(())
    }

    fn project_dirs() -> Result<ProjectDirs> {
        ProjectDirs::from("com", "claude-commander", "claude-commander").ok_or_else(|| {
            Error::Config(ConfigError::LoadFailed(
                "Could not determine home directory".to_string(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.default_program, "claude");
        assert_eq!(config.branch_prefix, "");
        assert_eq!(config.max_concurrent_tmux, 16);
        assert_eq!(config.capture_cache_ttl_ms, 50);
        assert_eq!(config.ui_refresh_fps, 30);
    }

    #[test]
    fn test_config_serialization() {
        let config = Config::default();
        let toml = toml::to_string_pretty(&config).unwrap();
        assert!(toml.contains("default_program"));
        assert!(toml.contains("claude"));
    }
}
