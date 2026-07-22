//! `claude-commander slack notify` preparation logic.
//!
//! The CLI half of the notify path: resolve *which* session a worker's message
//! is about (an explicit `--session`, or the session whose worktree contains the
//! current directory) and *where* to send it (the locally-running server,
//! discovered via its [`ServerInfo`] file). The actual HTTP POST lives in the
//! transport crate; keeping the resolution + server-discovery logic here means
//! `main.rs` stays a thin wire-up and this is unit-testable without a network.

use std::path::Path;

use thiserror::Error;

use claude_commander_protocol::api::SlackNotifyRequest;

use crate::config::AppState;
use crate::server_info::ServerInfo;
use crate::session::WorktreeSession;

/// Why a `slack notify` could not be prepared. Every variant renders an
/// actionable message so the CLI can print it and exit non-zero.
#[derive(Debug, Error)]
pub enum NotifyPrepError {
    /// No `--session` was given and the current directory isn't inside any
    /// session's worktree.
    #[error("no session for the current directory; pass --session <name-or-id>")]
    NoSessionForCwd,
    /// No server is advertising itself (no `server-info.json`), so there's
    /// nothing to POST to.
    #[error("server not running — notify unavailable (start claude-commander-server)")]
    ServerNotRunning,
    /// The info file exists but couldn't be read/parsed.
    #[error("could not read server info: {0}")]
    ServerInfoUnreadable(String),
}

/// Find the session whose worktree contains `cwd` (equal to, or nested under,
/// the worktree path). The deepest match wins so a nested worktree beats an
/// ancestor. Sessions with no worktree path yet (still `Creating`) are skipped.
pub fn resolve_cwd_session<'a>(state: &'a AppState, cwd: &Path) -> Option<&'a WorktreeSession> {
    state
        .sessions
        .values()
        .filter(|s| !s.worktree_path.as_os_str().is_empty() && cwd.starts_with(&s.worktree_path))
        .max_by_key(|s| s.worktree_path.components().count())
}

/// Resolve the session query for a notify: an explicit `--session` passes
/// through verbatim (the server resolves/404s it); otherwise the cwd's worktree
/// session's full id.
pub fn resolve_notify_query(
    state: &AppState,
    session: Option<String>,
    cwd: &Path,
) -> Result<String, NotifyPrepError> {
    match session {
        Some(q) => Ok(q),
        None => resolve_cwd_session(state, cwd)
            .map(|s| s.id.as_uuid().to_string())
            .ok_or(NotifyPrepError::NoSessionForCwd),
    }
}

/// Prepare a notify: resolve the session query, then read the running server's
/// [`ServerInfo`] from `data_dir`. Returns the transport target plus the request
/// body to POST. The caller performs the HTTP call and reports its outcome.
pub fn prepare_notify(
    state: &AppState,
    data_dir: &Path,
    session: Option<String>,
    message: String,
    cwd: &Path,
) -> Result<(ServerInfo, SlackNotifyRequest), NotifyPrepError> {
    let query = resolve_notify_query(state, session, cwd)?;
    let info = match ServerInfo::read_from(data_dir) {
        Ok(Some(info)) => info,
        Ok(None) => return Err(NotifyPrepError::ServerNotRunning),
        Err(e) => return Err(NotifyPrepError::ServerInfoUnreadable(e.to_string())),
    };
    Ok((
        info,
        SlackNotifyRequest {
            session: query,
            message,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Project, WorktreeSession};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Seed a state with one session whose worktree is `worktree`.
    fn state_with_worktree(worktree: PathBuf) -> (AppState, crate::session::SessionId) {
        let mut state = AppState::new();
        let project = Project::new("p", PathBuf::from("/tmp/p"), "main");
        let pid = project.id;
        let session = WorktreeSession::new(pid, "fix-auth", "fix-auth-br", worktree, "claude");
        let sid = session.id;
        let mut proj = project;
        proj.add_worktree(sid);
        state.projects.insert(pid, proj);
        state.sessions.insert(sid, session);
        (state, sid)
    }

    #[test]
    fn resolve_cwd_session_matches_when_cwd_is_inside_worktree() {
        let root = TempDir::new().unwrap();
        let worktree = root.path().join("wt");
        let (state, sid) = state_with_worktree(worktree.clone());
        // A nested directory inside the worktree resolves to that session.
        let nested = worktree.join("src").join("deep");
        assert_eq!(resolve_cwd_session(&state, &nested).unwrap().id, sid);
        // The worktree root itself also matches.
        assert_eq!(resolve_cwd_session(&state, &worktree).unwrap().id, sid);
    }

    #[test]
    fn resolve_cwd_session_none_when_cwd_outside_any_worktree() {
        let root = TempDir::new().unwrap();
        let (state, _) = state_with_worktree(root.path().join("wt"));
        let elsewhere = root.path().join("other");
        assert!(resolve_cwd_session(&state, &elsewhere).is_none());
    }

    #[test]
    fn resolve_notify_query_explicit_session_passes_through() {
        let root = TempDir::new().unwrap();
        let (state, _) = state_with_worktree(root.path().join("wt"));
        // An explicit query wins even from an unrelated cwd.
        let q = resolve_notify_query(&state, Some("some-name".into()), root.path()).unwrap();
        assert_eq!(q, "some-name");
    }

    #[test]
    fn prepare_notify_errors_when_no_session_for_cwd() {
        let root = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        let (state, _) = state_with_worktree(root.path().join("wt"));
        let err = prepare_notify(
            &state,
            data.path(),
            None,
            "hi".into(),
            &root.path().join("unrelated"),
        )
        .unwrap_err();
        assert!(matches!(err, NotifyPrepError::NoSessionForCwd));
    }

    #[test]
    fn prepare_notify_errors_when_server_not_running() {
        let root = TempDir::new().unwrap();
        let data = TempDir::new().unwrap(); // no server-info.json written
        let (state, _) = state_with_worktree(root.path().join("wt"));
        let err = prepare_notify(
            &state,
            data.path(),
            Some("fix-auth".into()),
            "hi".into(),
            root.path(),
        )
        .unwrap_err();
        assert!(matches!(err, NotifyPrepError::ServerNotRunning));
    }

    #[test]
    fn prepare_notify_builds_request_from_cwd_and_server_info() {
        let root = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        let worktree = root.path().join("wt");
        let (state, sid) = state_with_worktree(worktree.clone());
        ServerInfo::new("http://127.0.0.1:7878", Some("tok".into()))
            .write_to(data.path())
            .unwrap();

        let (info, req) =
            prepare_notify(&state, data.path(), None, "done".into(), &worktree).unwrap();
        assert_eq!(info.url, "http://127.0.0.1:7878");
        assert_eq!(info.token.as_deref(), Some("tok"));
        // The cwd resolved to the seeded session's full id.
        assert_eq!(req.session, sid.as_uuid().to_string());
        assert_eq!(req.message, "done");
    }
}
