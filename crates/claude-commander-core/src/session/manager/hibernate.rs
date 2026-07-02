//! Idle-session hibernation policy loop.
//!
//! A background task, owned by the library (not any frontend), periodically
//! stops the tmux process of sessions that have sat idle-and-unattended past a
//! configured threshold — freeing the ~400MB a live `claude` process holds —
//! while keeping the worktree, branch, and metadata so the session
//! transparently resumes on next attach (see [`SessionManager::hibernate_session`]).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use chrono::Utc;

use super::*;
use crate::agent::AgentKind;
use crate::session::AgentState;
use crate::telemetry::Telemetry;
use crate::tmux::AgentStateDetector;

/// Decide the next idle-tracking state for one session on a policy tick.
///
/// `idle_since` is when the session was first observed idle this streak (`None`
/// if it was active on the previous tick). Returns the `idle_since` to store
/// for the next tick and whether to hibernate now. The timer is *reset on every
/// active observation* — a session that works for an hour then idles briefly is
/// not hibernated on the strength of a stale first-idle stamp.
///
/// Pure, so it can be unit-tested without a clock or tmux.
pub(crate) fn idle_tick(
    is_active: bool,
    idle_since: Option<Instant>,
    now: Instant,
    threshold: Duration,
) -> (Option<Instant>, bool) {
    if is_active {
        return (None, false);
    }
    let since = idle_since.unwrap_or(now);
    let hibernate = now.duration_since(since) >= threshold;
    // Clear the timer once we hibernate; otherwise keep counting from `since`.
    (if hibernate { None } else { Some(since) }, hibernate)
}

/// Whether a session should count as active this tick (so its idle timer is
/// reset and it is never hibernated).
///
/// `Unknown` is treated as **active**: `detect_fresh` returns `Unknown` on any
/// tmux hiccup (pane-title or capture error — server overload, semaphore
/// timeout, transient EAGAIN), and a detection *failure* must never advance the
/// idle timer toward killing a session that may actually be working. This
/// mirrors the conservative attached-check (a failed attach probe also counts
/// as active); it costs at most one extra idle window for a genuinely-gone
/// session. Pure, so it can be unit-tested without tmux.
pub(crate) fn is_session_active(
    state: AgentState,
    attached: bool,
    recently_attached: bool,
) -> bool {
    matches!(
        state,
        AgentState::Working | AgentState::WaitingForInput | AgentState::Unknown
    ) || attached
        || recently_attached
}

impl SessionManager {
    /// Spawn the background hibernation loop. No-op unless `hibernate_enabled`
    /// is set, the check interval is non-zero, and a tokio runtime is present.
    ///
    /// Enablement is restart-required (mirrors the commander poll loop), but the
    /// idle threshold is read live each tick so it can be tuned without a
    /// restart.
    ///
    /// Only long-lived frontends should call this (the TUI does, via
    /// [`CommanderService::start_hibernation_loop`]). The runtime-presence check
    /// is NOT a proxy for "am I the TUI" — under `#[tokio::main]` a runtime is
    /// always present, and `tokio::time::interval`'s first tick fires
    /// immediately, so a one-shot CLI command that started this could run a full
    /// hibernation pass before exiting. `CommanderService::new` therefore does
    /// not call it; only the TUI startup path does.
    pub fn spawn_hibernation_loop(&self, telemetry: Telemetry) {
        let (enabled, interval_secs) = {
            let cfg = self.config_store.read();
            (cfg.hibernate_enabled, cfg.hibernate_check_interval_secs)
        };
        if !enabled || interval_secs == 0 || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let manager = self.clone();
        tokio::spawn(async move {
            manager.run_hibernation_loop(interval_secs, telemetry).await;
        });
        info!("Started hibernation loop (interval {}s)", interval_secs);
    }

