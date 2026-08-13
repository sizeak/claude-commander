//! Agent harness abstraction.
//!
//! Claude Commander launches different agent CLIs (Claude Code, OpenAI Codex,
//! OpenCode, Oh My Pi) inside tmux sessions. Each harness differs in how it is
//! resumed, whether it accepts a positional prompt, and what it renders in the
//! tmux pane while working or waiting for the user.
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

/// Claude Code's working status line can carry an elapsed-time and token-count
/// parenthetical after its ellipsis — `✽ Boondoggling… (5m 35s · ↓ 18.6k tokens ·
/// still thinking with high effort)`, `· Clauding… (4s · ↓ 4 tokens)`. Only a
/// turn that is actually in flight renders a *live* counter, so this is a safe
/// `Working` signal for the content fallback when the title check comes up empty
/// (a user can disable the terminal title, and Claude has changed its spinner
/// glyphs before now — see [`has_circle_spinner`]).
///
/// This fallback is deliberately **partial**, and the title check is the primary
/// signal. Two things constrain it:
///
/// - The parenthetical is not always present. On a long-running turn it is:
///   sampled 200× at 50ms against a live working pane, all 200 samples carried
///   it. On a short turn the line can be a bare `✽ Bunning…` with no
///   parenthetical at all (observed across 40 snapshots of one ~16s turn), which
///   this regex does not match. That is the accepted gap — see
///   `claude_content_bare_gerund_line_is_not_working`.
/// - Matching the bare `<glyph> <Gerund>…` line instead would be unsafe. Claude
///   renders tool lines in the same shape (`● Listing 1 directory… (ctrl+o to
///   expand)`) and they *persist in the transcript after completing*, so an idle
///   pane would read `Working` forever. Requiring a live duration *and* token
///   count is what rules those out; a finished turn's line reads
///   `✻ Cogitated for 15s`, with neither.
///
/// Receipt: every example line above captured verbatim from live panes
/// (claude-code 2.1.228). This version renders no `esc to interrupt` hint, so
/// unlike Codex there is no static string to match on instead.
static CLAUDE_WORKING_STATUS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"…\s*\(\d+(?:\.\d+)?(?:ms|s|m|h)\b[^)\n]*\btokens\b").expect("valid regex")
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

/// Oh My Pi renders one status row while a turn is in flight —
/// `⠴ Working… ⟦esc⟧`, or the running tool's progress message in place of
/// `Working…` (`⠇ Read existing glTF exporter patterns ⟨esc⟩`). Neither the
/// spinner nor the message is matchable (the message is arbitrary tool text),
/// but the trailing interrupt hint is: it is rendered only on that row, and the
/// row is dropped the moment the turn ends.
///
/// The bracket pair is glyph-mode dependent, so all three of omp's modes are
/// accepted — Unicode `⟦⟧` (U+27E6/U+27E7), Nerd Font `⟨⟩` (U+27E8/U+27E9) and
/// ASCII `[]`. The hint always ends its row, which is what keeps the ASCII form
/// from matching `[esc]` occurring mid-line in transcript prose or tool output.
///
/// Receipts (omp 17.2.15): the Unicode form captured from a local pane driven to
/// a live turn against a stalling mock endpoint; the Nerd Font form from the
/// user's working session on another host. The bracket set is omp's own
/// `format.bracketLeft`/`format.bracketRight` theme keys, whose three values are
/// exactly those pairs.
static OMP_WORKING_HINT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)(?:\u{27E6}esc\u{27E7}|\u{27E8}esc\u{27E9}|\[esc\])\s*$").expect("valid regex")
});

/// Pane-content substrings Oh My Pi renders in an overlay that is blocking on a
/// user decision. The tool-approval options cover a session whose title state
/// indicator has been turned off; the plan-review options cover the case the
/// title cannot report at all, because omp ends the turn (title state `idle`)
/// before showing its full-screen plan review.
///
/// Receipt (omp 17.2.15): both option sets are verbatim from omp's own bundle —
/// the approval options are its `allow_once`/`allow_always` option table, and
/// the plan-review options are the literal argument list passed to
/// `showPlanReview`. Neither survives the overlay being dismissed.
const OMP_ATTENTION_MARKERS: [&str; 4] = [
    // Tool-approval overlay options.
    "Allow once",
    "Always allow",
    // Plan-review overlay options.
    "Approve and execute",
    "Refine plan",
];

