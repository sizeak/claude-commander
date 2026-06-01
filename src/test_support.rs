//! Test-only helpers, including a `MockCommander` that fulfills the
//! `Commander` trait with canned data so callers can be unit-tested
//! without tmux, git, or the filesystem.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::api::{Commander, CreateSessionOpts, SessionDetail, SessionInfo};
use crate::config::{AppState, Config};
use crate::error::{Result, TmuxError};
use crate::session::{ProjectId, ScanResult, SessionId, WorktreeSession};
use crate::tmux::StatusBarInfo;

/// In-memory `Commander` that replays canned query responses and records
/// every mutation it receives. Intended for unit tests that exercise
/// code written against the `Commander` trait without spinning up tmux
/// or git.
pub(crate) struct MockCommander {
    sessions: Vec<SessionInfo>,
    pane_content: Option<String>,
    tmux_ok: bool,
    config: Config,
    calls: Mutex<MockCalls>,
}

#[derive(Default)]
pub(crate) struct MockCalls {
    pub created: Vec<String>,
    pub killed: Vec<SessionId>,
    pub restarted: Vec<SessionId>,
    pub deleted: Vec<SessionId>,
    pub added_projects: Vec<PathBuf>,
    pub scanned: Vec<PathBuf>,
    pub cascade_abandoned: usize,
    pub config_updates: Vec<Config>,
    pub config_reloads: usize,
}

impl MockCommander {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            pane_content: None,
            tmux_ok: true,
            config: Config::default(),
            calls: Mutex::new(MockCalls::default()),
        }
    }

    pub fn with_sessions(mut self, sessions: Vec<SessionInfo>) -> Self {
        self.sessions = sessions;
        self
    }

    pub fn with_pane_content(mut self, content: impl Into<String>) -> Self {
        self.pane_content = Some(content.into());
        self
    }

    pub fn with_tmux_unavailable(mut self) -> Self {
        self.tmux_ok = false;
        self
    }

    pub fn calls(&self) -> std::sync::MutexGuard<'_, MockCalls> {
        self.calls.lock().unwrap()
    }
}

#[async_trait(?Send)]
impl Commander for MockCommander {
    async fn list_sessions(&self, include_stopped: bool) -> Result<Vec<SessionInfo>> {
        Ok(self
            .sessions
            .iter()
            .filter(|s| include_stopped || s.status.is_active())
            .cloned()
            .collect())
    }

    async fn find_session(&self, query: &str) -> Result<Option<SessionInfo>> {
        Ok(self
            .sessions
            .iter()
            .find(|s| s.title == query || s.id.starts_with(query) || s.branch == query)
            .cloned())
    }

    async fn get_session_detail(
        &self,
        query: &str,
        _lines: Option<usize>,
    ) -> Result<Option<SessionDetail>> {
        let Some(info) = self.find_session(query).await? else {
            return Ok(None);
        };
        Ok(Some(SessionDetail {
            info,
            agent_state: crate::session::AgentState::Unknown,
            diff_stat: None,
            pane_content: self.pane_content.clone(),
        }))
    }

    async fn get_pane_content(
        &self,
        _query: &str,
        _lines: Option<usize>,
    ) -> Result<Option<String>> {
        Ok(self.pane_content.clone())
    }

    async fn check_tmux(&self) -> Result<()> {
        if self.tmux_ok {
            Ok(())
        } else {
            Err(TmuxError::NotInstalled.into())
        }
    }

    async fn create_session(&self, opts: CreateSessionOpts) -> Result<SessionId> {
        self.calls.lock().unwrap().created.push(opts.title.clone());
        Ok(SessionId::new())
    }

    async fn ensure_project(&self, path: PathBuf) -> Result<ProjectId> {
        self.calls.lock().unwrap().added_projects.push(path);
        Ok(ProjectId::new())
    }

    async fn add_project(&self, path: PathBuf) -> Result<ProjectId> {
        self.calls.lock().unwrap().added_projects.push(path);
        Ok(ProjectId::new())
    }

