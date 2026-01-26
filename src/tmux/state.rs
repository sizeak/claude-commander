//! Agent state detection from pane content
//!
//! Detects the current state of an AI agent by analyzing pane content:
//! - Prompt patterns (waiting for input)
//! - Activity indicators (processing)
//! - Error patterns

use regex::Regex;
use std::sync::LazyLock;
use tracing::debug;

use super::CapturedContent;
use crate::session::AgentState;

/// Patterns for detecting prompt (waiting for input)
static PROMPT_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // Claude Code patterns
        Regex::new(r"(?m)^>\s*$").unwrap(),
        Regex::new(r"(?m)^claude>\s*$").unwrap(),
        Regex::new(r"(?m)^\$ $").unwrap(),
        // Aider patterns
        Regex::new(r"(?m)^aider>\s*$").unwrap(),
        Regex::new(r"(?m)^───.*───\s*$").unwrap(),
        // Generic shell prompt patterns
        Regex::new(r"(?m)^[^>\n]*>\s*$").unwrap(),
        Regex::new(r"(?m)^[^$\n]*\$\s*$").unwrap(),
    ]
});

/// Patterns for detecting processing/activity
static PROCESSING_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // Spinner characters
        Regex::new(r"[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]").unwrap(),
        // Loading indicators
        Regex::new(r"(?i)(thinking|processing|running|loading)\.{1,3}").unwrap(),
        // Progress bars
        Regex::new(r"\[=+>?\s*\]").unwrap(),
        Regex::new(r"\[#+\s*\]").unwrap(),
        // Token streaming (partial lines without newline at end)
        Regex::new(r"(?m)^[^$>\n]{10,}[^\n\s]$").unwrap(),
    ]
});

/// Patterns for detecting errors
static ERROR_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"(?i)^error:").unwrap(),
        Regex::new(r"(?i)^fatal:").unwrap(),
        Regex::new(r"(?i)^exception:").unwrap(),
        Regex::new(r"(?i)traceback").unwrap(),
        Regex::new(r"(?i)panic:").unwrap(),
        Regex::new(r"(?i)rate.?limit").unwrap(),
        Regex::new(r"(?i)api.?error").unwrap(),
    ]
});

/// State detector for analyzing pane content
#[derive(Debug, Clone)]
pub struct StateDetector {
    /// Number of lines from the end to analyze
    pub analyze_lines: usize,
}

impl Default for StateDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl StateDetector {
    /// Create a new state detector
    pub fn new() -> Self {
        Self { analyze_lines: 50 }
    }

    /// Create with custom line count
    pub fn with_analyze_lines(mut self, lines: usize) -> Self {
        self.analyze_lines = lines;
        self
    }

    /// Detect the agent state from captured content
    pub fn detect(&self, content: &CapturedContent) -> AgentState {
        // Get the last N lines for analysis
        let lines: Vec<&str> = content.content.lines().collect();
        let start = lines.len().saturating_sub(self.analyze_lines);
        let recent_content = lines[start..].join("\n");

        // Check for errors first (highest priority)
        if self.matches_any(&recent_content, &ERROR_PATTERNS) {
            debug!("Detected error state");
            return AgentState::Error;
        }

        // Check for processing indicators
        if self.matches_any(&recent_content, &PROCESSING_PATTERNS) {
            debug!("Detected processing state");
            return AgentState::Processing;
        }

        // Check for prompt patterns (waiting for input)
        if self.matches_any(&recent_content, &PROMPT_PATTERNS) {
            debug!("Detected waiting for input state");
            return AgentState::WaitingForInput;
        }

        // Default to unknown if no patterns match
        debug!("No state patterns matched, returning unknown");
        AgentState::Unknown
    }

    /// Check if content matches any of the given patterns
    fn matches_any(&self, content: &str, patterns: &[Regex]) -> bool {
        patterns.iter().any(|p| p.is_match(content))
    }

    /// Get a description of the detected state
    pub fn describe_state(&self, content: &CapturedContent) -> String {
        let state = self.detect(content);
        match state {
            AgentState::WaitingForInput => "Waiting for input".to_string(),
            AgentState::Processing => "Processing...".to_string(),
            AgentState::Error => "Error detected".to_string(),
            AgentState::Unknown => "Unknown state".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_content(s: &str) -> CapturedContent {
        CapturedContent::new(s.to_string())
    }

    #[test]
    fn test_detect_waiting_for_input() {
        let detector = StateDetector::new();

        // Claude prompt
        let content = make_content("Some output\n> ");
        assert_eq!(detector.detect(&content), AgentState::WaitingForInput);

        // Shell prompt
        let content = make_content("Output\nuser@host:~$ ");
        assert_eq!(detector.detect(&content), AgentState::WaitingForInput);
    }

    #[test]
    fn test_detect_processing() {
        let detector = StateDetector::new();

        // Spinner
        let content = make_content("Processing ⠋");
        assert_eq!(detector.detect(&content), AgentState::Processing);

        // Loading text
        let content = make_content("Thinking...");
        assert_eq!(detector.detect(&content), AgentState::Processing);
    }

    #[test]
    fn test_detect_error() {
        let detector = StateDetector::new();

        // Error message
        let content = make_content("Error: something went wrong");
        assert_eq!(detector.detect(&content), AgentState::Error);

        // Rate limit
        let content = make_content("API rate limit exceeded");
        assert_eq!(detector.detect(&content), AgentState::Error);
    }

    #[test]
    fn test_error_takes_priority() {
        let detector = StateDetector::new();

        // Error with prompt should still be error
        let content = make_content("Error: failed\n> ");
        assert_eq!(detector.detect(&content), AgentState::Error);
    }
}
