//! Session and project lookup by user-supplied identifier.
//!
//! Shared resolution logic used by the service layer ([`crate::api`]), the CLI
//! and the HTTP server — every frontend resolves a session query through the
//! same matching rules so an ID prefix, a full UUID or a title behaves
//! identically whichever surface the user reaches for.

use crate::config::AppState;
use crate::session::WorktreeSession;

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
/// Both failures return a [`ConfigError::InvalidValue`](crate::error::ConfigError):
/// - **No match**: lists the available project names (an empty list reports
///   "none found" — e.g. a fresh remote with no sessions yet; seed one with
///   `--path`), so a typo is as actionable as an unknown `--remote` server.
/// - **Ambiguous match**: project names are derived from repo directory names
///   and are *not* unique, so two projects can share one. Rather than silently
///   pick the first, report the collision and direct the caller to `--path`
///   (which names an exact directory).
pub fn resolve_project_path(
    projects: &[crate::api::ProjectInfo],
    name: &str,
) -> crate::Result<std::path::PathBuf> {
    let mut matches = projects
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case(name));

    let Some(first) = matches.next() else {
        let available = if projects.is_empty() {
            "none found".to_string()
        } else {
            projects
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Err(crate::error::ConfigError::InvalidValue {
            key: "project".to_string(),
            reason: format!("no project named '{name}' (available: {available})"),
        }
        .into());
    };

    if matches.next().is_some() {
        return Err(crate::error::ConfigError::InvalidValue {
            key: "project".to_string(),
            reason: format!(
                "'{name}' matches more than one project — disambiguate with --path <repo path>"
            ),
        }
        .into());
    }

    Ok(first.repo_path.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{ProjectId, WorktreeSession};
    use std::path::PathBuf;

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

    // -- resolve_project_path tests --

    fn make_project_info(name: &str, repo_path: &str) -> crate::api::ProjectInfo {
        crate::api::ProjectInfo {
            id: ProjectId::new(),
            name: name.to_string(),
            repo_path: PathBuf::from(repo_path),
            main_branch: "main".to_string(),
            session_ids: Vec::new(),
            origin_url: None,
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

    #[test]
    fn resolve_project_path_ambiguous_errors_with_path_hint() {
        // Project names aren't unique (they come from repo dir names), so two
        // projects named "app" at different paths must not silently pick one.
        let projects = vec![
            make_project_info("app", "/home/mark/one/app"),
            make_project_info("App", "/home/mark/two/app"),
        ];
        let err = resolve_project_path(&projects, "app").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("more than one project") && msg.contains("--path"),
            "ambiguous project must error and point at --path: {err}"
        );
    }
}
