//! Agent state detection via tmux pane inspection
//!
//! Detects whether a Claude Code session is Working, Idle, or WaitingForInput
//! by inspecting the tmux pane title and visible pane content.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;
use tracing::debug;

/// Pre-compiled regex for stripping ANSI escape sequences.
static ANSI_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1B\[[0-9;]*[a-zA-Z]|\x1B\][^\x07]*\x07|\x1B\][^\x1B]*\x1B\\")
        .expect("valid regex")
});

use super::TmuxExecutor;
use crate::error::Result;
use crate::session::{AgentState, SessionId};

/// Detect agent state from the tmux pane title.
///
/// Claude Code sets the pane title to a braille spinner character (U+2800..U+28FF)
/// while working and `✳` (U+2733) when idle/waiting. Returns `Some(Working)` if a
/// braille spinner is found, `None` if we need to inspect content.
pub fn parse_pane_title(title: &str) -> Option<AgentState> {
    if title.contains(|c: char| ('\u{2800}'..='\u{28FF}').contains(&c)) {
        Some(AgentState::Working)
    } else {
        // Either contains ✳ (idle/waiting) or no known indicator — either way,
        // fall through to secondary content check.
        None
    }
}

/// Detect agent state from the visible pane content.
///
/// Checks the last non-empty lines for known Claude Code prompt patterns
/// that indicate the agent is waiting for user input.
pub fn parse_pane_content(content: &str) -> AgentState {
    let content = strip_ansi(content);

    // Check last 10 non-empty lines for prompt patterns
    let lines: Vec<&str> = content
        .lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .take(10)
        .collect();

    for line in &lines {
        // Permission prompt: "Esc to cancel"
        if line.contains("Esc to cancel") {
            return AgentState::WaitingForInput;
        }

        // Rejection menu option
        if line.contains("No, and tell Claude what to do differently") {
            return AgentState::WaitingForInput;
        }

        // Selection menu: ❯ followed by a digit
        if let Some(pos) = line.find('\u{276F}') {
            let after = line[pos + '\u{276F}'.len_utf8()..].trim_start();
            if after.starts_with(|c: char| c.is_ascii_digit()) {
                return AgentState::WaitingForInput;
            }
        }
    }

    AgentState::Idle
}

/// Strip ANSI escape sequences from a string.
pub fn strip_ansi(s: &str) -> String {
    ANSI_RE.replace_all(s, "").into_owned()
}

/// Agent state detector that polls tmux sessions and caches results.
pub struct AgentStateDetector {
    executor: TmuxExecutor,
    cache: HashMap<String, (AgentState, Instant)>,
    cache_ttl: Duration,
}

impl AgentStateDetector {
    /// Create a new detector with the given cache TTL.
    pub fn new(executor: TmuxExecutor, cache_ttl: Duration) -> Self {
        Self {
            executor,
            cache: HashMap::new(),
            cache_ttl,
        }
    }

    /// Detect agent state for a single tmux session, using cache if fresh.
    pub async fn detect(&mut self, tmux_session_name: &str) -> AgentState {
        // Check cache
        if let Some((state, cached_at)) = self.cache.get(tmux_session_name)
            && cached_at.elapsed() < self.cache_ttl
        {
            return *state;
        }

        let state = self.detect_fresh(tmux_session_name).await;

        self.cache
            .insert(tmux_session_name.to_string(), (state, Instant::now()));

        state
    }

    /// Perform fresh detection bypassing cache.
    async fn detect_fresh(&self, tmux_session_name: &str) -> AgentState {
        // Primary: check pane title
        match self.get_pane_title(tmux_session_name).await {
            Ok(title) => {
                if let Some(state) = parse_pane_title(&title) {
                    debug!(
                        "Pane title detection for {}: {:?}",
                        tmux_session_name, state
                    );
                    return state;
                }
                // Title says "not working", check content
            }
            Err(e) => {
                debug!("Failed to get pane title for {}: {}", tmux_session_name, e);
                return AgentState::Unknown;
            }
        }

        // Secondary: parse visible pane content
        match self.capture_visible_pane(tmux_session_name).await {
            Ok(content) => {
                let state = parse_pane_content(&content);
                debug!(
                    "Pane content detection for {}: {:?}",
                    tmux_session_name, state
                );
                state
            }
            Err(e) => {
                debug!("Failed to capture pane for {}: {}", tmux_session_name, e);
                AgentState::Unknown
            }
        }
    }

    /// Get the tmux pane title.
    async fn get_pane_title(&self, tmux_session_name: &str) -> Result<String> {
        self.executor
            .execute(&[
                "display-message",
                "-t",
                tmux_session_name,
                "-p",
                "#{pane_title}",
            ])
            .await
    }

    /// Capture visible pane content (no scrollback, no ANSI escapes).
    async fn capture_visible_pane(&self, tmux_session_name: &str) -> Result<String> {
        self.executor
            .execute(&["capture-pane", "-t", tmux_session_name, "-p"])
            .await
    }

