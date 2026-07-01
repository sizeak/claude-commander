//! Applying staged comments: deciding when it's safe to inject the prompt,
//! and the outcome of an apply attempt.
//!
//! The effectful orchestration (composing the brief, writing it, polling agent
//! state, sending keys) lives in `CommanderService::apply_comments`; the
//! pure decision here is unit-tested in isolation.

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

// `ApplyOutcome` (the apply-result wire type) lives in
// `claude-commander-protocol`; it's re-exported from this module's parent
// (`crate::comment`) so existing paths keep working.

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
