//! CLI helper utilities shared across subcommands.

use crate::config::AppState;
use crate::session::WorktreeSession;

/// Find a session by title (case-insensitive) or ID prefix.
///
/// Title match takes priority: if a session's title matches exactly
/// (case-insensitive), it is returned even if another session's ID
/// happens to start with the query string.
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

    // Fall back to ID prefix match
    state
        .sessions
        .values()
        .find(|s| s.id.to_string().starts_with(query))
}

/// Maximum lines allowed for the `log` command's `--lines` flag.
pub const LOG_MAX_LINES: usize = 10_000;

/// Clamp a requested line count to the allowed range [1, LOG_MAX_LINES].
pub fn clamp_log_lines(requested: usize) -> usize {
    requested.clamp(1, LOG_MAX_LINES)
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
            &format!("branch-{}", title),
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
}
