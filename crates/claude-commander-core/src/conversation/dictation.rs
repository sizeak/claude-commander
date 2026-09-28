//! Dictation: deciding what a voice transcript should do to the attached pane.
//!
//! Conversation mode speaks a transcript *to* a headless agent; dictation types
//! it *into* whatever pane is on screen, agent or shell. The difference that
//! matters here is that a pane is a terminal, so the transcript is keystrokes:
//! a stray newline in the text is a submit the user never asked for, and an
//! Enter after the text is a submit they may or may not have.
//!
//! Everything in this module is pure — [`plan_dictation`] turns a raw
//! transcript, the configured policy and a description of the pane into a
//! [`DictationPlan`], and something else does the typing. That split is what
//! makes the policy testable without a tmux server: the awkward cases (a
//! transcription engine that returns a trailing newline, a shell pane under an
//! `Always` policy, the per-harness submit delay) are all decided here.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::backend::AttachKind;
use crate::tmux::PaneInfo;

/// Whether an Enter follows a dictated transcript into the pane.
///
/// Insert-only ([`Never`](Self::Never)) is the default: dictation drops the text
/// into the composer and the user reads it before pressing Enter themselves,
/// which is the safe default when transcription can mishear. The other two trade
/// that review step for hands-free operation —
/// [`Agent`](Self::Agent) only on an agent pane (where a wrong submit costs a
/// turn, not a command), [`Always`](Self::Always) on any pane including a shell
/// (where it runs whatever was heard).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DictationSubmit {
    /// Type the transcript and stop; the user presses Enter.
    #[default]
    Never,
    /// Submit on an agent pane only; a shell pane is insert-only.
    Agent,
    /// Submit on any pane, shell included.
    Always,
}

impl DictationSubmit {
    /// All variants, in display order (used by the settings picker).
    pub const ALL: [DictationSubmit; 3] = [Self::Never, Self::Agent, Self::Always];

    /// snake_case config token (matches the serde representation).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Agent => "agent",
            Self::Always => "always",
        }
    }

    /// Parse from the snake_case config token.
    pub fn from_token(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }

    /// Parse from the human label (used by the settings option-picker).
    pub fn from_label(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.label() == s)
    }

    /// Human-friendly label for the settings UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::Never => "Never",
            Self::Agent => "Agent",
            Self::Always => "Always",
        }
    }
}

/// What follows the typed text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitPlan {
    /// Nothing — the text sits in the composer awaiting the user's own Enter.
    NoSubmit,
    /// Send an Enter, after `delay` if the harness needs one to read the text as
    /// its own keystroke first (see [`AgentKind::submit_key_delay`](crate::agent::AgentKind::submit_key_delay) for why
    /// Codex does and the others don't).
    Submit { delay: Option<Duration> },
}

/// A dictated transcript reduced to the two things the injector needs: the exact
/// text to type, and whether an Enter follows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictationPlan {
    pub text: String,
    pub submit: SubmitPlan,
}

