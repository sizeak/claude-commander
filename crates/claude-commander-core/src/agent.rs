//! Agent harness abstraction.
//!
//! Claude Commander launches different agent CLIs (Claude Code, OpenAI Codex,
//! OpenCode) inside tmux sessions. Each harness differs in how it is resumed,
//! whether it accepts a positional prompt, and what it renders in the tmux pane
//! while working or waiting for the user.
//!
//! [`AgentKind`] is *derived* from the persisted `program` command string (never
//! stored separately) and owns this per-harness behaviour, so the divergences
//! live in one place. Adding a new harness is a new enum variant plus filling in
//! its methods — the compiler then flags every behaviour left unimplemented.

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

use crate::session::AgentState;

/// Pre-compiled regex for stripping ANSI escape sequences (CSI sequences and
/// OSC strings terminated by BEL or ST).
static ANSI_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1B\[[0-9;]*[a-zA-Z]|\x1B\][^\x07]*\x07|\x1B\][^\x1B]*\x1B\\")
        .expect("valid regex")
});

/// OpenCode renders completed assistant turns as an agent/model/duration line,
/// e.g. `▣ Build · GPT-5.5 · 8.5s`. Active turns have the same agent/model
/// prefix but no duration; the active signal is the separate `esc interrupt`
/// footer below.
static OPENCODE_COMPLETED_TURN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^\s*▣\s+.+\s·\s+\d+(?:\.\d+)?(?:ms|s|m|h)(?:\s+\d+(?:\.\d+)?(?:ms|s|m|h))*\s*$",
    )
    .expect("valid regex")
});

/// Strip ANSI escape sequences from a string.
pub fn strip_ansi(s: &str) -> String {
    ANSI_RE.replace_all(s, "").into_owned()
}

/// Whether `title` contains a braille spinner glyph (U+2800..U+28FF). Both the
/// Claude Code and Codex TUIs animate a braille spinner in the terminal title
/// while the model is working, so this is shared across harnesses.
fn has_braille_spinner(title: &str) -> bool {
    title.contains(|c: char| ('\u{2800}'..='\u{28FF}').contains(&c))
}

/// Pane-content substrings Codex renders when it is blocked waiting for the user
/// to approve a command, edit, or network access. These are part of the
/// always-rendered approval overlay (independent of the user-configurable
/// terminal-title items), so they are the durable signal for `WaitingForInput`.
const CODEX_APPROVAL_MARKERS: [&str; 5] = [
    "Would you like to run the following command?",
    "Do you want to approve network access to",
    "Would you like to grant these permissions?",
    "Would you like to make the following edits?",
    "needs your approval.",
];

/// Pane-content substring Codex renders in its status line while a task is
/// actively running — the interrupt hint in `• Working (12s • esc to interrupt)`.
/// A durable `Working` signal that survives a user customising the terminal
/// title (e.g. dropping the spinner via `/title`), so working sessions aren't
/// mislabelled `Idle` when the title check can't see a spinner. Distinct from
/// the approval footer, which reads "esc to cancel".
const CODEX_WORKING_MARKER: &str = "esc to interrupt";

/// Pane-content substring OpenCode renders in both full and mini TUI footers
/// while a task is actively running. Completed turns keep the prompt footer but
/// drop this interrupt hint, so it is a durable active-task signal.
const OPENCODE_WORKING_MARKER: &str = "esc interrupt";

/// Pane-content substring OpenCode renders in its permission prompt overlay
/// when the agent is blocked on a user approval decision.
const OPENCODE_PERMISSION_MARKER: &str = "Permission required";

/// The agent CLI harness backing a session, derived from its `program` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    /// Anthropic Claude Code (`claude`).
    Claude,
    /// OpenAI Codex CLI (`codex`).
    Codex,
    /// OpenCode TUI (`opencode`).
    OpenCode,
    /// Any other program (a bare shell, an unrecognised agent, …). We launch it
    /// but make no assumptions about its flags or TUI output.
    Unknown,
}

