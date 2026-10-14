---
name: claude-commander-monitor-sessions
description: Inspect and monitor Claude Commander sessions. Use when the user says "list sessions", "what are my sessions doing", "check on a session", "session status", "show session output", "tail the session log", "is the session stuck", or "monitor sessions".
arguments:
  - session
---

# Monitor Claude Commander Sessions

The commander's toolkit for tracking delegated work. Use these three read-only `claude-commander` commands to follow up on sessions you've spun up: `list` to see what exists, `status` to check a specific session's progress and diff, and `log` to read what it has actually been doing.

All three are read-only and safe to run at any time — they never modify a session. Their full flag lists are in the `claude-commander-cli-reference` skill (generated from the CLI itself, so it always matches the installed version).

## Picking a session

`status` and `log` take a `<session>` argument that matches an **exact title** (case-insensitive) or an **ID** — the full ID, or a prefix of the 8-character ID that `list` shows. Title prefixes don't match: `claude-commander status fix-au` won't find `fix-auth-bug`. ID prefixes are allowed here because these commands are read-only (unlike `delete`, which needs an exact title or full ID). If two IDs share your prefix you get one of them, so use the full 8 characters.

## list — list all sessions

```bash
claude-commander list [options]
```

The JSON output is a machine-readable list of every session with its title, ID, status, and related metadata — use it to discover which sessions exist and pick one to inspect further.

## status &lt;session&gt; — detailed status of one session

```bash
claude-commander status <session> [options]
```

Reports the detailed state of a single session. The `--json` output additionally includes a git diffstat summary, so you can see how much work the session has produced (files changed, insertions/deletions) without attaching to it.

## log &lt;session&gt; — dump recent terminal output

```bash
claude-commander log <session> [options]
```

Dumps the recent terminal output (scrollback) for a session. Use this to see what a delegated session has been doing, or to tell whether it has finished, errored, or is stuck waiting for input.

## Instructions

`$ARGUMENTS` is the session the user wants to inspect, when they name one. Resolve it against session titles or IDs. If the user hasn't named a session (e.g. "what are my sessions doing?"), start with `list` to discover what exists, then drill in with `status` or `log`.

## Examples

```bash
# List active sessions
claude-commander list

# List every session, including stopped ones, as JSON for parsing
claude-commander list --all --json

# Check a specific session's status and diff summary (JSON for programmatic use)
claude-commander status fix-auth-bug --json

# Quick human-readable status by ID prefix
claude-commander status 3f2a

# See what a delegated session has been doing recently
claude-commander log design-api

# Capture more scrollback to debug a session that seems stuck
claude-commander log fix-auth-bug --lines 500
```

## Guidelines

- These commands are read-only and safe to run anytime — reach for them freely to check in on delegated work.
- When you (an agent) need to parse the output, always pass `--json` to `list` and `status`. Parse the structured fields rather than scraping the human-readable text.
- `status --json` includes a git diffstat, making it the best way to gauge how much a session has accomplished without attaching to its tmux session.
- Use `log` when a session appears idle or finished: read the recent output to determine whether it completed, hit an error, or is blocked waiting for input. Increase `--lines` if the relevant output has scrolled off.
- `<session>` accepts an exact title or an ID (full, or a prefix of the 8-character ID). (This is unlike `delete`, which requires an exact title or full ID.)
- Always use long-form flags (`--all`, `--json`, `--lines`) for clarity.
