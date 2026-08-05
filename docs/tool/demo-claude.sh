#!/usr/bin/env bash
#
# Stand-in agent for the README screenshot fixture: paints a plausible Claude
# Code transcript into the pane, then parks. Never runs a real agent — the
# screenshots must be reproducible and must not spend tokens.
#
# The fixture links this in as `claude`, which is load-bearing twice over. The
# *name* is what makes the app treat the session as a Claude harness at all
# (`AgentKind::from_program` matches the program's basename exactly, and the state
# detector skips panes whose harness it doesn't recognise). And being launched as
# a bare `claude`, with no arguments, is what keeps the client's session header
# reading like a real setup instead of showing a temp-dir path.
#
# So the per-session detail arrives out of band: each session's row in the
# fixture's manifest, keyed by worktree directory name. The manifest sits next to
# the worktrees, which is the one path this script can derive from its own cwd —
# the app starts the agent *in* its worktree. The fixture writes a session's row
# just after creating it, so the wait below is normally over in a moment.
#
# Manifest row: <worktree name>\t<mode>\t<task>\t<detail>, where mode picks the
# state the detector will report:
#
#   working  — braille spinner in the pane title (`AgentKind::title_state`)
#   waiting  — permission prompt containing "Esc to cancel"
#              (`claude_content_state`)
#   idle     — ordinary transcript, no prompt markers
#
# Every line is kept under ~46 columns so the transcript doesn't soft-wrap in the
# narrowest place it is shown: the client's phone-width terminal.
set -euo pipefail

MANIFEST="$(dirname "$PWD")/.demo-agents"
KEY="$(basename "$PWD")"

MODE=idle
TASK="Working on the task"
DETAIL=""
for _ in $(seq 1 300); do
  if [ -f "$MANIFEST" ] && row="$(grep -m1 -F "$KEY"$'\t' "$MANIFEST" 2>/dev/null)"; then
    IFS=$'\t' read -r _ MODE TASK DETAIL <<<"$row"
    break
  fi
  sleep 0.1
done

d='\033[2m'   # dim
g='\033[32m'  # green
c='\033[36m'  # cyan
y='\033[33m'  # yellow
b='\033[1m'   # bold
r='\033[0m'   # reset

# Every line goes through `printf '%b'`, so the colour escapes live in the
# *argument* rather than in a format string (and a stray % in a task can't be
# read as a conversion).
say() { printf '%b\n' "$1"; }

# Pane title. tmux reads OSC 2; a braille frame here is what makes the state
# detector report Working without any content heuristics.
set_title() { printf '\033]2;%s\a' "$1"; }

say "${d}╭────────────────────────────────────────────╮${r}"
say "${d}│${r} ${c}✻${r} Welcome to ${b}Claude Code${r}                   ${d}│${r}"
say "${d}╰────────────────────────────────────────────╯${r}"
say ""
say "${d}>${r} ${TASK}"
say ""
say " ${g}●${r} ${b}Read${r}${d}(src/lib.rs)${r}"
say "   ${d}⎿  Read 214 lines${r}"
say " ${g}●${r} ${b}Grep${r}${d}(pattern: \"pool\", 6 files)${r}"
say "   ${d}⎿  Found 11 matches${r}"
if [ -n "$DETAIL" ]; then
  say " ${g}●${r} ${DETAIL}"
fi

case "$MODE" in
working)
  say " ${g}●${r} ${b}Edit${r}${d}(src/pool.rs)${r}"
  say "   ${d}⎿  +18 −4${r}"
  say ""
  say " ${c}⠹${r} Running tests… ${d}(esc to interrupt · 42s)${r}"
  set_title "⠹ claude"
  ;;
waiting)
  say ""
  say "${y}╭────────────────────────────────────────────╮${r}"
  say "${y}│${r} ${b}Bash command${r}                               ${y}│${r}"
  say "${y}│${r}                                            ${y}│${r}"
  say "${y}│${r}   cargo test --workspace                   ${y}│${r}"
  say "${y}│${r}   Run the full test suite                  ${y}│${r}"
  say "${y}│${r}                                            ${y}│${r}"
  say "${y}│${r} Do you want to proceed?                    ${y}│${r}"
  say "${y}│${r}  ${b}❯ 1. Yes${r}                                  ${y}│${r}"
  say "${y}│${r}    2. Yes, and don't ask again             ${y}│${r}"
  say "${y}│${r}    3. No, and tell Claude what             ${y}│${r}"
  say "${y}│${r}       to do differently (esc)              ${y}│${r}"
  say "${y}╰────────────────────────────────────────────╯${r}"
  say "  ${d}Esc to cancel${r}"
  set_title "claude"
  ;;
*)
  say " ${g}●${r} Done — ${d}3 files changed, tests pass.${r}"
  say ""
  say "${d}╭────────────────────────────────────────────╮${r}"
  say "${d}│${r} ${d}>${r} Try \"add a regression test\"              ${d}│${r}"
  say "${d}╰────────────────────────────────────────────╯${r}"
  say "  ${d}? for shortcuts${r}"
  set_title "claude"
  ;;
esac

# Park so the pane content stays put for the capture. Echo is turned off first:
# creating a stacked session injects its PR-base context into the pane, and an
# echoing terminal would paint that keystroke text over the transcript.
stty -echo -icanon 2>/dev/null || true
exec sleep infinity
