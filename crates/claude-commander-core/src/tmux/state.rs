//! Agent state detection via tmux pane inspection.
//!
//! Detects whether an agent session is Working, Idle, or WaitingForInput by
//! inspecting the tmux pane title and visible pane content. The per-harness
//! pattern rules live on [`AgentKind`]; this module owns the tmux capture and
//! result caching mechanics and dispatches to the right harness.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use tracing::debug;

use super::TmuxExecutor;
use crate::agent::AgentKind;
use crate::error::Result;
use crate::session::{AgentState, SessionId};

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
    ///
    /// `kind` selects the harness-specific pattern rules.
    pub async fn detect(&mut self, kind: AgentKind, tmux_session_name: &str) -> AgentState {
        // Unrecognised harnesses have no reliable idle signal: `content_state`
        // falls back to `Idle` for them, which would let the hibernation loop
        // mistake an active non-agent session (a build, a shell, a custom
        // wrapper) for idle and kill it. Report `Unknown` — callers treat it as
        // active — without consulting tmux or the cache.
        if kind == AgentKind::Unknown {
            return AgentState::Unknown;
        }

        // Check cache
        if let Some((state, cached_at)) = self.cache.get(tmux_session_name)
            && cached_at.elapsed() < self.cache_ttl
        {
            return *state;
        }

        let state = self.detect_fresh(kind, tmux_session_name).await;

        self.cache
            .insert(tmux_session_name.to_string(), (state, Instant::now()));

        state
    }

    /// Perform fresh detection bypassing cache.
    async fn detect_fresh(&self, kind: AgentKind, tmux_session_name: &str) -> AgentState {
        // Primary: check pane title (conclusive titles skip the content capture).
        match self.get_pane_title(tmux_session_name).await {
            Ok(title) => {
                if let Some(state) = kind.title_state(&title) {
                    debug!(
                        "Pane title detection for {}: {:?}",
                        tmux_session_name, state
                    );
                    return state;
                }
                // Title inconclusive — fall through to content.
            }
            Err(e) => {
                debug!("Failed to get pane title for {}: {}", tmux_session_name, e);
                return AgentState::Unknown;
            }
        }

        // Secondary: parse visible pane content.
        match self.capture_visible_pane(tmux_session_name).await {
            Ok(content) => {
                let state = kind.content_state(&content);
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
    /// Sessions whose program is an unrecognised harness get `Unknown` without
    /// any tmux inspection.
    pub async fn detect_all(
        &mut self,
        sessions: &[(SessionId, String, String)],
    ) -> BTreeMap<SessionId, AgentState> {
        let mut results = BTreeMap::new();

        for (session_id, tmux_name, program) in sessions {
            let kind = AgentKind::from_program(program);
            // `detect` short-circuits Unknown kinds to `Unknown` itself.
            let state = self.detect(kind, tmux_name).await;
            results.insert(*session_id, state);
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- cache TTL boundary tests --
    //
    // These target the cache freshness check at the top of `detect()`:
    //   if let Some((state, cached_at)) = self.cache.get(tmux_session_name)
    //       && cached_at.elapsed() < self.cache_ttl
    //   { return *state; }
    //
    // They distinguish `<` from `<=`/`==`/`>` by pre-populating the private
    // `cache` field directly (same-module privacy) so we can place entries at
    // precise points relative to the TTL window without wall-clock sleeps.
    // `Instant::elapsed()` saturates to `Duration::ZERO` for instants in the
    // future, giving us an exact boundary handle.

    #[tokio::test]
    async fn test_detect_cache_hit_with_fresh_entry() {
        // Large TTL + just-now cached_at => elapsed is tiny relative to TTL, so
        // the freshness check is firmly inside the cache window. Returns the
        // cached state without invoking the executor.
        let executor = TmuxExecutor::new();
        let mut detector = AgentStateDetector::new(executor, Duration::from_secs(3600));
        detector.cache.insert(
            "tts-fresh".to_string(),
            (AgentState::Working, Instant::now()),
        );

        let result = detector.detect(AgentKind::Claude, "tts-fresh").await;
        assert_eq!(
            result,
            AgentState::Working,
            "fresh cache entry within TTL must hit and return the cached state"
        );
    }

    #[tokio::test]
    async fn test_detect_cache_miss_at_exact_boundary() {
        // Future cached_at + a TTL of zero yields `elapsed == ttl == 0`
        // deterministically. Correct `<` says miss; `<=`/`==` say hit. We
        // observe hit-vs-miss by inspecting the cache entry's instant after the
        // call: on a miss `detect_fresh` re-inserts a now-stamped entry whose
        // `elapsed()` becomes positive immediately.
        let executor = TmuxExecutor::new();
        let mut detector = AgentStateDetector::new(executor, Duration::ZERO);
        let future = Instant::now() + Duration::from_secs(3600);
        detector
            .cache
            .insert("tts-boundary".to_string(), (AgentState::Working, future));

        let _ = detector.detect(AgentKind::Claude, "tts-boundary").await;

        let (_, after) = detector
            .cache
            .get("tts-boundary")
            .expect("cache entry must still exist after detect");
        assert!(
            after.elapsed() > Duration::ZERO,
            "with ttl=0 and elapsed=0, correct `<` must miss and re-insert a \
             now-stamped entry; a `<=`/`==` mutant would have hit and left the \
             original future instant in place (elapsed == 0)"
        );
    }

    // -- unknown-kind short-circuit --

    #[tokio::test]
    async fn test_detect_unknown_kind_short_circuits_before_cache() {
        // An unrecognised harness must report Unknown (treated as active by the
        // hibernation loop), NOT fall through to the content heuristic's Idle.
        // Proven by seeding the cache with Idle: the Unknown-kind guard runs
        // before the cache check, so it must win and return Unknown. Without the
        // guard, `detect` would hit the fresh cache entry and return Idle —
        // which is exactly the read that would get an active session hibernated.
        let executor = TmuxExecutor::new();
        let mut detector = AgentStateDetector::new(executor, Duration::from_secs(3600));
        detector
            .cache
            .insert("uk-sess".to_string(), (AgentState::Idle, Instant::now()));

        let state = detector.detect(AgentKind::Unknown, "uk-sess").await;
        assert_eq!(state, AgentState::Unknown);
    }

    // -- detect_all filtering --

    #[tokio::test]
    async fn test_detect_all_marks_unknown_program_unknown() {
        // Non-agent programs get Unknown without attempting tmux inspection.
        let executor = TmuxExecutor::new();
        let mut detector = AgentStateDetector::new(executor, Duration::from_secs(10));

        let sessions = vec![(
            SessionId::new(),
            "non-existent-session".to_string(),
            "bash".to_string(),
        )];

        let results = detector.detect_all(&sessions).await;
        assert_eq!(results.len(), 1);
        for state in results.values() {
            assert_eq!(*state, AgentState::Unknown);
        }
    }
}
