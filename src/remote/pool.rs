//! Per-host remote-runner pool.
//!
//! Keeps one persistent [`RemoteRunner`] per `RemoteTransport::connection_key()`:
//! - `Ssh { host }` → [`OpensshRunner`] over a pooled `openssh::Session`
//!   (ControlMaster-multiplexed; subsequent commands cost ~1 RTT).
//! - `Codespace { name }` → [`GhCodespaceRunner`] which wraps each command
//!   in `gh codespace ssh -c <name> --` (one process per call).

use std::collections::HashMap;
use std::sync::Arc;

use openssh::{KnownHosts, Session, SessionBuilder};
use tokio::sync::Mutex;
use tracing::{debug, instrument};

use super::codespace;
use super::runner::{GhCodespaceRunner, OpensshRunner, RemoteRunner};
use crate::error::{Error, GitError, Result};
use crate::session::RemoteTransport;

/// Pool init helper: eagerly capture the login-shell env on the runner so
/// the first real command doesn't pay the profile-sourcing cost (and the
/// "Connecting…" loading modal accurately covers the warm-up time).
async fn warm_env_capture(runner: &Arc<dyn RemoteRunner>) -> Result<()> {
    // The trait doesn't expose `captured_env` directly. Instead, drive a
    // trivial command (`true`) which internally triggers the OnceCell init.
    let _ = runner.run(&["true"]).await?;
    Ok(())
}

/// Pool of remote runners, keyed by `RemoteTransport::connection_key()`.
#[derive(Default)]
pub struct SshSessionPool {
    runners: Mutex<HashMap<String, Arc<dyn RemoteRunner>>>,
}

impl SshSessionPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get a runner for `transport`, opening one if not pooled.
    #[instrument(skip(self))]
    pub async fn get_or_connect(
        &self,
        transport: &RemoteTransport,
    ) -> Result<Arc<dyn RemoteRunner>> {
        let key = transport.connection_key();
        {
            let guard = self.runners.lock().await;
            if let Some(r) = guard.get(&key) {
                debug!("Reusing pooled runner for {}", key);
                return Ok(Arc::clone(r));
            }
        }

        let runner: Arc<dyn RemoteRunner> = match transport {
            RemoteTransport::Ssh { host } => {
                debug!("Opening openssh session to {}", host);
                let session = SessionBuilder::default()
                    .known_hosts_check(KnownHosts::Strict)
                    .connect(host)
                    .await
                    .map_err(|e| {
                        Error::Git(GitError::WorktreeError(format!(
                            "SSH connect to {} failed: {}",
                            host, e
                        )))
                    })?;
                let runner: Arc<dyn RemoteRunner> = Arc::new(OpensshRunner::new(Arc::new(session)));
                warm_env_capture(&runner).await?;
                runner
            }
            RemoteTransport::Codespace { name } => {
                // Verify the codespace exists before we start — surfaces a
                // useful error if not. The runner's first call (the env
                // capture below) doubles as the wake-up: it retries on
                // transient gh-RPC failures so a Shutdown codespace boots
                // and a freshly-rebuilt one settles before we declare
                // success.
                let _ = codespace::gh_codespace_view(name).await?;
                debug!("Codespace {}: building runner + warming env", name);
                let runner: Arc<dyn RemoteRunner> = Arc::new(GhCodespaceRunner::new(name.clone()));
                warm_env_capture(&runner).await?;
                runner
            }
        };

        let mut guard = self.runners.lock().await;
        if let Some(r) = guard.get(&key) {
            return Ok(Arc::clone(r));
        }
        guard.insert(key, Arc::clone(&runner));
        Ok(runner)
    }

    /// Drop a runner from the pool — used when a connection drops or when
    /// the user removes a remote project.
    pub async fn evict(&self, transport: &RemoteTransport) {
        let key = transport.connection_key();
        let mut guard = self.runners.lock().await;
        guard.remove(&key);
    }
}

// Suppress unused: `Session` import is part of `SessionBuilder::connect`'s
// return type but not named explicitly; keep the use to make refactors easier.
#[allow(dead_code)]
fn _force_session_use(_s: Session) {}