    async fn scan_directory(&self, dir: &Path) -> Result<ScanResult> {
        self.calls.lock().unwrap().scanned.push(dir.to_path_buf());
        Ok(ScanResult {
            added: 0,
            skipped: 0,
        })
    }

    async fn cascade_abandon(&self) -> Result<()> {
        self.calls.lock().unwrap().cascade_abandoned += 1;
        Ok(())
    }

    async fn kill_session(&self, id: &SessionId) -> Result<()> {
        self.calls.lock().unwrap().killed.push(*id);
        Ok(())
    }

    async fn restart_session(&self, id: &SessionId) -> Result<()> {
        self.calls.lock().unwrap().restarted.push(*id);
        Ok(())
    }

    async fn delete_session(&self, id: &SessionId) -> Result<()> {
        self.calls.lock().unwrap().deleted.push(*id);
        Ok(())
    }

    fn config(&self) -> Config {
        self.config.clone()
    }

    fn restart_required(&self) -> bool {
        false
    }

    fn reload_config(&self) -> Result<bool> {
        self.calls.lock().unwrap().config_reloads += 1;
        Ok(false)
    }

    fn update_config(&self, config: Config) -> Result<()> {
        self.calls.lock().unwrap().config_updates.push(config);
        Ok(())
    }

    fn status_bar_info(&self, session: &WorktreeSession, _state: &AppState) -> StatusBarInfo {
        StatusBarInfo {
            branch: session.branch.clone(),
            pr_number: session.pr_number,
            pr_merged: session.pr_merged,
            status_style: String::new(),
            is_shell: !session.program.contains("claude"),
            project_name: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{ProjectId, SessionStatus};

    fn session(title: &str, status: SessionStatus) -> SessionInfo {
        let mut s = WorktreeSession::new(
            ProjectId::new(),
            title,
            format!("branch-{}", title),
            PathBuf::from("/tmp/wt"),
            "claude",
        );
        s.set_status(status);
        SessionInfo::from_session(&s, "mock-project")
    }

    #[tokio::test]
    async fn list_sessions_filters_stopped_by_default() {
        let mock = MockCommander::new().with_sessions(vec![
            session("running", SessionStatus::Running),
            session("stopped", SessionStatus::Stopped),
        ]);

        let active = mock.list_sessions(false).await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].title, "running");

        let all = mock.list_sessions(true).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn find_session_matches_by_title() {
        let mock = MockCommander::new().with_sessions(vec![session(
            "fix-auth",
            SessionStatus::Running,
        )]);
        let found = mock.find_session("fix-auth").await.unwrap();
        assert_eq!(found.unwrap().title, "fix-auth");
    }

    #[tokio::test]
    async fn check_tmux_returns_canned_failure() {
        let mock = MockCommander::new().with_tmux_unavailable();
        assert!(mock.check_tmux().await.is_err());
    }

    #[tokio::test]
    async fn mutations_are_recorded() {
        let mock = MockCommander::new();
        let id = SessionId::new();
        mock.kill_session(&id).await.unwrap();
        mock.delete_session(&id).await.unwrap();
        mock.cascade_abandon().await.unwrap();

        let calls = mock.calls();
        assert_eq!(calls.killed, vec![id]);
        assert_eq!(calls.deleted, vec![id]);
        assert_eq!(calls.cascade_abandoned, 1);
    }

    #[tokio::test]
    async fn pane_content_is_replayed_through_query_methods() {
        let mock = MockCommander::new()
            .with_sessions(vec![session("with-pane", SessionStatus::Running)])
            .with_pane_content("hello from tmux");

        assert_eq!(
            mock.get_pane_content("with-pane", Some(10)).await.unwrap(),
            Some("hello from tmux".to_string())
        );
        let detail = mock
            .get_session_detail("with-pane", Some(10))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail.pane_content.as_deref(), Some("hello from tmux"));
    }

    #[tokio::test]
    async fn mock_is_usable_through_dyn_commander() {
        let mock: Box<dyn Commander> =
            Box::new(MockCommander::new().with_sessions(vec![session(
                "via-dyn",
                SessionStatus::Running,
            )]));
        let listed = mock.list_sessions(false).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "via-dyn");
    }
}
