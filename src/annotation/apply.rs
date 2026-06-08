//! Applying staged annotations: deciding when it's safe to inject the prompt,
//! and the outcome of an apply attempt.
//!
//! The effectful orchestration (composing the brief, writing it, polling agent
//! state, sending keys) lives in `CommanderService::apply_annotations`; the
//! pure decision here is unit-tested in isolation.

use std::path::PathBuf;

use serde::Serialize;
use uuid::Uuid;

use crate::session::AgentState;

/// Whether the apply prompt can be injected immediately or must wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendDecision {
    /// Safe to send now. Claude queues typed input natively while `Working`,
    /// and is ready when `Idle`; `Unknown` is treated as best-effort send.
    Now,
    /// The agent is at a permission/selection prompt — injected text would be
    /// swallowed as a menu answer, so hold until the prompt clears.
    HoldUntilClear,
}

/// Decide how to deliver the apply prompt given the agent's current state.
pub fn decide_send(state: AgentState) -> SendDecision {
    match state {
        AgentState::WaitingForInput => SendDecision::HoldUntilClear,
        AgentState::Working | AgentState::Idle | AgentState::Unknown => SendDecision::Now,
    }
}

/// Outcome of applying a session's staged annotations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ApplyOutcome {
    /// No staged annotations to apply.
    Nothing,
    /// One or more annotations are drifted; nothing was sent.
    Blocked { drifted: Vec<Uuid> },
    /// Annotations were composed to `path` and the prompt injected.
    Applied { path: PathBuf, count: usize },
    /// The brief was written to `path` but couldn't be delivered (agent stopped
    /// or stayed at a prompt past the hold timeout); the user can re-apply.
    Deferred { path: PathBuf, count: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_only_for_waiting_for_input() {
        assert_eq!(decide_send(AgentState::Idle), SendDecision::Now);
        assert_eq!(decide_send(AgentState::Working), SendDecision::Now);
        assert_eq!(decide_send(AgentState::Unknown), SendDecision::Now);
        assert_eq!(
            decide_send(AgentState::WaitingForInput),
            SendDecision::HoldUntilClear
        );
    }
}