impl AgentKind {
    /// Derive the harness from a program command string by its first token,
    /// tolerating path prefixes and trailing arguments — e.g. `claude`,
    /// `Claude --resume`, and `/usr/local/bin/codex -m gpt-5` all resolve.
    pub fn from_program(program: &str) -> Self {
        let name = program
            .split_whitespace()
            .next()
            .and_then(|tok| tok.rsplit('/').next())
            .unwrap_or("");
        if name.eq_ignore_ascii_case("claude") {
            Self::Claude
        } else if name.eq_ignore_ascii_case("codex") {
            Self::Codex
        } else if name.eq_ignore_ascii_case("opencode") {
            Self::OpenCode
        } else {
            Self::Unknown
        }
    }

    /// Whether this harness is Claude Code. Claude-only launch flags
    /// (`--permission-mode`, `--effort`, `-n <name>`) gate on this.
    pub fn is_claude(self) -> bool {
        self == Self::Claude
    }

    /// Whether the harness accepts a single positional prompt argument at
    /// launch. Both Claude and Codex do (`claude '<prompt>'`,
    /// `codex '<prompt>'`); an unknown program (e.g. a bare shell) does not, so
    /// we must not append a prompt it would mis-parse.
    pub fn accepts_positional_prompt(self) -> bool {
        matches!(self, Self::Claude | Self::Codex)
    }

    /// Whether this harness accepts a `--model <name>` launch flag. Claude,
    /// Codex, and OpenCode support it; an unknown program's flags are
    /// unconstrained.
    pub fn supports_model_flag(self) -> bool {
        matches!(self, Self::Claude | Self::Codex | Self::OpenCode)
    }

    /// Delay to wait between injecting prompt *text* into the pane and sending
    /// the submit `Enter`, or `None` to send the two back-to-back.
    ///
    /// Codex folds a carriage-return that arrives in the same terminal read as
    /// the preceding text into the pasted text (as a literal newline) rather
    /// than treating it as a submit keystroke, so a back-to-back text+Enter
    /// leaves the prompt sitting unsent in the composer until a *separate* Enter
    /// arrives. Spacing the Enter out lets Codex drain the text first, so the
    /// Enter lands as its own read and submits. Claude Code submits on the
    /// carriage-return regardless of timing, so it needs no delay. (Verified
    /// against codex-cli 0.144.3: a coalesced text+Enter write never submitted
    /// across 5/5 trials; a ~200ms gap submitted 15/15.)
    pub fn submit_key_delay(self) -> Option<Duration> {
        match self {
            Self::Codex => Some(Duration::from_millis(250)),
            _ => None,
        }
    }

    /// Build the command that resumes this harness's previous session,
    /// preserving any flags on the base command. Returns `None` when the harness
    /// has no resume mechanism we can drive (so the caller launches fresh).
    ///
    /// Claude appends a `--resume` flag; Codex uses a `resume --last` subcommand
    /// that must follow the binary, before its other flags; OpenCode appends
    /// `--continue`.
    pub fn resume_command(self, program: &str) -> Option<String> {
        let mut parts = program.splitn(2, char::is_whitespace);
        let binary = parts.next().unwrap_or("");
        let rest = parts.next();
        match self {
            Self::Claude => Some(match rest {
                Some(r) => format!("{binary} {r} --resume"),
                None => format!("{binary} --resume"),
            }),
            Self::Codex => Some(match rest {
                Some(r) => format!("{binary} resume --last {r}"),
                None => format!("{binary} resume --last"),
            }),
            Self::OpenCode => Some(match rest {
                Some(r) => format!("{binary} {r} --continue"),
                None => format!("{binary} --continue"),
            }),
            Self::Unknown => None,
        }
    }

    /// Detect agent state from the tmux pane *title*. Returns `Some` when the
    /// title alone is conclusive (so the caller can skip capturing pane
    /// content), `None` when content must be inspected.
    pub fn title_state(self, title: &str) -> Option<AgentState> {
        match self {
            // Codex prefixes the title with "Action Required" (no spinner) while
            // blocked on approval — check it before the shared spinner since the
            // two are mutually exclusive in Codex's title.
            Self::Codex if title.contains("Action Required") => Some(AgentState::WaitingForInput),
            Self::Claude | Self::Codex if has_braille_spinner(title) => Some(AgentState::Working),
            // OpenCode's title is always "OpenCode" regardless of state, so
            // title alone is not conclusive — fall through to content.
            _ => None,
        }
    }

