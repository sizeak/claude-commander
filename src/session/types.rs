//! Core session types
//!
//! Defines the hierarchical session model:
//! - `Project` represents a git repository
//! - `WorktreeSession` represents a worktree session within a project

use std::fmt;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a project (git repository)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectId(Uuid);

impl ProjectId {
    /// Create a new random project ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Create from an existing UUID
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl Default for ProjectId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Use first 8 chars for display
        write!(f, "{}", &self.0.to_string()[..8])
    }
}

/// Unique identifier for a worktree session
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(Uuid);

impl SessionId {
    /// Create a new random session ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Create from an existing UUID
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Get the inner UUID
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Use first 8 chars for display
        write!(f, "{}", &self.0.to_string()[..8])
    }
}

/// Status of a worktree session
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Session is running and active
    Running,
    /// Session is paused (tmux detached, worktree preserved)
    Paused,
    /// Session has completed or been killed
    Stopped,
}

impl SessionStatus {
    /// Check if the session is active (running or paused)
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Running | Self::Paused)
    }

    /// Check if the session can be attached to
    pub fn can_attach(&self) -> bool {
        matches!(self, Self::Running | Self::Paused)
    }

    /// Check if the session can be paused
    pub fn can_pause(&self) -> bool {
        matches!(self, Self::Running)
    }

    /// Check if the session can be resumed
    pub fn can_resume(&self) -> bool {
        matches!(self, Self::Paused)
    }
}

impl fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Paused => write!(f, "paused"),
            Self::Stopped => write!(f, "stopped"),
        }
    }
}

/// Detected state of the agent running in the session
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Agent is waiting for input (prompt visible)
    #[default]
    WaitingForInput,
    /// Agent is actively processing
    Processing,
    /// Agent encountered an error
    Error,
    /// Agent state is unknown (e.g., just started)
    Unknown,
}

impl fmt::Display for AgentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WaitingForInput => write!(f, "waiting"),
            Self::Processing => write!(f, "processing"),
            Self::Error => write!(f, "error"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Project represents a git repository (parent session)
///
/// A project is the top-level container that holds:
/// - Reference to the main git repository
/// - Collection of worktree sessions
/// - Project-level metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    /// Unique identifier
    pub id: ProjectId,
    /// Display name (typically repo directory name)
    pub name: String,
    /// Path to the main repository
    pub repo_path: PathBuf,
    /// Main branch name (e.g., "main", "master")
    pub main_branch: String,
    /// When the project was added
    pub created_at: DateTime<Utc>,
    /// Worktree sessions belonging to this project
    #[serde(default)]
    pub worktrees: Vec<SessionId>,
}

impl Project {
    /// Create a new project
    pub fn new(name: impl Into<String>, repo_path: PathBuf, main_branch: impl Into<String>) -> Self {
        Self {
            id: ProjectId::new(),
            name: name.into(),
            repo_path,
            main_branch: main_branch.into(),
            created_at: Utc::now(),
            worktrees: Vec::new(),
        }
    }

    /// Add a worktree session to this project
    pub fn add_worktree(&mut self, session_id: SessionId) {
        if !self.worktrees.contains(&session_id) {
            self.worktrees.push(session_id);
        }
    }

    /// Remove a worktree session from this project
    pub fn remove_worktree(&mut self, session_id: &SessionId) {
        self.worktrees.retain(|id| id != session_id);
    }
}

/// WorktreeSession represents a git worktree with an associated tmux session
///
/// Each worktree session:
/// - Belongs to a parent project
/// - Has its own git branch
/// - Has an isolated working directory
/// - Has a dedicated tmux session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeSession {
    /// Unique identifier
    pub id: SessionId,
    /// Parent project ID
    pub project_id: ProjectId,
    /// User-friendly title
    pub title: String,
    /// Git branch name
    pub branch: String,
    /// Path to the worktree directory
    pub worktree_path: PathBuf,
    /// Current status
    pub status: SessionStatus,
    /// Detected agent state
    pub agent_state: AgentState,
    /// Program running in the session (e.g., "claude", "aider")
    pub program: String,
    /// When the session was created
    pub created_at: DateTime<Utc>,
    /// When the session was last active
    pub last_active_at: DateTime<Utc>,
    /// Tmux session name (for tmux commands)
    pub tmux_session_name: String,
    /// Base commit for diff computation (branch point)
    #[serde(default)]
    pub base_commit: Option<String>,
}

impl WorktreeSession {
    /// Create a new worktree session
    pub fn new(
        project_id: ProjectId,
        title: impl Into<String>,
        branch: impl Into<String>,
        worktree_path: PathBuf,
        program: impl Into<String>,
    ) -> Self {
        let id = SessionId::new();
        let title = title.into();
        let now = Utc::now();

        // Generate tmux session name from ID (short, unique)
        let tmux_session_name = format!("cc-{}", &id.0.to_string()[..8]);

        Self {
            id,
            project_id,
            title,
            branch: branch.into(),
            worktree_path,
            status: SessionStatus::Running,
            agent_state: AgentState::Unknown,
            program: program.into(),
            created_at: now,
            last_active_at: now,
            tmux_session_name,
            base_commit: None,
        }
    }

