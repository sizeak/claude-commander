---
name: claude-commander-new-session
description: Spin up a new Claude Commander session. Use when the user says "spin up a session", "hand this off", "create a new session", "start a new session", "delegate this", or asks you to work on something in a separate session.
arguments:
  - instructions
---

# New Claude Commander Session

Spin up a new Claude Commander session using the `claude-commander new` CLI command. Each session gets its own git worktree, branch, and tmux session.

## Syntax

```bash
claude-commander new <session-name> [options]
```

## Options

**From a chat request** (your prompt starts "You are answering a … mention"),
`new` is restricted: name the project with `--project` (see
`claude-commander projects`), and don't use `--path`, `--base-branch`,
`--workspace` or `--remote`; pass the `--origin-*` flags from your prompt
header. The worker reports back with `claude-commander notify`, so skip the tmux
report-back block below.

The complete flag list for `new` is in the `claude-commander-cli-reference` skill (generated from the CLI itself, so it always matches the installed version). The ones you'll use most are `--initial-prompt`, `--path`, `--base-branch`, `--mode` and `--effort`.

## Output

The command produces verbose logging. On success, report only a single line confirming the session was created and its name. Do not include the full command output, session ID, or attach instructions.

Example response: `Spun up session **fix-auth-bug** in plan mode.`

## Instructions

`$ARGUMENTS` is the user's request describing what the new session should do. Use it to determine the session name, initial prompt, mode, effort, and base branch.

## Give the new session a way to report back

A spawned session has **no shared context** with yours and no built-in channel back to you. If you want the child to be able to "post back" (status, results, questions) to the session that spawned it, you must tell it **who spawned it** and **how to reach it**, by embedding that in the child's `--initial-prompt`.

This works because every Claude Commander session runs in a pane on one **shared tmux server**, so a child can write directly into the parent's input with `tmux send-keys`.

**1. Capture your own identity first** (you are the spawning session, running inside tmux):

```bash
# Only meaningful if you are yourself inside a tmux/commander session:
[ -n "$TMUX" ] || echo "not in tmux — skip the report-back block"
PARENT_PANE="$TMUX_PANE"                        # e.g. %34 — globally-unique, stable send-keys target
PARENT_TMUX="$(tmux display-message -p '#S')"   # e.g. cc-24a723c7 — human-readable session name
```

`$TMUX_PANE` is the reliable target (a pane id is unique and stable for the pane's life). The `cc-XXXXXXXX` session name is `cc-` + the first 8 chars of the Commander session id, so it also resolves with `claude-commander status <XXXXXXXX>`.

**2. Append a report-back block to the child's `--initial-prompt`**, after the actual task, substituting the captured values:

```
---
You were spawned by a parent Claude Commander session. If — and ONLY if — the user
explicitly asks you to report or post something back to the session that spawned you,
deliver a ONE-LINE message to the parent's input:

    tmux send-keys -t <PARENT_PANE> -l '[from <your-session-title>] YOUR ONE-LINE MESSAGE'
    tmux send-keys -t <PARENT_PANE> Enter

Parent tmux pane:    <PARENT_PANE>      (e.g. %34 — use this as the send-keys target)
Parent tmux session: <PARENT_TMUX>      (e.g. cc-24a723c7 — human-readable / maps to `claude-commander status`)

Keep it to a single line, prefix it so it's recognisable, and do this ONLY on explicit
user request — never unprompted.
```

Only include this block when you are inside tmux (`$TMUX` set). If you are not in a tmux/commander session there is nothing to post back to, so omit it.

## Examples

```bash
# Create a session that starts working immediately
claude-commander new "fix-auth-bug" \
  --initial-prompt "There is an auth bypass in src/login.rs where the session token is not validated. Fix it and add a regression test."

# Create a session in plan mode with high effort
claude-commander new "design-api" \
  --initial-prompt "Design the new REST API for user management." \
  --mode plan --effort high

# Fork from a specific branch (stacks automatically if branch belongs to a session)
claude-commander new "feature-follow-up" \
  --base-branch feature-auth

# Minimal — just create a session with no initial prompt
claude-commander new "exploratory-work"

# Spawn a session that can report back to you via tmux (see "Give the new session a way to report back")
PARENT_PANE="$TMUX_PANE"                        # e.g. %34
PARENT_TMUX="$(tmux display-message -p '#S')"   # e.g. cc-24a723c7
claude-commander new "run-migration" \
  --initial-prompt "Run the DB migration in db/migrations and report the outcome.

---
You were spawned by a parent Claude Commander session. If — and ONLY if — the user asks you
to report back, deliver a one-line message to the parent's input:
    tmux send-keys -t ${PARENT_PANE} -l '[from run-migration] MESSAGE'
    tmux send-keys -t ${PARENT_PANE} Enter
Parent tmux pane: ${PARENT_PANE}   Parent tmux session: ${PARENT_TMUX}
Do this only on explicit user request — never unprompted."
```

## Guidelines

- Write clear, self-contained prompts. The new session has no shared context with yours — include file paths, the problem description, and what the expected outcome is.
- When using `--mode plan`, include an explicit instruction in the prompt such as "You MUST use plan mode. Do not make any edits until the plan is approved." The `--mode` flag sets the Claude CLI permission mode, but the agent may still attempt edits unless the prompt reinforces the constraint.
- Keep session names short and descriptive (they become branch names).
- Always use long-form flags (`--initial-prompt`, `--mode`, `--effort`), never short flags, to avoid ambiguity.
- **Never spin up a new session unless the user explicitly asks you to.** Do not proactively create sessions, even if you think it would be helpful. Wait for a clear instruction like "spin up a session for this", "hand this off", or "create a new session".
- When you do create a session, always tell the user what you're doing and why.
- If the user wants the new session to be able to report back to you, capture `$TMUX_PANE` / `#S` and embed the report-back block in the initial prompt (see "Give the new session a way to report back"). Instruct the child to post back **only on explicit user request**, so it never spams your input unprompted.