    /// Detect agent state from the visible pane *content* (the fallback when the
    /// title is inconclusive). Recognised harnesses with no durable idle signal
    /// should return `Unknown` rather than guessing. The `Unknown` harness arm is
    /// retained as a benign fallback for direct unit tests; `AgentStateDetector`
    /// short-circuits unknown programs before pane inspection.
    pub fn content_state(self, content: &str) -> AgentState {
        let content = strip_ansi(content);
        match self {
            Self::Claude => claude_content_state(&content),
            Self::Codex => codex_content_state(&content),
            Self::OpenCode => opencode_content_state(&content),
            Self::Unknown => AgentState::Idle,
        }
    }
}

/// Claude content patterns: the last visible lines carry permission/selection
/// prompts when Claude is waiting for the user.
fn claude_content_state(content: &str) -> AgentState {
    let lines: Vec<&str> = content
        .lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .take(10)
        .collect();

    for line in &lines {
        // Permission prompt footer.
        if line.contains("Esc to cancel") {
            return AgentState::WaitingForInput;
        }
        // Rejection menu option.
        if line.contains("No, and tell Claude what to do differently") {
            return AgentState::WaitingForInput;
        }
        // Selection menu: ❯ followed by a digit.
        if let Some(pos) = line.find('\u{276F}') {
            let after = line[pos + '\u{276F}'.len_utf8()..].trim_start();
            if after.starts_with(|c: char| c.is_ascii_digit()) {
                return AgentState::WaitingForInput;
            }
        }
    }

    AgentState::Idle
}

/// Codex content patterns. The approval overlay's question text is rendered in
/// the visible pane whenever Codex is blocked on the user; the interrupt hint is
/// rendered while a task runs. Scanning the whole visible pane (which
/// `capture-pane -p` already bounds to the current screen) is robust to the
/// overlay's variable height. Approval takes precedence over working.
fn codex_content_state(content: &str) -> AgentState {
    if CODEX_APPROVAL_MARKERS
        .iter()
        .any(|marker| content.contains(marker))
    {
        return AgentState::WaitingForInput;
    }
    if content.contains(CODEX_WORKING_MARKER) {
        return AgentState::Working;
    }
    AgentState::Idle
}

