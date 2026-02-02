//! Persistent state storage
//!
//! Manages session state persistence in JSON format

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{ConfigError, Result};
use crate::session::{Project, ProjectId, SessionId, WorktreeSession};

use super::Config;

/// Persistent application state
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppState {
    /// All registered projects
    #[serde(default)]
    pub projects: HashMap<ProjectId, Project>,

    /// All worktree sessions
    #[serde(default)]
    pub sessions: HashMap<SessionId, WorktreeSession>,

    /// Whether the user has seen the help screen
    #[serde(default)]
    pub seen_help: bool,

    /// Last selected project ID
    #[serde(default)]
    pub last_selected_project: Option<ProjectId>,

    /// Last selected session ID
    #[serde(default)]
    pub last_selected_session: Option<SessionId>,

    /// Application version that last wrote this state
    #[serde(default)]
    pub version: String,

    /// Path to save state to (not serialized, set at load time)
    #[serde(skip)]
    state_path: Option<PathBuf>,
}

impl AppState {
    /// Create a new empty state
    pub fn new() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            ..Default::default()
        }
    }

    /// Load state from the default location
    pub fn load() -> Result<Self> {
        let path = Config::state_file_path()?;
        Self::load_from(&path)
    }

    /// Load state from a specific path
    pub fn load_from(path: &PathBuf) -> Result<Self> {
        if !path.exists() {
            let mut state = Self::new();
            state.state_path = Some(path.clone());
            return Ok(state);
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::LoadFailed(format!("Failed to read state file: {}", e)))?;

        let mut state: AppState = serde_json::from_str(&content)
            .map_err(|e| ConfigError::LoadFailed(format!("Failed to parse state file: {}", e)))?;

        // Update version and remember path
        state.version = env!("CARGO_PKG_VERSION").to_string();
        state.state_path = Some(path.clone());

        Ok(state)
    }

    /// Save state to the remembered location (or default if none)
    pub fn save(&self) -> Result<()> {
        let path = match &self.state_path {
            Some(p) => p.clone(),
            None => Config::state_file_path()?,
        };
        self.save_to(&path)
    }

    /// Save state to a specific path
    pub fn save_to(&self, path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ConfigError::SaveFailed(format!("Failed to create state directory: {}", e))
            })?;
        }

        let content = serde_json::to_string_pretty(self)
            .map_err(|e| ConfigError::SaveFailed(format!("Failed to serialize state: {}", e)))?;

        std::fs::write(path, content)
            .map_err(|e| ConfigError::SaveFailed(format!("Failed to write state file: {}", e)))?;

        Ok(())
    }

    /// Add a project
    pub fn add_project(&mut self, project: Project) {
        self.projects.insert(project.id, project);
    }

    /// Remove a project and all its sessions
    pub fn remove_project(&mut self, project_id: &ProjectId) -> Option<Project> {
        if let Some(project) = self.projects.remove(project_id) {
            // Remove all associated sessions
            for session_id in &project.worktrees {
                self.sessions.remove(session_id);
            }
            Some(project)
        } else {
            None
        }
    }

    /// Get a project by ID
    pub fn get_project(&self, id: &ProjectId) -> Option<&Project> {
        self.projects.get(id)
    }

    /// Get a mutable reference to a project
    pub fn get_project_mut(&mut self, id: &ProjectId) -> Option<&mut Project> {
        self.projects.get_mut(id)
    }

    /// Add a session
    pub fn add_session(&mut self, session: WorktreeSession) {
        let project_id = session.project_id;
        let session_id = session.id;

        self.sessions.insert(session_id, session);

        // Add to parent project
        if let Some(project) = self.projects.get_mut(&project_id) {
            project.add_worktree(session_id);
        }
    }

    /// Remove a session
    pub fn remove_session(&mut self, session_id: &SessionId) -> Option<WorktreeSession> {
        if let Some(session) = self.sessions.remove(session_id) {
            // Remove from parent project
            if let Some(project) = self.projects.get_mut(&session.project_id) {
                project.remove_worktree(session_id);
            }
            Some(session)
        } else {
            None
        }
    }

    /// Get a session by ID
    pub fn get_session(&self, id: &SessionId) -> Option<&WorktreeSession> {
        self.sessions.get(id)
    }

    /// Get a mutable reference to a session
    pub fn get_session_mut(&mut self, id: &SessionId) -> Option<&mut WorktreeSession> {
        self.sessions.get_mut(id)
    }

    /// Get all sessions for a project
    pub fn get_project_sessions(&self, project_id: &ProjectId) -> Vec<&WorktreeSession> {
        self.sessions
            .values()
            .filter(|s| s.project_id == *project_id)
            .collect()
    }

    /// Get all active sessions
    pub fn get_active_sessions(&self) -> Vec<&WorktreeSession> {
        self.sessions
            .values()
            .filter(|s| s.status.is_active())
            .collect()
    }

    /// Count total sessions
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Count total projects
    pub fn project_count(&self) -> usize {
        self.projects.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn create_test_project() -> Project {
        Project::new("test-project", PathBuf::from("/tmp/test"), "main")
    }

    fn create_test_session(project_id: ProjectId) -> WorktreeSession {
        WorktreeSession::new(
            project_id,
            "Test Session",
            "test-branch",
            PathBuf::from("/tmp/worktree"),
            "claude",
        )
    }

    #[test]
    fn test_new_state() {
        let state = AppState::new();
        assert!(state.projects.is_empty());
        assert!(state.sessions.is_empty());
        assert!(!state.seen_help);
    }

    #[test]
    fn test_add_remove_project() {
        let mut state = AppState::new();
        let project = create_test_project();
        let project_id = project.id;

        state.add_project(project);
        assert_eq!(state.project_count(), 1);

        let removed = state.remove_project(&project_id);
        assert!(removed.is_some());
        assert_eq!(state.project_count(), 0);
    }

    #[test]
    fn test_add_remove_session() {
        let mut state = AppState::new();
        let project = create_test_project();
        let project_id = project.id;
        state.add_project(project);

        let session = create_test_session(project_id);
        let session_id = session.id;

        state.add_session(session);
        assert_eq!(state.session_count(), 1);

        // Check session is linked to project
        let project = state.get_project(&project_id).unwrap();
        assert!(project.worktrees.contains(&session_id));

        // Remove session
        let removed = state.remove_session(&session_id);
        assert!(removed.is_some());
        assert_eq!(state.session_count(), 0);

        // Check session is unlinked from project
        let project = state.get_project(&project_id).unwrap();
        assert!(!project.worktrees.contains(&session_id));
    }

    #[test]
    fn test_save_load_roundtrip() {
        let temp_dir = TempDir::new().unwrap();
        let state_path = temp_dir.path().join("state.json");

        let mut state = AppState::new();
        let project = create_test_project();
        let project_id = project.id;
        state.add_project(project);

        let session = create_test_session(project_id);
        state.add_session(session);

        state.save_to(&state_path).unwrap();

        let loaded = AppState::load_from(&state_path).unwrap();
        assert_eq!(loaded.project_count(), 1);
        assert_eq!(loaded.session_count(), 1);
    }

    #[test]
    fn test_get_project_sessions() {
        let mut state = AppState::new();

        let project1 = create_test_project();
        let project1_id = project1.id;
        state.add_project(project1);

        let mut project2 = create_test_project();
        project2.name = "other-project".to_string();
        let project2_id = project2.id;
        state.add_project(project2);

        // Add sessions to project1
        state.add_session(create_test_session(project1_id));
        state.add_session(create_test_session(project1_id));

        // Add session to project2
        state.add_session(create_test_session(project2_id));

        let p1_sessions = state.get_project_sessions(&project1_id);
        assert_eq!(p1_sessions.len(), 2);

        let p2_sessions = state.get_project_sessions(&project2_id);
        assert_eq!(p2_sessions.len(), 1);
    }
}
