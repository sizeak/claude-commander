//! Commander API — unified service layer for CLI and TUI consumers.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::config::{AppState, ConfigStore, StateStore};
use crate::error::Result;
use crate::git::{PrState, ReviewDecision, diff_stat_summary, effective_pr_state};
use crate::session::{
    AgentState, ProjectId, SessionId, SessionManager, SessionStatus, WorktreeSession,
};
use crate::tmux::{AgentStateDetector, TmuxExecutor};
use crate::tui::theme::Theme;

/// High-level service that wraps `SessionManager`, state stores, and agent
/// detection into a single entry point. Both the CLI and TUI route through
/// this rather than wiring the pieces together independently.
pub struct CommanderService {
    manager: SessionManager,
    store: Arc<StateStore>,
    config_store: Arc<ConfigStore>,
}

impl CommanderService {
    pub fn new(config_store: Arc<ConfigStore>, store: Arc<StateStore>) -> Self {
        let manager = SessionManager::new(
            config_store.clone(),
            store.clone(),
            Theme::default().tmux_status_style(),
        );
        Self {
            manager,
            store,
            config_store,
        }
    }

    pub fn for_cli(config: crate::config::Config) -> std::result::Result<Self, crate::Error> {
        let config_store = Arc::new(ConfigStore::new(config)?);
        let app_state = AppState::load().unwrap_or_else(|_| AppState::new());
        let store = Arc::new(StateStore::new(app_state)?);
        Ok(Self::new(config_store, store))
    }

    pub fn session_manager(&self) -> &SessionManager {
        &self.manager
    }

    pub fn store(&self) -> &Arc<StateStore> {
        &self.store
    }

    pub fn config_store(&self) -> &Arc<ConfigStore> {
        &self.config_store
    }

    // -- Queries --

    pub async fn list_sessions(&self, include_stopped: bool) -> Result<Vec<SessionInfo>> {
        let state = self.store.read().await;
        Ok(build_session_info_list(&state, include_stopped))
    }

    pub async fn find_session(&self, query: &str) -> Result<Option<SessionInfo>> {
        let state = self.store.read().await;
        Ok(find_session_info(&state, query))
    }

    pub async fn get_session_detail(
        &self,
        query: &str,
        lines: Option<usize>,
    ) -> Result<Option<SessionDetail>> {
        let (found, project_name) = {
            let state = self.store.read().await;
            let Some(session) = crate::cli::find_session(&state, query) else {
                return Ok(None);
            };
            let pname = state
                .projects
                .get(&session.project_id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "unknown".to_string());
            (session.clone(), pname)
        };

        let agent_state = if found.status.is_active() {
            let executor = TmuxExecutor::new();
            let mut detector = AgentStateDetector::new(executor, Duration::ZERO);
            detector.detect(&found.tmux_session_name).await
        } else {
            AgentState::Unknown
        };

        let diff_stat = if found.worktree_path.exists() {
            let diff_base = found.base_commit.as_deref().unwrap_or("HEAD");
            diff_stat_summary(&found.worktree_path, diff_base).await
        } else {
            None
        };

        let pane_content = if found.status.is_active() {
            let n = lines.map(crate::cli::clamp_log_lines);
            capture_pane(&self.manager.tmux, &found.tmux_session_name, n).await?
        } else {
            None
        };

        Ok(Some(SessionDetail {
            info: SessionInfo::from_session(&found, &project_name),
            agent_state,
            diff_stat,
            pane_content,
        }))
    }

    pub async fn get_pane_content(
        &self,
        query: &str,
        lines: Option<usize>,
    ) -> Result<Option<String>> {
        let state = self.store.read().await;
        let Some(session) = crate::cli::find_session(&state, query) else {
            return Ok(None);
        };
        let tmux_name = session.tmux_session_name.clone();
        drop(state);

        let n = lines.map(crate::cli::clamp_log_lines);
        capture_pane(&self.manager.tmux, &tmux_name, n).await
    }

    pub async fn check_tmux(&self) -> Result<()> {
        self.manager.check_tmux().await
    }
}

// -- Response types --

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub session_id: SessionId,
    pub title: String,
    pub branch: String,
    pub status: SessionStatus,
    pub program: String,
    pub project_id: ProjectId,
    pub project_name: String,
    pub pr_number: Option<u32>,
    pub pr_url: Option<String>,
    pub pr_state: PrState,
    pub pr_draft: bool,
    pub pr_labels: Vec<String>,
    pub review_decision: Option<ReviewDecision>,
    pub pr_reviewers: Vec<String>,
    pub created_at: DateTime<Utc>,
}