/// Strip ANSI escape sequences from a string.
pub fn strip_ansi(s: &str) -> String {
    ANSI_RE.replace_all(s, "").into_owned()
}

/// Whether `title` contains a braille spinner glyph (U+2800..U+28FF). The Codex
/// TUI animates a braille spinner in the terminal title while the model is
/// working. Older Claude Code builds did too, so the Claude arm of
/// [`AgentKind::title_state`] still accepts it alongside its current spinner.
fn has_braille_spinner(title: &str) -> bool {
    title.contains(|c: char| ('\u{2800}'..='\u{28FF}').contains(&c))
}

/// Whether `title` contains a quadrant-circle spinner glyph (U+25D0..U+25D3).
/// Current Claude Code animates `◐`/`◑` in the terminal title while a turn is in
/// flight and shows a static `✳` (U+2733) when idle, so the spinner is a clean
/// `Working` signal.
///
/// Receipt: claude-code 2.1.228 on Linux, sampled with `tmux display-message -p
/// '#{pane_title}'` 200× at 50ms against a long-running working session and 100×
/// at 200ms across a fresh session's first turn. Only U+25D0 and U+25D1 appeared
/// while working, and only U+2733 while idle. U+25D2/U+25D3 complete the same
/// four-frame set and are accepted for that reason, but were never observed.
///
/// This is why Claude sessions read `Idle` while working: the detector looked
/// only for [`has_braille_spinner`], which Claude Code no longer renders.
fn has_circle_spinner(title: &str) -> bool {
    title.contains(|c: char| ('\u{25D0}'..='\u{25D3}').contains(&c))
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
    /// Oh My Pi (`omp`).
    Omp,
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
        } else if name.eq_ignore_ascii_case("omp") {
            Self::Omp
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
    /// launch. Claude, Codex and Oh My Pi do (`claude '<prompt>'`,
    /// `codex '<prompt>'`, `omp '<prompt>'`); an unknown program (e.g. a bare
    /// shell) does not, so we must not append a prompt it would mis-parse.
    ///
    /// Receipt for omp (17.2.15): its `--help` documents a positional
    /// `MESSAGES` argument and the example `omp "List all .ts files in src/"`.
    pub fn accepts_positional_prompt(self) -> bool {
        matches!(self, Self::Claude | Self::Codex | Self::Omp)
    }

    /// Whether this harness accepts a `--model <name>` launch flag. Claude,
    /// Codex, OpenCode and Oh My Pi support it; an unknown program's flags are
    /// unconstrained.
    pub fn supports_model_flag(self) -> bool {
        matches!(
            self,
            Self::Claude | Self::Codex | Self::OpenCode | Self::Omp
        )
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
    ///
    /// Oh My Pi needs no delay either, verified rather than assumed: a single
    /// `send-keys '<text>' Enter` — one write carrying both — submitted 3/3
    /// against omp 17.2.15, each time leaving an empty composer and dispatching
    /// the turn.
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
    /// that must follow the binary, before its other flags; OpenCode and Oh My
    /// Pi append `--continue`.
    ///
    /// Receipt for omp (17.2.15): `--help` documents `-c, --continue  Continue
    /// previous session` and the example `omp --continue "What did we
    /// discuss?"`. Its `-r, --resume` takes an optional session id and opens a
    /// picker when given none, so it is not the flag to drive unattended.
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
            Self::OpenCode | Self::Omp => Some(match rest {
                Some(r) => format!("{binary} {r} --continue"),
                None => format!("{binary} --continue"),
            }),
            Self::Unknown => None,
        }
    }

    /// Detect agent state from the tmux pane *title*. Returns `Some` when the
    /// title alone is conclusive (so the caller can skip capturing pane
    /// content), `None` when content must be inspected.
    ///
    /// Returning `Some(Working)` on a spinner is only sound because a spinner
    /// title never coexists with a pending prompt: `AgentStateDetector::
    /// detect_fresh` takes a `Some` as final and never captures content, so a
    /// harness that kept spinning through an approval prompt would have its
    /// `WaitingForInput` masked as `Working` — a worse failure than the `Idle`
    /// bug this detector was fixed for.
    ///
    /// Receipt (claude-code 2.1.228): the title reverts from the `◐`/`◑` spinner
    /// to the idle `✳ <topic>` the moment a prompt appears. Measured two ways —
    /// 14/14 samples of a session driven to a mid-turn Bash approval prompt with
    /// `Esc to cancel` visible, and 5/5 live sessions found sitting at a prompt,
    /// none of which carried a spinner glyph.
    pub fn title_state(self, title: &str) -> Option<AgentState> {
        match self {
            // Codex prefixes the title with "Action Required" (no spinner) while
            // blocked on approval — check it before the shared spinner since the
            // two are mutually exclusive in Codex's title.
            Self::Codex if title.contains("Action Required") => Some(AgentState::WaitingForInput),
            // Claude animates a quadrant-circle spinner, Codex a braille one.
            // Both arms accept either glyph set: the two never collide with an
            // idle title (Claude's is `✳ <name>`, Codex's a bare project name),
            // so a harness changing its frames again degrades to the content
            // fallback rather than silently reporting Idle.
            Self::Claude | Self::Codex
                if has_circle_spinner(title) || has_braille_spinner(title) =>
            {
                Some(AgentState::Working)
            }
            Self::Omp => omp_title_state(title),
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
            Self::Omp => omp_content_state(&content),
            Self::Unknown => AgentState::Idle,
        }
    }
}