/// OpenCode content patterns. The interrupt hint is rendered while a task runs.
/// Completed turns render an agent/model/duration line, which is the durable idle
/// signal. Brand-new sessions still return `Unknown`: "Ask anything" only
/// appears before the first turn and is not a general idle marker.
fn opencode_content_state(content: &str) -> AgentState {
    if content.contains(OPENCODE_PERMISSION_MARKER) {
        return AgentState::WaitingForInput;
    }
    if content.contains(OPENCODE_WORKING_MARKER) {
        return AgentState::Working;
    }
    if OPENCODE_COMPLETED_TURN_RE.is_match(content) {
        return AgentState::Idle;
    }
    AgentState::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- from_program ---

    #[test]
    fn from_program_detects_claude() {
        assert_eq!(AgentKind::from_program("claude"), AgentKind::Claude);
        assert_eq!(
            AgentKind::from_program("claude --resume"),
            AgentKind::Claude
        );
        assert_eq!(AgentKind::from_program("Claude"), AgentKind::Claude);
        assert_eq!(AgentKind::from_program("CLAUDE -c"), AgentKind::Claude);
        assert_eq!(
            AgentKind::from_program("/usr/local/bin/claude --debug"),
            AgentKind::Claude
        );
    }

    #[test]
    fn from_program_detects_codex() {
        assert_eq!(AgentKind::from_program("codex"), AgentKind::Codex);
        assert_eq!(AgentKind::from_program("codex -m gpt-5"), AgentKind::Codex);
        assert_eq!(AgentKind::from_program("Codex"), AgentKind::Codex);
        assert_eq!(
            AgentKind::from_program("/opt/homebrew/bin/codex --full-auto"),
            AgentKind::Codex
        );
    }

    #[test]
    fn from_program_detects_opencode() {
        assert_eq!(AgentKind::from_program("opencode"), AgentKind::OpenCode);
        assert_eq!(
            AgentKind::from_program("opencode --auto"),
            AgentKind::OpenCode
        );
        assert_eq!(AgentKind::from_program("OpenCode"), AgentKind::OpenCode);
        assert_eq!(
            AgentKind::from_program("/usr/local/bin/opencode"),
            AgentKind::OpenCode
        );
    }

    #[test]
    fn from_program_unknown_for_others() {
        assert_eq!(AgentKind::from_program("bash"), AgentKind::Unknown);
        assert_eq!(AgentKind::from_program(""), AgentKind::Unknown);
        // Different binary names that merely contain the substring must not match.
        assert_eq!(AgentKind::from_program("claude-code"), AgentKind::Unknown);
        assert_eq!(AgentKind::from_program("codex-cli"), AgentKind::Unknown);
        assert_eq!(AgentKind::from_program("opencode-ai"), AgentKind::Unknown);
    }

    // --- capability flags ---

    #[test]
    fn accepts_positional_prompt_for_agents_only() {
        assert!(AgentKind::Claude.accepts_positional_prompt());
        assert!(AgentKind::Codex.accepts_positional_prompt());
        // OpenCode's plain `opencode` does not accept a positional prompt;
        // prompts are passed via `opencode run [message..]` instead.
        assert!(!AgentKind::OpenCode.accepts_positional_prompt());
        assert!(!AgentKind::Unknown.accepts_positional_prompt());
    }

    #[test]
    fn is_claude_only_for_claude() {
        assert!(AgentKind::Claude.is_claude());
        assert!(!AgentKind::Codex.is_claude());
        assert!(!AgentKind::OpenCode.is_claude());
        assert!(!AgentKind::Unknown.is_claude());
    }

    #[test]
    fn supports_model_flag_for_agents_only() {
        assert!(AgentKind::Claude.supports_model_flag());
        assert!(AgentKind::Codex.supports_model_flag());
        assert!(AgentKind::OpenCode.supports_model_flag());
        assert!(!AgentKind::Unknown.supports_model_flag());
    }

    #[test]
    fn submit_key_delay_only_for_codex() {
        // Codex needs the submit Enter spaced out from the injected prompt text
        // or it folds the newline into the paste and never submits; the other
        // harnesses submit on the carriage-return regardless. Removing the delay
        // reintroduces the "comments sit unsent in the composer" bug.
        assert_eq!(
            AgentKind::Codex.submit_key_delay(),
            Some(Duration::from_millis(250))
        );
        assert_eq!(AgentKind::Claude.submit_key_delay(), None);
        assert_eq!(AgentKind::OpenCode.submit_key_delay(), None);
        assert_eq!(AgentKind::Unknown.submit_key_delay(), None);
    }

    // --- resume_command ---

    #[test]
    fn resume_command_claude_appends_flag() {
        assert_eq!(
            AgentKind::Claude.resume_command("claude"),
            Some("claude --resume".to_string())
        );
        assert_eq!(
            AgentKind::Claude.resume_command("claude -c"),
            Some("claude -c --resume".to_string())
        );
    }

    #[test]
    fn resume_command_codex_uses_subcommand_after_binary() {
        assert_eq!(
            AgentKind::Codex.resume_command("codex"),
            Some("codex resume --last".to_string())
        );
        // Flags on the base command survive, and the subcommand lands right
        // after the binary (not at the end).
        assert_eq!(
            AgentKind::Codex.resume_command("codex -m gpt-5"),
            Some("codex resume --last -m gpt-5".to_string())
        );
    }

    #[test]
    fn resume_command_none_for_unknown() {
        assert_eq!(AgentKind::Unknown.resume_command("bash"), None);
    }

    #[test]
    fn resume_command_opencode_appends_continue() {
        assert_eq!(
            AgentKind::OpenCode.resume_command("opencode"),
            Some("opencode --continue".to_string())
        );
        assert_eq!(
            AgentKind::OpenCode.resume_command("opencode --auto"),
            Some("opencode --auto --continue".to_string())
        );
    }

    // --- title_state ---

    #[test]
    fn title_state_working_braille_both_harnesses() {
        // Braille spinner frame U+280B → Working for both.
        assert_eq!(
            AgentKind::Claude.title_state("⠋ feature-branch"),
            Some(AgentState::Working)
        );
        assert_eq!(
            AgentKind::Codex.title_state("⠹ my-project"),
            Some(AgentState::Working)
        );
    }

    #[test]
    fn title_state_codex_action_required_is_waiting() {
        assert_eq!(
            AgentKind::Codex.title_state("[ ! ] Action Required | my-project"),
            Some(AgentState::WaitingForInput)
        );
        // Blink phase variant.
        assert_eq!(
            AgentKind::Codex.title_state("[ . ] Action Required"),
            Some(AgentState::WaitingForInput)
        );
    }

    #[test]
    fn title_state_inconclusive_returns_none() {
        assert_eq!(AgentKind::Claude.title_state("✳ Claude Code"), None);
        assert_eq!(AgentKind::Codex.title_state("my-project"), None);
        assert_eq!(AgentKind::Claude.title_state(""), None);
        // Claude has no "Action Required" concept — the literal alone must not
        // trip its detector via the title path.
        assert_eq!(AgentKind::Claude.title_state("Action Required"), None);
        assert_eq!(AgentKind::Unknown.title_state("⠋ working"), None);
        // OpenCode's title is always "OpenCode" regardless of state, so title
        // alone is not conclusive.
        assert_eq!(AgentKind::OpenCode.title_state("OpenCode"), None);
    }

    // --- content_state: Claude ---

    #[test]
    fn claude_content_waiting_patterns() {
        assert_eq!(
            AgentKind::Claude.content_state("Some output\n  Allow tool? Esc to cancel\n"),
            AgentState::WaitingForInput
        );
        assert_eq!(
            AgentKind::Claude.content_state("Result\nNo, and tell Claude what to do differently\n"),
            AgentState::WaitingForInput
        );
        assert_eq!(
            AgentKind::Claude.content_state("Choose:\n❯ 1. Allow once\n  2. Allow always\n"),
            AgentState::WaitingForInput
        );
    }

    #[test]
    fn claude_content_idle() {
        // ❯ not followed by a digit = idle prompt, not a selection menu.
        assert_eq!(
            AgentKind::Claude.content_state("Done editing files.\n\n❯ \n"),
            AgentState::Idle
        );
        assert_eq!(AgentKind::Claude.content_state(""), AgentState::Idle);
    }

    #[test]
    fn claude_content_strips_ansi_before_matching() {
        assert_eq!(
            AgentKind::Claude.content_state("\x1B[1mAllow?\x1B[0m \x1B[33mEsc to cancel\x1B[0m\n"),
            AgentState::WaitingForInput
        );
    }

    // --- content_state: Codex ---

    #[test]
    fn codex_content_approval_markers_are_waiting() {
        for marker in CODEX_APPROVAL_MARKERS {
            let content = format!("codex output\n\n{marker}\n\n  Yes   No\n");
            assert_eq!(
                AgentKind::Codex.content_state(&content),
                AgentState::WaitingForInput,
                "marker {marker:?} should signal WaitingForInput"
            );
        }
    }

    #[test]
    fn codex_content_idle_when_no_marker() {
        assert_eq!(
            AgentKind::Codex.content_state("Edited src/main.rs\nDone.\n› \n"),
            AgentState::Idle
        );
        assert_eq!(AgentKind::Codex.content_state(""), AgentState::Idle);
    }

    #[test]
    fn codex_content_working_from_interrupt_hint() {
        // Real status-line shape captured from a live Codex session. The
        // interrupt hint is a durable Working signal independent of the
        // (user-configurable) terminal-title spinner.
        let content = "› Create a file…\n• Working (13s • esc to interrupt)\n";
        assert_eq!(AgentKind::Codex.content_state(content), AgentState::Working);
    }

    #[test]
    fn codex_content_approval_takes_precedence_over_working() {
        // If both a working hint and an approval question are visible, the
        // pending approval (needs-attention) must win.
        let content =
            "• Working (2s • esc to interrupt)\nWould you like to run the following command?\n";
        assert_eq!(
            AgentKind::Codex.content_state(content),
            AgentState::WaitingForInput
        );
    }

    #[test]
    fn codex_content_approval_footer_is_not_working() {
        // The approval footer reads "esc to cancel", not "esc to interrupt",
        // so it must not be mistaken for the working hint.
        let content = "Press enter to confirm or esc to cancel\n";
        assert_eq!(AgentKind::Codex.content_state(content), AgentState::Idle);
    }

    #[test]
    fn codex_content_strips_ansi_before_matching() {
        assert_eq!(
            AgentKind::Codex
                .content_state("\x1B[1mWould you like to run the following command?\x1B[0m\n"),
            AgentState::WaitingForInput
        );
    }

    #[test]
    fn unknown_content_is_idle() {
        assert_eq!(
            AgentKind::Unknown.content_state("Esc to cancel"),
            AgentState::Idle
        );
    }

    // --- content_state: OpenCode ---

    #[test]
    fn opencode_content_new_session_prompt_is_unknown() {
        // The brand-new-session prompt is not a general idle marker: after a
        // completed turn OpenCode leaves a blank prompt instead. Avoid using it
        // as an idle signal for hibernation.
        let content = "Ask anything... \"Fix broken tests\"\n  Build · GPT-5.5\n";
        assert_eq!(
            AgentKind::OpenCode.content_state(content),
            AgentState::Unknown
        );
    }

    #[test]
    fn opencode_content_working_from_interrupt_hint() {
        // Real active-task footer captured from OpenCode full and mini TUIs.
        let full = "⬝⬝⬝⬝⬝⬝⬝⬝  esc interrupt                         tab agents  ctrl+p commands\n";
        let mini =
            "BUILD  ⬝■■■■■■⬝ esc interrupt                                       ctrl+p cmd\n";
        assert_eq!(AgentKind::OpenCode.content_state(full), AgentState::Working);
        assert_eq!(AgentKind::OpenCode.content_state(mini), AgentState::Working);
    }

    #[test]
    fn opencode_content_permission_prompt_is_waiting() {
        // Real approval overlay captured from OpenCode when accessing `~/` from
        // a session sandboxed in another directory.
        let content = "⚠ Permission required\n← Access external directory ~\n\nPatterns\n\n- /home/si/*\n\nAllow once   Allow always   Reject\n";
        assert_eq!(
            AgentKind::OpenCode.content_state(content),
            AgentState::WaitingForInput
        );
    }

    #[test]
    fn opencode_content_permission_takes_precedence() {
        let content = "Permission required\nesc interrupt\n▣ Build · GPT-5.5 · 2.7s\n";
        assert_eq!(
            AgentKind::OpenCode.content_state(content),
            AgentState::WaitingForInput
        );
    }

    #[test]
    fn opencode_content_completed_turn_is_idle() {
        let content = "▣  Build · GPT-5.5 · 2.7s\n7.6K (1%) · $0.04  ctrl+p commands\n";
        assert_eq!(AgentKind::OpenCode.content_state(content), AgentState::Idle);
    }

    #[test]
    fn opencode_content_completed_turn_accepts_longer_duration() {
        let content = "▣  Build · GPT-5.5 · 1m 8.5s\n";
        assert_eq!(AgentKind::OpenCode.content_state(content), AgentState::Idle);
    }

    #[test]
    fn opencode_content_active_agent_line_without_duration_is_unknown() {
        // Active turns show the agent/model line before the footer appears, but
        // no duration. Do not call it idle unless the completed-turn duration is
        // present.
        let content = "▣  Build · GPT-5.5\n";
        assert_eq!(
            AgentKind::OpenCode.content_state(content),
            AgentState::Unknown
        );
    }

    #[test]
    fn opencode_content_unknown_when_ambiguous() {
        // No working or idle marker visible — return Unknown, not Idle, to
        // avoid false auto-hibernation.
        assert_eq!(
            AgentKind::OpenCode.content_state("Some intermediate TUI state\n"),
            AgentState::Unknown
        );
        assert_eq!(AgentKind::OpenCode.content_state(""), AgentState::Unknown);
    }

    #[test]
    fn opencode_content_tip_is_not_working() {
        // This text appears as a rotating idle tip, not as the active-task
        // footer's "esc interrupt" status signal.
        let content = "● Tip Press escape to stop the AI mid-response\n";
        assert_eq!(
            AgentKind::OpenCode.content_state(content),
            AgentState::Unknown
        );
    }

    #[test]
    fn opencode_content_strips_ansi_before_matching() {
        assert_eq!(
            AgentKind::OpenCode.content_state("\x1B[1mesc interrupt\x1B[0m  ctrl+p commands\n"),
            AgentState::Working
        );
    }

    // --- strip_ansi ---

    #[test]
    fn strip_ansi_removes_csi_sequences() {
        assert_eq!(strip_ansi("\x1B[31mred\x1B[0m text"), "red text");
    }

    #[test]
    fn strip_ansi_leaves_clean_text() {
        assert_eq!(strip_ansi("hello world"), "hello world");
    }
}