impl SessionInfo {
    pub fn from_session(session: &WorktreeSession, project_name: &str) -> Self {
        Self {
            id: session.id.as_uuid().to_string(),
            session_id: session.id,
            title: session.title.clone(),
            branch: session.branch.clone(),
            status: session.status,
            program: session.program.clone(),
            project_id: session.project_id,
            project_name: project_name.to_string(),
            pr_number: session.pr_number,
            pr_url: session.pr_url.clone(),
            pr_state: effective_pr_state(session.pr_state, session.pr_merged),
            pr_draft: session.pr_draft,
            pr_labels: session.pr_labels.clone(),
            review_decision: session.review_decision,
            pr_reviewers: session.pr_reviewers.clone(),
            created_at: session.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionDetail {
    #[serde(flatten)]
    pub info: SessionInfo,
    pub agent_state: AgentState,
    pub diff_stat: Option<String>,
    pub pane_content: Option<String>,
}

// -- Internal helpers --

fn build_session_info_list(state: &AppState, include_stopped: bool) -> Vec<SessionInfo> {
    let mut entries = Vec::new();
    for project in state.projects.values() {
        for session in project
            .worktrees
            .iter()
            .filter_map(|id| state.sessions.get(id))
            .filter(|s| include_stopped || s.status.is_active())
        {
            entries.push(SessionInfo::from_session(session, &project.name));
        }
    }
    entries
}

fn find_session_info(state: &AppState, query: &str) -> Option<SessionInfo> {
    let session = crate::cli::find_session(state, query)?;
    let project_name = state
        .projects
        .get(&session.project_id)
        .map(|p| p.name.as_str())
        .unwrap_or("unknown");
    Some(SessionInfo::from_session(session, project_name))
}

async fn capture_pane(
    executor: &TmuxExecutor,
    tmux_name: &str,
    lines: Option<usize>,
) -> Result<Option<String>> {
    if !executor.session_exists(tmux_name).await? {
        return Ok(None);
    }
    let mut args = vec!["capture-pane", "-t", tmux_name, "-p"];
    let lines_arg;
    if let Some(n) = lines {
        lines_arg = format!("-{}", n);
        args.extend_from_slice(&["-S", &lines_arg]);
    }
    Ok(Some(executor.execute(&args).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Project, ProjectId, WorktreeSession};
    use std::path::PathBuf;

    fn make_project(name: &str) -> Project {
        Project::new(name, PathBuf::from("/tmp/repo"), "main")
    }

    fn make_session_for_project(title: &str, project_id: ProjectId) -> WorktreeSession {
        WorktreeSession::new(
            project_id,
            title,
            format!("branch-{}", title),
            PathBuf::from("/tmp/wt"),
            "claude",
        )
    }

    fn make_state_with_project(project: &Project, sessions: Vec<WorktreeSession>) -> AppState {
        let mut state = AppState::new();
        let mut proj = project.clone();
        for s in &sessions {
            proj.add_worktree(s.id);
        }
        state.projects.insert(proj.id, proj);
        for s in sessions {
            state.sessions.insert(s.id, s);
        }
        state
    }

    #[test]
    fn session_info_from_session_populates_fields() {
        let session = make_session_for_project("fix-bug", ProjectId::new());
        let info = SessionInfo::from_session(&session, "my-project");

        assert_eq!(info.title, "fix-bug");
        assert_eq!(info.branch, "branch-fix-bug");
        assert_eq!(info.program, "claude");
        assert_eq!(info.project_name, "my-project");
        assert_eq!(info.session_id, session.id);
        assert!(uuid::Uuid::parse_str(&info.id).is_ok());
    }

    #[test]
    fn session_info_resolves_legacy_pr_merged() {
        let mut session = make_session_for_project("legacy", ProjectId::new());
        session.pr_number = Some(10);
        session.pr_state = None;
        session.pr_merged = true;

        let info = SessionInfo::from_session(&session, "proj");
        assert_eq!(info.pr_state, PrState::Merged);
    }

    #[test]
    fn build_list_excludes_stopped_by_default() {
        let project = make_project("repo");
        let mut s1 = make_session_for_project("running", project.id);
        s1.set_status(SessionStatus::Running);
        let mut s2 = make_session_for_project("stopped", project.id);
        s2.set_status(SessionStatus::Stopped);

        let state = make_state_with_project(&project, vec![s1, s2]);
        let list = build_session_info_list(&state, false);

        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "running");
    }

    #[test]
    fn build_list_includes_stopped_when_requested() {
        let project = make_project("repo");
        let mut s1 = make_session_for_project("running", project.id);
        s1.set_status(SessionStatus::Running);
        let mut s2 = make_session_for_project("stopped", project.id);
        s2.set_status(SessionStatus::Stopped);

        let state = make_state_with_project(&project, vec![s1, s2]);
        let list = build_session_info_list(&state, true);

        assert_eq!(list.len(), 2);
    }

    #[test]
    fn build_list_populates_project_name() {
        let project = make_project("my-repo");
        let s = make_session_for_project("task", project.id);
        let state = make_state_with_project(&project, vec![s]);
        let list = build_session_info_list(&state, false);

        assert_eq!(list.len(), 1);
        assert_eq!(list[0].project_name, "my-repo");
    }

    #[test]
    fn find_session_info_by_title() {
        let project = make_project("repo");
        let s = make_session_for_project("fix-auth", project.id);
        let expected_id = s.id;
        let state = make_state_with_project(&project, vec![s]);

        let info = find_session_info(&state, "fix-auth").unwrap();
        assert_eq!(info.session_id, expected_id);
        assert_eq!(info.project_name, "repo");
    }

    #[test]
    fn find_session_info_returns_none_for_missing() {
        let state = AppState::new();
        assert!(find_session_info(&state, "nope").is_none());
    }

    #[test]
    fn session_detail_flattens_info_in_json() {
        let session = make_session_for_project("test", ProjectId::new());
        let detail = SessionDetail {
            info: SessionInfo::from_session(&session, "proj"),
            agent_state: AgentState::Working,
            diff_stat: Some("3 files changed".to_string()),
            pane_content: None,
        };
        let json: serde_json::Value = serde_json::to_value(&detail).unwrap();
        assert_eq!(json["title"], "test");
        assert_eq!(json["agent_state"], "working");
        assert_eq!(json["diff_stat"], "3 files changed");
        assert!(json["pane_content"].is_null());
    }
}
