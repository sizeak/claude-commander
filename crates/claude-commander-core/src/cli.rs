//! CLI helper utilities shared across subcommands.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::config::AppState;
use crate::git::{PrState, ReviewDecision, effective_pr_state};
use crate::session::{AgentState, WorktreeSession};

/// Whether `query` identifies `session`: a full-UUID match, an 8-char
/// display-prefix match, or (handled by callers) a title match.
///
/// `SessionId`'s `Display` is an 8-char prefix of the UUID, but the HTTP API
/// hands clients the full 36-char UUID. Matching on either keeps both the
/// CLI/TUI (which show the prefix) and API clients (which echo the full id)
/// working through the same resolution path.
fn id_matches(session: &WorktreeSession, query: &str) -> bool {
    // Full UUID is exact and unambiguous; the 8-char display is a prefix match.
    session.id.as_uuid().to_string() == query || session.id.to_string().starts_with(query)
}

/// Find a session by title (case-insensitive), full ID, or ID prefix.
///
/// Title match takes priority: if a session's title matches exactly
/// (case-insensitive), it is returned even if another session's ID
/// happens to start with the query string. The ID fallback accepts either the
/// full UUID (as returned by the HTTP API) or the 8-char display prefix (as
/// shown in the CLI/TUI).
pub fn find_session<'a>(state: &'a AppState, query: &str) -> Option<&'a WorktreeSession> {
    let query_lower = query.to_lowercase();

    // Prefer exact title match (case-insensitive)
    let by_title = state
        .sessions
        .values()
        .find(|s| s.title.to_lowercase() == query_lower);

    if by_title.is_some() {
        return by_title;
    }

    // Fall back to ID match (full UUID or display prefix)
    state.sessions.values().find(|s| id_matches(s, query))
}

/// Outcome of resolving a session by an *exact* identifier.
#[derive(Debug, PartialEq, Eq)]
pub enum SessionLookup<T> {
    /// Exactly one session matched.
    Found(T),
    /// No session matched the query.
    NotFound,
    /// More than one session matched (the count of matches).
    Ambiguous(usize),
}

/// Resolve a session by an *exact* identifier: a case-insensitive exact title
/// match or a full session-ID match.
///
/// Unlike [`find_session`], this performs no prefix matching, so a destructive
/// command can never act on the wrong session merely because the query was a
/// prefix shared by several IDs (or an empty string, which prefixes every ID).
/// Returns [`SessionLookup::Ambiguous`] when more than one session matches
/// (e.g. two sessions share a title) rather than picking one arbitrarily.
pub fn find_session_exact<'a>(
    state: &'a AppState,
    query: &str,
) -> SessionLookup<&'a WorktreeSession> {
    let query_lower = query.to_lowercase();
    let mut matches = state
        .sessions
        .values()
        .filter(|s| s.title.to_lowercase() == query_lower || s.id.as_uuid().to_string() == query);

    let Some(first) = matches.next() else {
        return SessionLookup::NotFound;
    };
    match matches.count() {
        0 => SessionLookup::Found(first),
        extra => SessionLookup::Ambiguous(extra + 1),
    }
}