    async fn run_hibernation_loop(&self, interval_secs: u64, telemetry: Telemetry) {
        // ZERO cache TTL: every hibernation decision reads fresh agent state, so
        // a session that just flipped Idle→Working isn't killed on a stale read.
        let mut detector = AgentStateDetector::new(self.tmux.clone(), Duration::ZERO);
        let mut idle_since: HashMap<SessionId, Instant> = HashMap::new();
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;

            let (still_enabled, threshold_secs, attach_grace_secs) = {
                let cfg = self.config_store.read();
                (
                    cfg.hibernate_enabled,
                    cfg.hibernate_idle_timeout_secs,
                    // Protect a session attached within roughly the last tick from
                    // the attach-race window; never shorter than 30s.
                    cfg.hibernate_check_interval_secs.max(30),
                )
            };
            if !still_enabled {
                // Toggled off at runtime: stop tracking but keep the loop alive.
                idle_since.clear();
                continue;
            }
            let threshold = Duration::from_secs(threshold_secs);
            let attach_grace = Duration::from_secs(attach_grace_secs);

            // Running, non-keep-alive sessions are the only candidates.
            let candidates: Vec<(SessionId, String, String, Option<chrono::DateTime<Utc>>)> = {
                let state = self.store.read().await;
                state
                    .sessions
                    .values()
                    .filter(|s| s.status == SessionStatus::Running && !s.keep_alive)
                    .map(|s| {
                        (
                            s.id,
                            s.tmux_session_name.clone(),
                            s.program.clone(),
                            s.last_attached_at,
                        )
                    })
                    .collect()
            };

            // Forget timers for sessions no longer candidates (stopped, deleted,
            // or newly keep-alive) so a later re-entry starts a fresh streak.
            let candidate_ids: HashSet<SessionId> = candidates.iter().map(|(id, ..)| *id).collect();
            idle_since.retain(|id, _| candidate_ids.contains(id));

            let now = Instant::now();
            for (id, tmux_name, program, last_attached_at) in candidates {
                let state = detector
                    .detect(AgentKind::from_program(&program), &tmux_name)
                    .await;
                // Conservative: a failed attached-check counts as attached, so a
                // detection error never triggers hibernation.
                let attached = self
                    .tmux
                    .is_session_attached(&tmux_name)
                    .await
                    .unwrap_or(true);
                // last_attached_at is stamped just before the PTY attaches, so a
                // recent stamp guards the sub-second window where a session being
                // attached still reads unattached.
                let recently_attached = last_attached_at
                    .and_then(|t| (Utc::now() - t).to_std().ok())
                    .is_some_and(|elapsed| elapsed < attach_grace);
                let is_active = is_session_active(state, attached, recently_attached);

                let (next, hibernate) =
                    idle_tick(is_active, idle_since.get(&id).copied(), now, threshold);
                match next {
                    Some(t) => {
                        idle_since.insert(id, t);
                    }
                    None => {
                        idle_since.remove(&id);
                    }
                }
                if hibernate {
                    info!("Auto-hibernating idle session {}", id);
                    telemetry.feature("hibernate_auto");
                    if let Err(e) = self.hibernate_session(&id).await {
                        warn!("Failed to hibernate session {}: {}", id, e);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn active_session_resets_timer_and_never_hibernates() {
        let now = Instant::now();
        // Active with an existing idle stamp: timer cleared, no hibernate.
        let (next, hibernate) = idle_tick(true, Some(now - secs(10_000)), now, secs(1800));
        assert_eq!(next, None);
        assert!(!hibernate);
    }

    #[test]
    fn first_idle_observation_stamps_now_and_waits() {
        let now = Instant::now();
        let (next, hibernate) = idle_tick(false, None, now, secs(1800));
        assert_eq!(next, Some(now));
        assert!(!hibernate);
    }

    #[test]
    fn idle_past_threshold_hibernates_and_clears() {
        let now = Instant::now();
        let idle_since = now - secs(1800);
        let (next, hibernate) = idle_tick(false, Some(idle_since), now, secs(1800));
        assert!(hibernate);
        // Timer cleared so a woken-then-idle session gets a fresh full window.
        assert_eq!(next, None);
    }

    #[test]
    fn idle_below_threshold_keeps_counting_from_original_stamp() {
        let now = Instant::now();
        let idle_since = now - secs(100);
        let (next, hibernate) = idle_tick(false, Some(idle_since), now, secs(1800));
        assert!(!hibernate);
        // Must keep the ORIGINAL stamp, not reset to now — else it never expires.
        assert_eq!(next, Some(idle_since));
    }

    #[test]
    fn unknown_agent_state_counts_as_active() {
        // A transient tmux detection failure surfaces as Unknown. It must NOT
        // be treated as idle, or a hiccup while a detached session is working
        // would advance the idle timer toward hibernating it mid-work.
        assert!(is_session_active(AgentState::Unknown, false, false));
    }

    #[test]
    fn explicit_idle_and_unattached_counts_as_inactive() {
        // Only an explicit Idle read on an unattached session is inactive.
        assert!(!is_session_active(AgentState::Idle, false, false));
    }

    #[test]
    fn working_or_attached_or_recent_counts_as_active() {
        assert!(is_session_active(AgentState::Working, false, false));
        assert!(is_session_active(AgentState::WaitingForInput, false, false));
        // Idle but attached, or recently attached, is still active.
        assert!(is_session_active(AgentState::Idle, true, false));
        assert!(is_session_active(AgentState::Idle, false, true));
    }
}