/// Claude content patterns: the last visible lines carry permission/selection
/// prompts when Claude is waiting for the user, and an in-flight turn's status
/// line when it is working. Waiting takes precedence over working, as it does
/// for Codex — a permission prompt can be up while the status line is still
/// rendered, and needs-attention is the more urgent read.
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

    // Working: the in-flight turn's status line, which sits directly above the
    // composer and so falls inside the same last-10-lines window.
    if lines
        .iter()
        .any(|line| CLAUDE_WORKING_STATUS_RE.is_match(line))
    {
        return AgentState::Working;
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

/// Oh My Pi's title is `π <state glyph> <label>`, where the label is the
/// auto-generated session title (falling back to the working directory's
/// basename) and the state glyph is one of exactly four values: a braille
/// spinner frame while a turn is in flight, `!` while blocked on the user, `>`
/// when idle, or `:` in place of the spinner on Windows. With the state
/// indicator turned off the title is `π: <label>` instead, carrying no state.
///
/// Receipt (omp 17.2.15): the glyph set and the `π <glyph> <label>` assembly are
/// from omp's own title builder, whose state is driven by three transitions —
/// `working` when a turn starts, `attention` when a tool call needs approval or
/// the `ask` tool runs, `idle` when a terminal turn ends. Live confirmations:
/// `π ⠼ omp-support-5e59cfe0` cycling the documented ten frames against a
/// stalling mock endpoint, and `π > omp-support-5e59cfe0` at an idle composer.
///
/// The title is *not* subject to omp's glyph-mode setting, unlike its pane
/// content: driven to ASCII glyph mode — where the same pane's status row
/// degrades to `| Working… [esc]` and its statusline brand to a literal `pi` —
/// the title still read `π > …` idle and `π ⠼ …` working.
///
/// Two glyphs are deliberately not treated as conclusive:
///
/// - `>` (idle) returns `None` rather than `Some(Idle)`, because omp ends the
///   turn *before* raising its full-screen plan review — so an idle title can
///   sit above an overlay that is waiting on the user. Falling through lets
///   [`omp_content_state`] see the overlay. It costs nothing: pane content is
///   captured through the same cache either way.
/// - A spinner frame reaches this function only when the pane title survived
///   tmux intact. tmux replaces every non-ASCII character of a pane title with
///   `_` when its server's locale is not UTF-8, so a working omp session can
///   report `_ _ <label>` — the brand *and* the spinner flattened. Reproduced on
///   tmux 3.6a: identical `printf` of `π ⠦ Probe` round-tripped byte-for-byte
///   under `LANG=en_GB.UTF-8` and came back `_ _ Probe` under `LANG=C` and under
///   an unset locale. That case is left to the content fallback, which is
///   unaffected (pane *cells* keep their UTF-8 regardless), rather than inferred
///   from the replacement character.
fn omp_title_state(title: &str) -> Option<AgentState> {
    let mut tokens = title.split_whitespace();
    // The brand prefix is a single character (`π`, or `_` once tmux has
    // flattened it). `π:` — the state-indicator-off form — is two, and so
    // declines here, as does any title an extension has overridden wholesale.
    let brand = tokens.next()?;
    if brand.chars().count() != 1 {
        return None;
    }
    match tokens.next()? {
        "!" => Some(AgentState::WaitingForInput),
        ":" => Some(AgentState::Working),
        glyph if has_braille_spinner(glyph) => Some(AgentState::Working),
        _ => None,
    }
}

/// Oh My Pi content patterns. A blocking overlay's options are rendered in the
/// visible pane, and the in-flight turn's status row carries the interrupt hint;
/// waiting takes precedence over working, as it does for the other harnesses.
///
/// Unlike OpenCode this falls back to `Idle` rather than `Unknown`, because omp
/// *does* have a durable idle signal: the status row exists only while a turn is
/// in flight and is dropped when it ends (verified — an idle pane carries no
/// interrupt hint at all). So "no working hint and no overlay" is a positive
/// idle reading rather than an absence of information.
fn omp_content_state(content: &str) -> AgentState {
    if OMP_ATTENTION_MARKERS
        .iter()
        .any(|marker| content.contains(marker))
    {
        return AgentState::WaitingForInput;
    }
    if OMP_WORKING_HINT_RE.is_match(content) {
        return AgentState::Working;
    }
    AgentState::Idle
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
    fn from_program_detects_omp() {
        assert_eq!(AgentKind::from_program("omp"), AgentKind::Omp);
        assert_eq!(AgentKind::from_program("omp --model opus"), AgentKind::Omp);
        assert_eq!(AgentKind::from_program("OMP"), AgentKind::Omp);
        assert_eq!(AgentKind::from_program("/usr/bin/omp"), AgentKind::Omp);
    }

    #[test]
    fn from_program_unknown_for_others() {
        assert_eq!(AgentKind::from_program("bash"), AgentKind::Unknown);
        assert_eq!(AgentKind::from_program(""), AgentKind::Unknown);
        // Different binary names that merely contain the substring must not match.
        assert_eq!(AgentKind::from_program("claude-code"), AgentKind::Unknown);
        assert_eq!(AgentKind::from_program("codex-cli"), AgentKind::Unknown);
        assert_eq!(AgentKind::from_program("opencode-ai"), AgentKind::Unknown);
        assert_eq!(AgentKind::from_program("ompx"), AgentKind::Unknown);
        assert_eq!(AgentKind::from_program("omp-wrapper"), AgentKind::Unknown);
    }

    // --- capability flags ---

    #[test]
    fn accepts_positional_prompt_for_agents_only() {
        assert!(AgentKind::Claude.accepts_positional_prompt());
        assert!(AgentKind::Codex.accepts_positional_prompt());
        // OpenCode's plain `opencode` does not accept a positional prompt;
        // prompts are passed via `opencode run [message..]` instead.
        assert!(!AgentKind::OpenCode.accepts_positional_prompt());
        // omp takes a positional `MESSAGES` argument, like Claude and Codex.
        assert!(AgentKind::Omp.accepts_positional_prompt());
        assert!(!AgentKind::Unknown.accepts_positional_prompt());
    }

    #[test]
    fn is_claude_only_for_claude() {
        assert!(AgentKind::Claude.is_claude());
        assert!(!AgentKind::Codex.is_claude());
        assert!(!AgentKind::OpenCode.is_claude());
        assert!(!AgentKind::Omp.is_claude());
        assert!(!AgentKind::Unknown.is_claude());
    }

    #[test]
    fn supports_model_flag_for_agents_only() {
        assert!(AgentKind::Claude.supports_model_flag());
        assert!(AgentKind::Codex.supports_model_flag());
        assert!(AgentKind::OpenCode.supports_model_flag());
        assert!(AgentKind::Omp.supports_model_flag());
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
        assert_eq!(AgentKind::Omp.submit_key_delay(), None);
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

    #[test]
    fn resume_command_omp_appends_continue() {
        assert_eq!(
            AgentKind::Omp.resume_command("omp"),
            Some("omp --continue".to_string())
        );
        assert_eq!(
            AgentKind::Omp.resume_command("omp --model opus"),
            Some("omp --model opus --continue".to_string())
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
    fn title_state_working_claude_circle_spinner() {
        // Claude Code animates a half-filled-circle spinner in the pane title
        // while a turn is in flight, not a braille one. Captured verbatim from
        // live panes (claude-code 2.1.228); without these frames every Claude
        // session's title reads inconclusive and the session shows as Idle.
        assert_eq!(
            AgentKind::Claude.title_state("◐ audio-cleanup-video"),
            Some(AgentState::Working)
        );
        assert_eq!(
            AgentKind::Claude.title_state("◑ audio-cleanup-video"),
            Some(AgentState::Working)
        );
    }

    #[test]
    fn title_state_accepts_either_spinner_set_for_both_harnesses() {
        // Deliberate: each harness accepts the other's frames too, so a harness
        // changing its spinner again degrades to the content fallback instead of
        // silently reporting Idle (the failure this detector was fixed for).
        // Neither glyph set can collide with an idle title — Claude's is
        // `✳ <name>`, Codex's a bare project name — so the widening is free.
        // Codex has never been observed rendering a circle frame; this pins the
        // behaviour as intended rather than incidental.
        assert_eq!(
            AgentKind::Codex.title_state("◐ my-project"),
            Some(AgentState::Working)
        );
        assert_eq!(
            AgentKind::Claude.title_state("⠹ feature-branch"),
            Some(AgentState::Working)
        );
        // Endpoints of the accepted quadrant-circle run. `◒`/`◓` complete the
        // four-frame set but were never observed in 2.1.228 — pinned so the
        // range boundary can't silently narrow.
        assert_eq!(
            AgentKind::Claude.title_state("◒ x"),
            Some(AgentState::Working)
        );
        assert_eq!(
            AgentKind::Claude.title_state("◓ x"),
            Some(AgentState::Working)
        );
        // Just outside the range on both sides.
        assert_eq!(AgentKind::Claude.title_state("● x"), None);
        assert_eq!(AgentKind::Claude.title_state("◔ x"), None);
    }

    #[test]
    fn title_state_claude_idle_asterisk_is_not_working() {
        // The static `✳` is what an idle Claude title carries — it must stay
        // inconclusive so the content fallback gets a say, and must never be
        // mistaken for the spinner.
        assert_eq!(AgentKind::Claude.title_state("✳ extract-tui-crate"), None);
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

    // --- title_state: omp ---

    #[test]
    fn title_state_omp_working_across_every_spinner_frame() {
        // omp's ten braille frames, verbatim from its title builder and
        // observed cycling in a live pane. Every one must read Working, or the
        // state flickers between Working and Idle at 80ms.
        for frame in [
            '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}',
            '\u{2827}', '\u{2807}', '\u{280F}',
        ] {
            let title = format!("\u{3C0} {frame} omp-support-5e59cfe0");
            assert_eq!(
                AgentKind::Omp.title_state(&title),
                Some(AgentState::Working),
                "frame {frame:?} should signal Working"
            );
        }
    }

    #[test]
    fn title_state_omp_attention_is_waiting() {
        // `!` replaces the spinner while omp is blocked on a tool approval or
        // the `ask` tool. Captured shape: `π ! <session title>`.
        assert_eq!(
            AgentKind::Omp.title_state("\u{3C0} ! Promote model loaders to api"),
            Some(AgentState::WaitingForInput)
        );
    }

    #[test]
    fn title_state_omp_windows_colon_glyph_is_working() {
        // On Windows omp substitutes a static `:` for the animated spinner.
        assert_eq!(
            AgentKind::Omp.title_state("\u{3C0} : omp-support"),
            Some(AgentState::Working)
        );
    }

    #[test]
    fn title_state_omp_idle_glyph_falls_through_to_content() {
        // `>` is omp's idle glyph, but it is deliberately inconclusive: omp ends
        // the turn before raising its full-screen plan review, so an idle title
        // can sit above an overlay that is waiting on the user. Returning
        // `Some(Idle)` here would skip the content capture that spots it.
        assert_eq!(
            AgentKind::Omp.title_state("\u{3C0} > omp-support-5e59cfe0"),
            None
        );
    }

    #[test]
    fn title_state_omp_ignores_punctuation_in_the_label() {
        // Only the glyph *position* may be read. A session title ending in `!`
        // under the idle glyph must not be mistaken for the attention glyph —
        // matching `!` anywhere in the title would report WaitingForInput for an
        // idle session forever.
        assert_eq!(
            AgentKind::Omp.title_state("\u{3C0} > Fix the parser!"),
            None
        );
        // And a label that merely starts with `!` sits in the label position, so
        // it cannot reach the glyph match either.
        assert_eq!(
            AgentKind::Omp.title_state("\u{3C0} > !important cleanup"),
            None
        );
    }

    #[test]
    fn title_state_omp_state_indicator_disabled_is_inconclusive() {
        // With the state indicator off omp renders `π: <label>` — the brand and
        // a colon as one token, carrying no state. It must not be read as the
        // Windows `:` working glyph.
        assert_eq!(
            AgentKind::Omp.title_state("\u{3C0}: omp-support-5e59cfe0"),
            None
        );
    }

    #[test]
    fn title_state_omp_survives_tmux_flattening_the_brand() {
        // tmux replaces non-ASCII in a pane title with `_` when its server's
        // locale is not UTF-8 (reproduced on tmux 3.6a under `LANG=C`), so the
        // `π` brand can arrive as `_`. The ASCII glyphs still land, and this is
        // the shape the user's own working session reports.
        assert_eq!(
            AgentKind::Omp.title_state("_ ! Promote model loaders to api"),
            Some(AgentState::WaitingForInput)
        );
        // The spinner flattens to `_` too. That is left to the content fallback
        // rather than inferred from tmux's replacement character.
        assert_eq!(
            AgentKind::Omp.title_state("_ _ Promote model loaders to api"),
            None
        );
    }

    #[test]
    fn title_state_omp_needs_both_a_brand_and_a_glyph() {
        assert_eq!(AgentKind::Omp.title_state(""), None);
        assert_eq!(AgentKind::Omp.title_state("\u{3C0}"), None);
        // A shell prompt left behind after omp exits must not parse as a state.
        assert_eq!(AgentKind::Omp.title_state("si@togusa:~/Projects"), None);
        // Another harness's title must not either.
        assert_eq!(
            AgentKind::Omp.title_state("\u{2733} extract-tui-crate"),
            None
        );
    }

    #[test]
    fn title_state_omp_glyphs_are_inert_for_other_harnesses() {
        // The omp parser is reached only via the Omp arm — `!` and `>` carry no
        // meaning in Claude's or Codex's titles.
        assert_eq!(AgentKind::Claude.title_state("\u{3C0} ! working"), None);
        assert_eq!(AgentKind::Codex.title_state("\u{3C0} > my-project"), None);
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
    fn claude_content_working_from_status_line() {
        // Both lines captured verbatim from live working panes (claude-code
        // 2.1.228): a long-running turn and a fresh session's first turn. The
        // elapsed/token parenthetical is the durable Working signal for the
        // content fallback — this version renders no `esc to interrupt` hint.
        let long_run = "  ⎿  Read src/main.rs\n\n✽ Boondoggling… (5m 35s · ↓ 18.6k tokens · still thinking with high effort)\n\n─── audio-cleanup-video ──\n❯ \n";
        let first_turn = "· Clauding… (4s · ↓ 4 tokens)\n\n❯ \n";
        assert_eq!(
            AgentKind::Claude.content_state(long_run),
            AgentState::Working
        );
        assert_eq!(
            AgentKind::Claude.content_state(first_turn),
            AgentState::Working
        );
    }

    #[test]
    fn claude_content_waiting_takes_precedence_over_working() {
        // A permission prompt can be up while the status line is still
        // rendered; needs-attention must win, as it does for Codex.
        let content = "✽ Boondoggling… (12s · ↓ 900 tokens)\nAllow tool? Esc to cancel\n";
        assert_eq!(
            AgentKind::Claude.content_state(content),
            AgentState::WaitingForInput
        );
    }

    #[test]
    fn claude_content_bare_gerund_line_is_not_working() {
        // Known, accepted gap in the content fallback: a short turn can render
        // the status line with no elapsed/token parenthetical. Matching the bare
        // `<glyph> <Gerund>…` shape instead would be unsafe, because completed
        // tool lines keep that shape in the transcript forever — so this reads
        // Idle and the title check is what covers the case. Captured verbatim
        // from a live pane mid-turn (claude-code 2.1.228).
        assert_eq!(
            AgentKind::Claude.content_state("✽ Bunning…\n\n❯ \n"),
            AgentState::Idle
        );
    }

    #[test]
    fn claude_content_finished_turn_and_tool_lines_are_not_working() {
        // The lines that persist in the transcript after a turn ends must never
        // read Working — this is what the duration+tokens requirement buys.
        // Both captured verbatim from live panes.
        assert_eq!(
            AgentKind::Claude.content_state("✻ Cogitated for 15s\n\n❯ \n"),
            AgentState::Idle
        );
        assert_eq!(
            AgentKind::Claude.content_state("● Listing 1 directory… (ctrl+o to expand)\n\n❯ \n"),
            AgentState::Idle
        );
    }

    #[test]
    fn claude_content_prose_ellipsis_is_not_working() {
        // Requiring both an elapsed duration and the token count keeps ordinary
        // transcript prose from being read as an in-flight turn.
        assert_eq!(
            AgentKind::Claude.content_state("Compiling… (2s)\nDone.\n❯ \n"),
            AgentState::Idle
        );
    }

    #[test]
    fn claude_content_strips_ansi_before_matching() {
        assert_eq!(
            AgentKind::Claude.content_state("\x1B[1mAllow?\x1B[0m \x1B[33mEsc to cancel\x1B[0m\n"),
            AgentState::WaitingForInput
        );
        // The Working path needs the same guarantee: Claude colours the status
        // line, so the regex would never match un-stripped input.
        assert_eq!(
            AgentKind::Claude
                .content_state("\x1B[36m✽ Boondoggling…\x1B[0m \x1B[2m(4s · ↓ 4 tokens)\x1B[0m\n"),
            AgentState::Working
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

    // --- content_state: omp ---

    #[test]
    fn omp_content_working_from_interrupt_hint_in_every_glyph_mode() {
        // All three captured verbatim from live working panes (omp 17.2.15).
        // Unicode is the default glyph mode; ASCII was reached by driving the
        // setup wizard; the Nerd Font row is from the user's own session, and
        // shows the status row carrying a running tool's progress message in
        // place of `Working…`.
        let unicode = " \u{2834} Working\u{2026} \u{27E6}esc\u{27E7}\n";
        let ascii = " | Working\u{2026} [esc]\n";
        let nerd = " \u{2826} Read existing glTF exporter patterns \u{27E8}esc\u{27E9}\n";
        for content in [unicode, ascii, nerd] {
            assert_eq!(
                AgentKind::Omp.content_state(content),
                AgentState::Working,
                "content {content:?} should signal Working"
            );
        }
    }

    #[test]
    fn omp_content_working_hint_is_found_above_the_statusline() {
        // The status row is not the last row of the pane — omp's statusline is
        // drawn below it. This is the real capture, and it is what the
        // end-of-*line* anchor buys over an end-of-input one.
        let content = concat!(
            " count to ten\n",
            " \u{2834} Working\u{2026} \u{27E6}esc\u{27E7}\n",
            "\u{256D}\u{2500}\u{2500} \u{3C0}  > \u{2B22} Sonnet 5 \u{B7} \u{25D2} high > ",
            "\u{1F333} claude-commander/omp-support \u{2500}\u{2500}\u{256E}\n",
            "\u{2570}\u{2500} \u{2500}\u{256F}\n",
        );
        assert_eq!(AgentKind::Omp.content_state(content), AgentState::Working);
    }

    #[test]
    fn omp_content_idle_when_the_status_row_is_gone() {
        // omp drops the status row the moment the turn ends, so an idle pane
        // carries no interrupt hint. Verified against a live idle composer: zero
        // occurrences of the hint anywhere in the visible pane.
        let content = concat!(
            " Tip: Ctrl+D can be used to exit, but with your draft saved!\n",
            " \u{2718} 401 {\"type\":\"error\"}\n",
            " Dismissed when you send your next message.\n",
            "\u{256D}\u{2500}\u{2500} \u{3C0}  > \u{2B22} Sonnet 5 \u{B7} \u{25D2} high ",
            "\u{2500}\u{2500}\u{256E}\n",
        );
        assert_eq!(AgentKind::Omp.content_state(content), AgentState::Idle);
        assert_eq!(AgentKind::Omp.content_state(""), AgentState::Idle);
    }

    #[test]
    fn omp_content_mid_line_ascii_hint_is_not_working() {
        // In ASCII glyph mode the hint is a bare `[esc]`, which can plausibly
        // occur in transcript prose or tool output. Requiring it to end its row
        // is what keeps that from reading as an in-flight turn.
        assert_eq!(
            AgentKind::Omp.content_state("Press [esc] twice to clear the composer.\n"),
            AgentState::Idle
        );
    }

    #[test]
    fn omp_content_overlay_options_are_waiting() {
        for marker in OMP_ATTENTION_MARKERS {
            let content = format!("omp output\n\n  \u{276F} {marker}\n");
            assert_eq!(
                AgentKind::Omp.content_state(&content),
                AgentState::WaitingForInput,
                "marker {marker:?} should signal WaitingForInput"
            );
        }
    }

    #[test]
    fn omp_content_waiting_takes_precedence_over_working() {
        // The approval overlay is drawn over a pane whose status row is still
        // up; needs-attention must win, as it does for the other harnesses.
        let content =
            " \u{2834} Working\u{2026} \u{27E6}esc\u{27E7}\n  Allow once   Always allow   Reject\n";
        assert_eq!(
            AgentKind::Omp.content_state(content),
            AgentState::WaitingForInput
        );
    }

    #[test]
    fn omp_content_strips_ansi_before_matching() {
        // omp colours both the status row and the overlay, so the regex would
        // never see a clean end-of-line without the strip.
        assert_eq!(
            AgentKind::Omp.content_state(
                "\x1B[36m \u{2834} Working\u{2026}\x1B[0m \x1B[2m\u{27E6}esc\u{27E7}\x1B[0m\n"
            ),
            AgentState::Working
        );
    }

    #[test]
    fn omp_content_patterns_are_inert_for_other_harnesses() {
        // omp's hint must not leak into the other harnesses' detectors.
        assert_eq!(
            AgentKind::Codex.content_state(" \u{2834} Working\u{2026} \u{27E6}esc\u{27E7}\n"),
            AgentState::Idle
        );
        assert_eq!(
            AgentKind::OpenCode.content_state(" \u{2834} Working\u{2026} \u{27E6}esc\u{27E7}\n"),
            AgentState::Unknown
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