    /// Detect agent states for a batch of sessions.
    ///
    /// Filters to Claude programs only; non-Claude sessions get `Unknown`.
    pub async fn detect_all(
        &mut self,
        sessions: &[(SessionId, String, String)],
    ) -> HashMap<SessionId, AgentState> {
        let mut results = HashMap::new();

        for (session_id, tmux_name, program) in sessions {
            let state = if program.eq_ignore_ascii_case("claude") {
                self.detect(tmux_name).await
            } else {
                AgentState::Unknown
            };
            results.insert(*session_id, state);
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- parse_pane_title tests --

    #[test]
    fn test_pane_title_working_braille_spinner() {
        // Braille dot pattern U+2802 indicates Claude is working
        assert_eq!(
            parse_pane_title("⠂ feature-branch"),
            Some(AgentState::Working)
        );
    }

    #[test]
    fn test_pane_title_working_braille_spinner_alt() {
        // Braille dot pattern U+2810 — another spinner frame
        assert_eq!(
            parse_pane_title("⠐ feature-branch"),
            Some(AgentState::Working)
        );
    }

    #[test]
    fn test_pane_title_not_working() {
        // Eight-spoked asterisk indicates idle/waiting — needs content check
        assert_eq!(parse_pane_title("✳ Claude Code"), None);
    }

    #[test]
    fn test_pane_title_empty() {
        assert_eq!(parse_pane_title(""), None);
    }

    #[test]
    fn test_pane_title_no_indicator() {
        assert_eq!(parse_pane_title("some random title"), None);
    }

    // -- parse_pane_content tests --

    #[test]
    fn test_content_esc_to_cancel() {
        let content = "Some output\n  Allow tool? Esc to cancel\n";
        assert_eq!(parse_pane_content(content), AgentState::WaitingForInput);
    }

    #[test]
    fn test_content_rejection_menu() {
        let content = "Result\nNo, and tell Claude what to do differently\n";
        assert_eq!(parse_pane_content(content), AgentState::WaitingForInput);
    }

    #[test]
    fn test_content_selection_menu() {
        let content = "Choose:\n❯ 1. Allow once\n  2. Allow always\n";
        assert_eq!(parse_pane_content(content), AgentState::WaitingForInput);
    }

    #[test]
    fn test_content_selection_menu_with_space() {
        let content = "Choose:\n❯  3. Deny\n";
        assert_eq!(parse_pane_content(content), AgentState::WaitingForInput);
    }

    #[test]
    fn test_content_idle() {
        let content = "Done editing files.\n\n❯ \n";
        // ❯ not followed by a digit = idle prompt, not selection menu
        assert_eq!(parse_pane_content(content), AgentState::Idle);
    }

    #[test]
    fn test_content_empty() {
        assert_eq!(parse_pane_content(""), AgentState::Idle);
    }

    #[test]
    fn test_content_no_patterns() {
        let content = "Building project...\nCompiling main.rs\nFinished in 2.3s\n";
        assert_eq!(parse_pane_content(content), AgentState::Idle);
    }

    // -- strip_ansi tests --

    #[test]
    fn test_strip_ansi_clean_string() {
        assert_eq!(strip_ansi("hello world"), "hello world");
    }

    #[test]
    fn test_strip_ansi_csi_sequences() {
        assert_eq!(strip_ansi("\x1B[31mred\x1B[0m text"), "red text");
    }

    #[test]
    fn test_strip_ansi_preserves_content() {
        let input = "\x1B[1;32m● working\x1B[0m Esc to cancel";
        let stripped = strip_ansi(input);
        assert!(stripped.contains("Esc to cancel"));
        assert!(!stripped.contains("\x1B"));
    }

    #[test]
    fn test_content_with_ansi_escapes() {
        let content = "\x1B[1mAllow?\x1B[0m \x1B[33mEsc to cancel\x1B[0m\n";
        assert_eq!(parse_pane_content(content), AgentState::WaitingForInput);
    }

    // -- cache tests --

    #[test]
    fn test_cache_ttl_fresh() {
        let entry = (AgentState::Working, Instant::now());
        assert!(entry.1.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn test_cache_ttl_stale() {
        let entry = (
            AgentState::Working,
            Instant::now() - Duration::from_secs(10),
        );
        assert!(entry.1.elapsed() > Duration::from_secs(5));
    }

    // -- detect_all filtering --

    #[tokio::test]
    async fn test_detect_all_filters_non_claude() {
        // We can't make real tmux calls in tests, but we can test that
        // non-Claude programs get Unknown without attempting detection.
        // This test verifies the filtering logic by checking that aider
        // sessions produce Unknown.
        let executor = TmuxExecutor::new();
        let mut detector = AgentStateDetector::new(executor, Duration::from_secs(10));

        let sessions = vec![(
            SessionId::new(),
            "non-existent-session".to_string(),
            "aider".to_string(),
        )];

        let results = detector.detect_all(&sessions).await;
        assert_eq!(results.len(), 1);
        for state in results.values() {
            assert_eq!(*state, AgentState::Unknown);
        }
    }
}
