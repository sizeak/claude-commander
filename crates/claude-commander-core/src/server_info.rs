//! Runtime info file for a running `claude-commander-server`.
//!
//! On boot the server writes `server-info.json` into its data dir (alongside
//! `state.json`) recording the base `url` clients should hit and the bearer
//! `token` they must present (or `null` when auth is disabled). The CLI reads it
//! to discover a locally-running server without any configuration — e.g. the
//! `slack notify` path a worker session invokes.
//!
//! The file can carry a bearer secret, so it is written `0o600` via the same
//! atomic-rename helper the config file uses, and is best-effort removed on a
//! clean server shutdown. A reader must treat an absent file as "no server
//! running"; a present-but-unreachable server (connection refused) is the
//! caller's concern once it tries to connect, not this reader's.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Name of the runtime info file within the data dir.
const SERVER_INFO_FILENAME: &str = "server-info.json";

/// Connection details for a running server, as written to `server-info.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    /// Base URL clients should hit, e.g. `http://127.0.0.1:7878`.
    pub url: String,
    /// Bearer token to present, or `None` when the server runs with auth
    /// disabled (loopback `--allow-no-auth`).
    #[serde(default)]
    pub token: Option<String>,
}

impl ServerInfo {
    /// Build the info for a server bound at `url`, authed with `token`
    /// (`None` = auth disabled).
    pub fn new(url: impl Into<String>, token: Option<String>) -> Self {
        Self {
            url: url.into(),
            token,
        }
    }

    /// Resolve the info-file path within `dir` (the server's data dir).
    pub fn path_in(dir: &Path) -> PathBuf {
        dir.join(SERVER_INFO_FILENAME)
    }

    /// Write the info file into `dir`, `0o600`, atomically. Creates `dir` if it
    /// does not yet exist.
    pub fn write_to(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::config::write_private_file(&Self::path_in(dir), json)
    }

    /// Read the info file from `dir`. Returns `Ok(None)` when the file is
    /// absent (no server has advertised itself); `Err` on an unreadable or
    /// malformed file.
    pub fn read_from(dir: &Path) -> std::io::Result<Option<Self>> {
        let path = Self::path_in(dir);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let info = serde_json::from_slice(&bytes)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                Ok(Some(info))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Best-effort remove the info file from `dir` (clean-shutdown cleanup).
    /// A missing file is not an error.
    pub fn remove_from(dir: &Path) {
        let _ = std::fs::remove_file(Self::path_in(dir));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_read_remove_round_trip() {
        let dir = TempDir::new().unwrap();
        assert!(
            ServerInfo::read_from(dir.path()).unwrap().is_none(),
            "no file yet → None"
        );

        let info = ServerInfo::new("http://127.0.0.1:7878", Some("sekret".to_string()));
        info.write_to(dir.path()).unwrap();

        let read = ServerInfo::read_from(dir.path()).unwrap().unwrap();
        assert_eq!(read, info);

        ServerInfo::remove_from(dir.path());
        assert!(
            ServerInfo::read_from(dir.path()).unwrap().is_none(),
            "removed file → None"
        );
        // Removing again is a no-op, not an error.
        ServerInfo::remove_from(dir.path());
    }

    #[test]
    fn token_none_serializes_and_reads_back() {
        let dir = TempDir::new().unwrap();
        let info = ServerInfo::new("http://127.0.0.1:7878", None);
        info.write_to(dir.path()).unwrap();
        let read = ServerInfo::read_from(dir.path()).unwrap().unwrap();
        assert!(read.token.is_none());
        assert_eq!(read.url, "http://127.0.0.1:7878");
    }

    #[test]
    fn write_creates_missing_data_dir() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("data").join("nested");
        let info = ServerInfo::new("http://host:1", None);
        info.write_to(&nested).unwrap();
        assert!(ServerInfo::read_from(&nested).unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn info_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        ServerInfo::new("http://127.0.0.1:7878", Some("sekret".to_string()))
            .write_to(dir.path())
            .unwrap();
        let mode = std::fs::metadata(ServerInfo::path_in(dir.path()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "info file may carry a token; must be 0o600");
    }

    #[test]
    fn read_malformed_file_is_error() {
        let dir = TempDir::new().unwrap();
        std::fs::write(ServerInfo::path_in(dir.path()), b"not json").unwrap();
        assert!(ServerInfo::read_from(dir.path()).is_err());
    }
}