/// Resolve a `--project <name>` flag to the project's on-disk repo path using a
/// backend's [`WorkspaceSnapshot`](crate::api::WorkspaceSnapshot). Matches a
/// project by name (case-insensitive) and returns its `repo_path` — the path
/// the session's worktree will fork from. For a remote backend this is the
/// server-side path, so the caller never has to know it.
///
/// Returns a [`ConfigError::InvalidValue`](crate::error::ConfigError) listing
/// the available project names when no project matches, so a `--project` typo is
/// as actionable as an unknown `--remote` server. An empty project list reports
/// "none found" (e.g. a fresh remote with no sessions yet — seed one with
/// `--path`).
pub fn resolve_project_path(
    projects: &[crate::api::ProjectInfo],
    name: &str,
) -> crate::Result<std::path::PathBuf> {
    projects
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .map(|p| p.repo_path.clone())
        .ok_or_else(|| {
            let available = if projects.is_empty() {
                "none found".to_string()
            } else {
                projects
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            crate::error::ConfigError::InvalidValue {
                key: "project".to_string(),
                reason: format!("no project named '{name}' (available: {available})"),
            }
            .into()
        })
}

/// Maximum lines allowed for the `log` command's `--lines` flag.
pub const LOG_MAX_LINES: usize = 10_000;

/// Clamp a requested line count to the allowed range [1, LOG_MAX_LINES].
pub fn clamp_log_lines(requested: usize) -> usize {
    requested.clamp(1, LOG_MAX_LINES)
}

/// What the `delete` command should do before mutating, given the `--force`
/// flag and whether stdin is an interactive terminal.
#[derive(Debug, PartialEq, Eq)]
pub enum DeleteGuard {
    /// `--force` was given: delete without asking.
    Proceed,
    /// Interactive terminal: prompt the user for confirmation.
    Prompt,
    /// Non-interactive stdin without `--force`: refuse to delete.
    RequireForce,
}

/// Decide how the `delete` command should guard against accidental deletion.
pub fn delete_guard(force: bool, stdin_is_tty: bool) -> DeleteGuard {
    if force {
        DeleteGuard::Proceed
    } else if stdin_is_tty {
        DeleteGuard::Prompt
    } else {
        DeleteGuard::RequireForce
    }
}

/// Parse a yes/no confirmation answer. Only `y`/`yes` (case-insensitive,
/// surrounding whitespace ignored) count as yes; everything else is no.
pub fn parse_yes_no(input: &str) -> bool {
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

/// JSON-serializable session entry for CLI output.
#[derive(Debug, Serialize)]
pub struct SessionJsonEntry {
    pub id: String,
    pub title: String,
    pub branch: String,
    pub status: String,
    pub program: String,
    pub project_name: String,
    pub pr_number: Option<u32>,
    pub pr_url: Option<String>,
    /// Resolved PR state, accounting for legacy `pr_merged` field.
    /// Only meaningful when `pr_number` is `Some`.
    pub pr_state: PrState,
    pub pr_draft: bool,
    pub pr_labels: Vec<String>,
    pub created_at: DateTime<Utc>,
}

impl SessionJsonEntry {
    pub fn from_info(info: &crate::api::SessionInfo) -> Self {
        Self {
            id: info.id.clone(),
            title: info.title.clone(),
            branch: info.branch.clone(),
            status: info.status.to_string(),
            program: info.program.clone(),
            project_name: info.project_name.clone(),
            pr_number: info.pr_number,
            pr_url: info.pr_url.clone(),
            pr_state: info.pr_state,
            pr_draft: info.pr_draft,
            pr_labels: info.pr_labels.clone(),
            created_at: info.created_at,
        }
    }

    pub fn from_session(session: &WorktreeSession, project_name: &str) -> Self {
        Self {
            id: session.id.as_uuid().to_string(),
            title: session.title.clone(),
            branch: session.branch.clone(),
            status: session.status.to_string(),
            program: session.program.clone(),
            project_name: project_name.to_string(),
            pr_number: session.pr_number,
            pr_url: session.pr_url.clone(),
            pr_state: effective_pr_state(session.pr_state, session.pr_merged),
            pr_draft: session.pr_draft,
            pr_labels: session.pr_labels.clone(),
            created_at: session.created_at,
        }
    }
}

/// Build a JSON-serializable list of sessions, optionally including stopped ones.
pub fn build_session_list(state: &AppState, include_stopped: bool) -> Vec<SessionJsonEntry> {
    let mut entries = Vec::new();
    for project in state.projects.values() {
        for session in project
            .worktrees
            .iter()
            .filter_map(|id| state.sessions.get(id))
            .filter(|s| include_stopped || s.status.is_active())
        {
            entries.push(SessionJsonEntry::from_session(session, &project.name));
        }
    }
    entries
}

/// JSON-serializable session detail for the `status` subcommand.
#[derive(Debug, Serialize)]
pub struct StatusJsonEntry {
    pub id: String,
    pub title: String,
    pub branch: String,
    pub status: String,
    pub program: String,
    pub project_name: String,
    pub agent_state: String,
    pub diff_stat: Option<String>,
    pub pr_number: Option<u32>,
    pub pr_url: Option<String>,
    pub pr_state: PrState,
    pub pr_draft: bool,
    pub pr_labels: Vec<String>,
    pub review_decision: Option<ReviewDecision>,
    pub pr_reviewers: Vec<String>,
    pub created_at: DateTime<Utc>,
}

impl StatusJsonEntry {
    pub fn from_detail(detail: &crate::api::SessionDetail) -> Self {
        let info = &detail.info;
        Self {
            id: info.id.clone(),
            title: info.title.clone(),
            branch: info.branch.clone(),
            status: info.status.to_string(),
            program: info.program.clone(),
            project_name: info.project_name.clone(),
            agent_state: detail.agent_state.to_string(),
            diff_stat: detail.diff_stat.clone(),
            pr_number: info.pr_number,
            pr_url: info.pr_url.clone(),
            pr_state: info.pr_state,
            pr_draft: info.pr_draft,
            pr_labels: info.pr_labels.clone(),
            review_decision: info.review_decision,
            pr_reviewers: info.pr_reviewers.clone(),
            created_at: info.created_at,
        }
    }

    pub fn from_session(
        session: &WorktreeSession,
        project_name: &str,
        agent_state: AgentState,
        diff_stat: Option<String>,
    ) -> Self {
        Self {
            id: session.id.as_uuid().to_string(),
            title: session.title.clone(),
            branch: session.branch.clone(),
            status: session.status.to_string(),
            program: session.program.clone(),
            project_name: project_name.to_string(),
            agent_state: agent_state.to_string(),
            diff_stat,
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

/// Format a human-readable status summary for a session.
pub fn format_status_human(entry: &StatusJsonEntry) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Session: {} ({})", entry.title, entry.id));
    lines.push(format!("Branch:  {}", entry.branch));
    lines.push(format!(
        "Status:  {} | Agent: {}",
        entry.status, entry.agent_state
    ));
    lines.push(format!(
        "Program: {} | Project: {}",
        entry.program, entry.project_name
    ));

    if let Some(ref stat) = entry.diff_stat {
        lines.push(format!("Diff:    {}", stat.trim()));
    }

    if let Some(pr) = entry.pr_number {
        let url = entry.pr_url.as_deref().unwrap_or("(no url)");
        lines.push(format!(
            "PR:      #{} ({}, {}) {}",
            pr,
            entry.pr_state,
            if entry.pr_draft { "draft" } else { "ready" },
            url,
        ));
        if let Some(decision) = entry.review_decision {
            lines.push(format!("Review:  {}", decision));
        }
        if !entry.pr_reviewers.is_empty() {
            lines.push(format!("Reviewers: {}", entry.pr_reviewers.join(", ")));
        }
        if !entry.pr_labels.is_empty() {
            lines.push(format!("Labels:  {}", entry.pr_labels.join(", ")));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Project, ProjectId, SessionStatus, WorktreeSession};
    use std::path::PathBuf;

    fn make_project(name: &str) -> Project {
        Project::new(name, PathBuf::from("/tmp/repo"), "main")
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

    fn make_state(sessions: Vec<WorktreeSession>) -> AppState {
        let mut state = AppState::new();
        for s in sessions {
            state.sessions.insert(s.id, s);
        }
        state
    }

    fn make_session(title: &str) -> WorktreeSession {
        WorktreeSession::new(
            ProjectId::new(),
            title,
            format!("branch-{}", title),
            PathBuf::from("/tmp/wt"),
            "claude",
        )
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

    #[test]
    fn finds_by_exact_title() {
        let s = make_session("fix-auth");
        let state = make_state(vec![s.clone()]);
        let found = find_session(&state, "fix-auth").unwrap();
        assert_eq!(found.id, s.id);
    }

    #[test]
    fn finds_by_title_case_insensitive() {
        let s = make_session("Fix-Auth");
        let state = make_state(vec![s.clone()]);
        let found = find_session(&state, "fix-auth").unwrap();
        assert_eq!(found.id, s.id);
    }

    #[test]
    fn finds_by_id_prefix() {
        let s = make_session("my-session");
        let id_prefix = &s.id.to_string()[..4];
        let state = make_state(vec![s.clone()]);
        let found = find_session(&state, id_prefix).unwrap();
        assert_eq!(found.id, s.id);
    }

    #[test]
    fn finds_by_full_uuid() {
        // The HTTP API hands clients the full 36-char UUID, not the 8-char
        // display. `find_session` must resolve it (B1 regression).
        let s = make_session("my-session");
        let full_uuid = s.id.as_uuid().to_string();
        assert!(
            full_uuid.len() > 8,
            "full uuid should be longer than display"
        );
        let state = make_state(vec![s.clone()]);
        let found = find_session(&state, &full_uuid).unwrap();
        assert_eq!(found.id, s.id);
    }

    #[test]
    fn returns_none_when_no_match() {
        let state = make_state(vec![make_session("something")]);
        assert!(find_session(&state, "nonexistent").is_none());
    }

    #[test]
    fn title_match_takes_priority_over_id_prefix() {
        // Create two sessions where one's title could collide with the
        // other's ID prefix in theory. The title match should always win.
        let s1 = make_session("abc");
        let s2 = make_session("other");
        let state = make_state(vec![s1.clone(), s2]);
        let found = find_session(&state, "abc").unwrap();
        assert_eq!(found.id, s1.id);
    }

    #[test]
    fn returns_none_on_empty_state() {
        let state = AppState::new();
        assert!(find_session(&state, "anything").is_none());
    }

    // -- find_session_exact tests --

    #[test]
    fn exact_matches_full_title_case_insensitive() {
        let s = make_session("Fix-Auth");
        let state = make_state(vec![s.clone()]);
        match find_session_exact(&state, "fix-auth") {
            SessionLookup::Found(found) => assert_eq!(found.id, s.id),
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[test]
    fn exact_matches_full_id() {
        let s = make_session("my-session");
        // The full 36-char UUID, as the HTTP API returns it — not the 8-char
        // `Display` prefix.
        let full_id = s.id.as_uuid().to_string();
        let state = make_state(vec![s.clone()]);
        match find_session_exact(&state, &full_id) {
            SessionLookup::Found(found) => assert_eq!(found.id, s.id),
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[test]
    fn exact_does_not_match_id_prefix() {
        // The dangerous case the loose `find_session` allowed: a prefix of an
        // ID must NOT resolve for a destructive command.
        let s = make_session("my-session");
        let id_prefix = &s.id.to_string()[..4];
        let state = make_state(vec![s]);
        assert!(matches!(
            find_session_exact(&state, id_prefix),
            SessionLookup::NotFound
        ));
    }

    #[test]
    fn exact_empty_query_is_not_found() {
        // An empty string is a prefix of every ID; it must never resolve.
        let state = make_state(vec![make_session("a"), make_session("b")]);
        assert!(matches!(
            find_session_exact(&state, ""),
            SessionLookup::NotFound
        ));
    }

    #[test]
    fn exact_reports_ambiguity_on_duplicate_titles() {
        let state = make_state(vec![make_session("dup"), make_session("dup")]);
        assert!(matches!(
            find_session_exact(&state, "dup"),
            SessionLookup::Ambiguous(2)
        ));
    }

    #[test]
    fn exact_returns_not_found_when_no_match() {
        let state = make_state(vec![make_session("something")]);
        assert!(matches!(
            find_session_exact(&state, "nonexistent"),
            SessionLookup::NotFound
        ));
    }

    // -- clamp_log_lines tests --

    #[test]
    fn clamp_log_lines_default_passthrough() {
        assert_eq!(clamp_log_lines(100), 100);
    }

    #[test]
    fn clamp_log_lines_zero_becomes_one() {
        assert_eq!(clamp_log_lines(0), 1);
    }

    #[test]
    fn clamp_log_lines_max_boundary() {
        assert_eq!(clamp_log_lines(LOG_MAX_LINES), LOG_MAX_LINES);
    }

    #[test]
    fn clamp_log_lines_over_max() {
        assert_eq!(clamp_log_lines(LOG_MAX_LINES + 1), LOG_MAX_LINES);
        assert_eq!(clamp_log_lines(usize::MAX), LOG_MAX_LINES);
    }

    // -- SessionJsonEntry tests --

    #[test]
    fn json_entry_has_expected_fields() {
        let session = make_session("fix-bug");
        let entry = SessionJsonEntry::from_session(&session, "my-project");
        let json: serde_json::Value = serde_json::to_value(&entry).unwrap();

        assert_eq!(json["title"], "fix-bug");
        assert_eq!(json["branch"], "branch-fix-bug");
        assert_eq!(json["program"], "claude");
        assert_eq!(json["project_name"], "my-project");
        assert_eq!(json["pr_draft"], false);
        assert!(json["pr_number"].is_null());
        assert!(json["pr_url"].is_null());
        // No PR set + pr_merged=false → defaults to "open" via effective_pr_state
        assert_eq!(json["pr_state"], "open");
        assert!(json["pr_labels"].as_array().unwrap().is_empty());
        assert!(json["created_at"].is_string());
        assert!(json["id"].is_string());
        assert!(json["status"].is_string());
    }

    #[test]
    fn json_entry_includes_pr_fields_when_set() {
        let mut session = make_session("with-pr");
        session.pr_number = Some(42);
        session.pr_url = Some("https://github.com/org/repo/pull/42".to_string());
        session.pr_state = Some(crate::git::PrState::Open);
        session.pr_draft = true;
        session.pr_labels = vec!["bug".to_string(), "urgent".to_string()];

        let entry = SessionJsonEntry::from_session(&session, "proj");
        let json: serde_json::Value = serde_json::to_value(&entry).unwrap();

        assert_eq!(json["pr_number"], 42);
        assert_eq!(json["pr_url"], "https://github.com/org/repo/pull/42");
        assert_eq!(json["pr_state"], "open");
        assert_eq!(json["pr_draft"], true);
        assert_eq!(json["pr_labels"], serde_json::json!(["bug", "urgent"]));
    }

    #[test]
    fn json_entry_resolves_legacy_pr_merged() {
        let mut session = make_session("legacy");
        session.pr_number = Some(10);
        session.pr_state = None;
        session.pr_merged = true;

        let entry = SessionJsonEntry::from_session(&session, "proj");
        let json: serde_json::Value = serde_json::to_value(&entry).unwrap();

        assert_eq!(json["pr_state"], "merged");
    }

    #[test]
    fn json_entry_id_is_full_uuid() {
        let session = make_session("test");
        let entry = SessionJsonEntry::from_session(&session, "proj");
        // SessionId::Display truncates to 8 chars, but JSON should have the full UUID
        assert!(entry.id.len() > 8);
        assert!(uuid::Uuid::parse_str(&entry.id).is_ok());
    }

    // -- build_session_list tests --

    #[test]
    fn build_list_excludes_stopped_by_default() {
        let project = make_project("repo");
        let mut s1 = make_session_for_project("running", project.id);
        s1.set_status(SessionStatus::Running);
        let mut s2 = make_session_for_project("stopped", project.id);
        s2.set_status(SessionStatus::Stopped);

        let state = make_state_with_project(&project, vec![s1, s2]);
        let list = build_session_list(&state, false);

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
        let list = build_session_list(&state, true);

        assert_eq!(list.len(), 2);
    }

    #[test]
    fn build_list_populates_project_name() {
        let project = make_project("my-repo");
        let s = make_session_for_project("task", project.id);
        let state = make_state_with_project(&project, vec![s]);
        let list = build_session_list(&state, false);

        assert_eq!(list.len(), 1);
        assert_eq!(list[0].project_name, "my-repo");
    }

    #[test]
    fn build_list_empty_when_no_projects() {
        let state = AppState::new();
        let list = build_session_list(&state, true);
        assert!(list.is_empty());
    }

    // -- StatusJsonEntry tests --

    #[test]
    fn status_entry_has_expected_fields() {
        let session = make_session("fix-bug");
        let entry = StatusJsonEntry::from_session(
            &session,
            "proj",
            AgentState::Working,
            Some("3 files changed".to_string()),
        );
        let json: serde_json::Value = serde_json::to_value(&entry).unwrap();

        assert_eq!(json["title"], "fix-bug");
        assert_eq!(json["agent_state"], "working");
        assert_eq!(json["diff_stat"], "3 files changed");
        assert_eq!(json["project_name"], "proj");
        assert!(json["pr_reviewers"].as_array().unwrap().is_empty());
        assert!(json["review_decision"].is_null());
    }

    #[test]
    fn status_entry_with_pr_and_reviews() {
        let mut session = make_session("reviewed");
        session.pr_number = Some(5);
        session.pr_url = Some("https://github.com/org/repo/pull/5".to_string());
        session.pr_state = Some(crate::git::PrState::Open);
        session.review_decision = Some(ReviewDecision::Approved);
        session.pr_reviewers = vec!["alice".to_string(), "bob".to_string()];

        let entry = StatusJsonEntry::from_session(&session, "proj", AgentState::Idle, None);
        let json: serde_json::Value = serde_json::to_value(&entry).unwrap();

        assert_eq!(json["pr_number"], 5);
        assert_eq!(json["review_decision"], "approved");
        assert_eq!(json["pr_reviewers"], serde_json::json!(["alice", "bob"]));
        assert!(json["diff_stat"].is_null());
    }

    #[test]
    fn status_entry_agent_state_variants() {
        let session = make_session("test");
        for (state, expected) in [
            (AgentState::Working, "working"),
            (AgentState::Idle, "idle"),
            (AgentState::WaitingForInput, "waiting"),
            (AgentState::Unknown, "unknown"),
        ] {
            let entry = StatusJsonEntry::from_session(&session, "p", state, None);
            assert_eq!(entry.agent_state, expected);
        }
    }

    // -- format_status_human tests --

    #[test]
    fn human_format_includes_basic_info() {
        let session = make_session("my-task");
        let entry = StatusJsonEntry::from_session(
            &session,
            "my-project",
            AgentState::Working,
            Some("2 files changed, 10 insertions(+)".to_string()),
        );
        let output = format_status_human(&entry);

        assert!(output.contains("my-task"));
        assert!(output.contains("my-project"));
        assert!(output.contains("working"));
        assert!(output.contains("2 files changed"));
    }

    #[test]
    fn human_format_shows_pr_when_present() {
        let mut session = make_session("with-pr");
        session.pr_number = Some(42);
        session.pr_url = Some("https://example.com/pull/42".to_string());
        session.pr_state = Some(crate::git::PrState::Open);
        session.pr_labels = vec!["bug".to_string()];
        session.pr_reviewers = vec!["alice".to_string()];
        session.review_decision = Some(ReviewDecision::ChangesRequested);

        let entry = StatusJsonEntry::from_session(&session, "proj", AgentState::Idle, None);
        let output = format_status_human(&entry);

        assert!(output.contains("#42"));
        assert!(output.contains("https://example.com/pull/42"));
        assert!(output.contains("bug"));
        assert!(output.contains("alice"));
        assert!(output.contains("Changes requested"));
    }

    #[test]
    fn human_format_omits_pr_when_absent() {
        let session = make_session("no-pr");
        let entry = StatusJsonEntry::from_session(&session, "proj", AgentState::Idle, None);
        let output = format_status_human(&entry);

        assert!(!output.contains("PR:"));
        assert!(!output.contains("Review:"));
        assert!(!output.contains("Labels:"));
    }

    #[test]
    fn delete_guard_force_always_proceeds() {
        assert_eq!(delete_guard(true, true), DeleteGuard::Proceed);
        assert_eq!(delete_guard(true, false), DeleteGuard::Proceed);
    }

    #[test]
    fn delete_guard_interactive_prompts() {
        assert_eq!(delete_guard(false, true), DeleteGuard::Prompt);
    }

    #[test]
    fn delete_guard_non_tty_requires_force() {
        assert_eq!(delete_guard(false, false), DeleteGuard::RequireForce);
    }

    #[test]
    fn parse_yes_no_accepts_yes_variants() {
        for input in ["y", "Y", "yes", "YES", " yes\n", "  y  "] {
            assert!(parse_yes_no(input), "expected {input:?} to be yes");
        }
    }

    #[test]
    fn parse_yes_no_rejects_everything_else() {
        for input in ["", "n", "no", "x", "yep", "\n"] {
            assert!(!parse_yes_no(input), "expected {input:?} to be no");
        }
    }

    fn make_project_info(name: &str, repo_path: &str) -> crate::api::ProjectInfo {
        crate::api::ProjectInfo {
            id: ProjectId::new(),
            name: name.to_string(),
            repo_path: PathBuf::from(repo_path),
            main_branch: "main".to_string(),
            session_ids: Vec::new(),
        }
    }

    #[test]
    fn resolve_project_path_matches_case_insensitively() {
        let projects = vec![
            make_project_info("Genio", "/home/mark/genio"),
            make_project_info("other", "/home/mark/other"),
        ];
        let path = resolve_project_path(&projects, "genio").unwrap();
        assert_eq!(path, PathBuf::from("/home/mark/genio"));
    }

    #[test]
    fn resolve_project_path_unknown_lists_available() {
        let projects = vec![make_project_info("genio", "/home/mark/genio")];
        let err = resolve_project_path(&projects, "nope").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("no project named 'nope'") && msg.contains("genio"),
            "unknown project must name the miss and list projects: {err}"
        );
    }

    #[test]
    fn resolve_project_path_empty_reports_none_found() {
        let err = resolve_project_path(&[], "genio").unwrap_err();
        assert!(
            err.to_string().contains("none found"),
            "with no projects the error must say none were found: {err}"
        );
    }
}
