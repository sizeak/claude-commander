//! Pull-request wire enums.
//!
//! The PR *state* and *review decision* a client renders as badges. The PR
//! fetch/derivation logic (and richer types like check status) stays in
//! `claude-commander-core`; only these two enums cross the network.

use serde::{Deserialize, Serialize};

/// PR state as reported by the GitHub API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

/// GitHub `reviewDecision` field — derived state of the review process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    /// Reviews requested, none decisive yet (includes comment-only reviews).
    ReviewRequired,
    /// At least one approving review and no outstanding changes-requested.
    Approved,
    /// At least one reviewer requested changes.
    ChangesRequested,
}

impl std::fmt::Display for ReviewDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReviewRequired => write!(f, "Review required"),
            Self::Approved => write!(f, "Approved"),
            Self::ChangesRequested => write!(f, "Changes requested"),
        }
    }
}

impl std::fmt::Display for PrState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "Open"),
            Self::Closed => write!(f, "Closed"),
            Self::Merged => write!(f, "Merged"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        assert_eq!(
            serde_json::to_string(&PrState::Merged).unwrap(),
            r#""merged""#
        );
        assert_eq!(
            serde_json::from_str::<ReviewDecision>(r#""changes_requested""#).unwrap(),
            ReviewDecision::ChangesRequested
        );
    }
}