    /// Update the session status
    pub fn set_status(&mut self, status: SessionStatus) {
        self.status = status;
        if status == SessionStatus::Running {
            self.last_active_at = Utc::now();
        }
    }

    /// Update the agent state
    pub fn set_agent_state(&mut self, state: AgentState) {
        self.agent_state = state;
        self.last_active_at = Utc::now();
    }

    /// Mark the session as active (update last_active_at)
    pub fn touch(&mut self) {
        self.last_active_at = Utc::now();
    }

    /// Check if this session matches a search query
    pub fn matches_query(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        self.title.to_lowercase().contains(&query)
            || self.branch.to_lowercase().contains(&query)
            || self.program.to_lowercase().contains(&query)
    }
}

/// Represents an item in the hierarchical session list
/// Used for UI display and navigation
#[derive(Debug, Clone)]
pub enum SessionListItem {
    /// A project header
    Project {
        id: ProjectId,
        name: String,
        repo_path: PathBuf,
        main_branch: String,
        worktree_count: usize,
    },
    /// A worktree session (indented under project)
    Worktree {
        id: SessionId,
        project_id: ProjectId,
        title: String,
        branch: String,
        status: SessionStatus,
        agent_state: AgentState,
        program: String,
    },
}

impl SessionListItem {
    /// Get a unique key for this item (for selection tracking)
    pub fn key(&self) -> String {
        match self {
            Self::Project { id, .. } => format!("project:{}", id),
            Self::Worktree { id, .. } => format!("worktree:{}", id),
        }
    }

    /// Check if this is a project item
    pub fn is_project(&self) -> bool {
        matches!(self, Self::Project { .. })
    }

    /// Check if this is a worktree item
    pub fn is_worktree(&self) -> bool {
        matches!(self, Self::Worktree { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_id_display() {
        let id = SessionId::new();
        let display = id.to_string();
        assert_eq!(display.len(), 8);
    }

    #[test]
    fn test_project_id_display() {
        let id = ProjectId::new();
        let display = id.to_string();
        assert_eq!(display.len(), 8);
    }

    #[test]
    fn test_session_status_transitions() {
        assert!(SessionStatus::Running.can_pause());
        assert!(!SessionStatus::Running.can_resume());
        assert!(SessionStatus::Paused.can_resume());
        assert!(!SessionStatus::Paused.can_pause());
        assert!(!SessionStatus::Stopped.can_attach());
    }

    #[test]
    fn test_project_worktree_management() {
        let mut project = Project::new("test", PathBuf::from("/tmp/test"), "main");
        let session_id = SessionId::new();

        project.add_worktree(session_id);
        assert_eq!(project.worktrees.len(), 1);

        // Adding same ID again should not duplicate
        project.add_worktree(session_id);
        assert_eq!(project.worktrees.len(), 1);

        project.remove_worktree(&session_id);
        assert!(project.worktrees.is_empty());
    }

    #[test]
    fn test_worktree_session_creation() {
        let project_id = ProjectId::new();
        let session = WorktreeSession::new(
            project_id,
            "Feature Auth",
            "feature-auth",
            PathBuf::from("/tmp/worktree"),
            "claude",
        );

        assert_eq!(session.project_id, project_id);
        assert_eq!(session.title, "Feature Auth");
        assert_eq!(session.branch, "feature-auth");
        assert_eq!(session.program, "claude");
        assert!(session.tmux_session_name.starts_with("cc-"));
        assert_eq!(session.status, SessionStatus::Running);
    }

    #[test]
    fn test_session_matches_query() {
        let session = WorktreeSession::new(
            ProjectId::new(),
            "Feature Authentication",
            "feature-auth",
            PathBuf::from("/tmp"),
            "claude",
        );

        assert!(session.matches_query("auth"));
        assert!(session.matches_query("AUTH")); // case insensitive
        assert!(session.matches_query("feature"));
        assert!(session.matches_query("claude"));
        assert!(!session.matches_query("unrelated"));
    }

    #[test]
    fn test_session_list_item_key() {
        let project_id = ProjectId::new();
        let session_id = SessionId::new();

        let project_item = SessionListItem::Project {
            id: project_id,
            name: "test".to_string(),
            repo_path: PathBuf::from("/tmp"),
            main_branch: "main".to_string(),
            worktree_count: 0,
        };

        let worktree_item = SessionListItem::Worktree {
            id: session_id,
            project_id,
            title: "test".to_string(),
            branch: "test".to_string(),
            status: SessionStatus::Running,
            agent_state: AgentState::WaitingForInput,
            program: "claude".to_string(),
        };

        assert!(project_item.key().starts_with("project:"));
        assert!(worktree_item.key().starts_with("worktree:"));
        assert!(project_item.is_project());
        assert!(worktree_item.is_worktree());
    }
}