/// Flatten a raw transcript into one line of typeable text.
///
/// Every run of whitespace — including `\r\n`, `\r`, `\n` and tabs — becomes a
/// single space, because a line break typed into a pane is not whitespace: it is
/// Enter, and it would submit a half-finished sentence in the middle of
/// dictation. Transcription engines routinely return a trailing newline, and a
/// paragraph break comes back as two, so this runs on every transcript rather
/// than only on suspect ones. Collapsing runs (rather than mapping each break
/// to a space) is what keeps a blank line from landing as a double space. The
/// result is trimmed at both ends, so it can still be empty (silence, or a
/// transcript that was nothing but whitespace) — [`plan_dictation`] treats that
/// as nothing to do.
pub fn normalise_dictation(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Decide what a transcript does to the pane described by `pane`, under
/// `policy`.
///
/// `None` when the normalised text is empty — there is nothing to type, and a
/// bare Enter into someone's shell is the worst thing an empty transcript could
/// do. Otherwise the submit decision follows the policy: `Never` never submits;
/// `Agent` submits only on an agent pane; `Always` submits anywhere. The delay
/// comes from the pane's harness either way, and a shell pane's
/// [`AgentKind::Unknown`](crate::agent::AgentKind::Unknown) yields `None`, so an `Always` submit into a shell is
/// sent straight after the text.
pub fn plan_dictation(raw: &str, policy: DictationSubmit, pane: PaneInfo) -> Option<DictationPlan> {
    let text = normalise_dictation(raw);
    if text.is_empty() {
        return None;
    }
    let submits = match policy {
        DictationSubmit::Never => false,
        DictationSubmit::Agent => pane.kind == AttachKind::Agent,
        DictationSubmit::Always => true,
    };
    let submit = if submits {
        SubmitPlan::Submit {
            delay: pane.agent.submit_key_delay(),
        }
    } else {
        SubmitPlan::NoSubmit
    };
    Some(DictationPlan { text, submit })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentKind;
    use crate::backend::AttachKind;
    use crate::tmux::PaneInfo;
    use std::time::Duration;

    fn pane(kind: AttachKind, agent: AgentKind) -> PaneInfo {
        PaneInfo { kind, agent }
    }

    #[test]
    fn normalise_replaces_newlines_and_trims() {
        assert_eq!(normalise_dictation("  hello\nworld  "), "hello world");
        assert_eq!(normalise_dictation("a\r\nb"), "a b");
        assert_eq!(normalise_dictation("a\rb"), "a b");
    }

    #[test]
    fn normalise_collapses_blank_lines_and_tabs() {
        // A paragraph break is two newlines; typed as two spaces it would read
        // as a stray gap in the prompt. Tabs are keystrokes too (completion!).
        assert_eq!(normalise_dictation("first\n\nsecond"), "first second");
        assert_eq!(normalise_dictation("a\tb  c"), "a b c");
    }

    #[test]
    fn normalise_never_ends_in_newline() {
        for raw in ["trailing\n", "trailing\r\n", "trailing\r", "trailing\n\n"] {
            let out = normalise_dictation(raw);
            assert!(!out.ends_with('\n'), "{out:?} ends in LF");
            assert!(!out.ends_with('\r'), "{out:?} ends in CR");
            assert_eq!(out, "trailing");
        }
    }

    #[test]
    fn plan_never_inserts_only() {
        let plan = plan_dictation(
            "run the tests",
            DictationSubmit::Never,
            pane(AttachKind::Agent, AgentKind::Codex),
        )
        .expect("non-empty text plans");
        assert_eq!(plan.text, "run the tests");
        assert_eq!(plan.submit, SubmitPlan::NoSubmit);
    }

    #[test]
    fn plan_agent_submits_on_agent_pane_with_codex_delay() {
        let plan = plan_dictation(
            "run the tests",
            DictationSubmit::Agent,
            pane(AttachKind::Agent, AgentKind::Codex),
        )
        .expect("non-empty text plans");
        assert_eq!(
            plan.submit,
            SubmitPlan::Submit {
                delay: Some(Duration::from_millis(250))
            }
        );
    }

    #[test]
    fn plan_agent_no_delay_for_claude() {
        let plan = plan_dictation(
            "run the tests",
            DictationSubmit::Agent,
            pane(AttachKind::Agent, AgentKind::Claude),
        )
        .expect("non-empty text plans");
        assert_eq!(plan.submit, SubmitPlan::Submit { delay: None });
    }

    #[test]
    fn plan_agent_does_not_submit_on_shell_pane() {
        let plan = plan_dictation(
            "ls -la",
            DictationSubmit::Agent,
            pane(AttachKind::Shell, AgentKind::Unknown),
        )
        .expect("non-empty text plans");
        assert_eq!(plan.submit, SubmitPlan::NoSubmit);
    }

    #[test]
    fn plan_always_submits_on_shell() {
        let plan = plan_dictation(
            "ls -la",
            DictationSubmit::Always,
            pane(AttachKind::Shell, AgentKind::Unknown),
        )
        .expect("non-empty text plans");
        assert_eq!(plan.submit, SubmitPlan::Submit { delay: None });
    }

    #[test]
    fn plan_empty_text_is_none() {
        for raw in ["", "   ", "\n", " \r\n "] {
            assert!(
                plan_dictation(
                    raw,
                    DictationSubmit::Always,
                    pane(AttachKind::Agent, AgentKind::Claude)
                )
                .is_none(),
                "{raw:?} should not plan"
            );
        }
    }

    #[test]
    fn dictation_submit_tokens_roundtrip() {
        for v in DictationSubmit::ALL {
            assert_eq!(DictationSubmit::from_token(v.as_str()), Some(v));
            assert_eq!(DictationSubmit::from_label(v.label()), Some(v));
        }
        assert_eq!(DictationSubmit::default(), DictationSubmit::Never);
        assert_eq!(DictationSubmit::from_token("nope"), None);
    }
}
