---
name: claude-commander-delete-session
description: Delete a Claude Commander session. Use when the user says "delete a session", "remove a session", "clean up a session", "kill a session", "tear down a session", or asks you to get rid of an existing session.
arguments:
  - session
---

# Delete a Claude Commander Session

Permanently delete a Claude Commander session using the `claude-commander delete` CLI command (alias `rm`). This kills the session's tmux session, removes its git worktree, and drops it from state.

**This is destructive and irreversible.** There is no undo — the worktree and any uncommitted work in it are gone.

## Syntax

```bash
claude-commander delete <session> [--force]
# alias:
claude-commander rm <session> [--force]
```

## Options

`--force` deletes without confirmation and is required for any non-interactive call (see Guidelines). The complete flag list is in the `claude-commander-cli-reference` skill (generated from the CLI itself, so it always matches the installed version).

## Session resolution

Deletion uses **exact match only** — deliberately stricter than the read-only commands — to prevent accidentally deleting the wrong session:

- The `<session>` argument must be either the **exact session title** (case-insensitive) **or the full session ID**.
- Prefix matching is **not** supported. The 8-character ID prefix shown by `claude-commander list` is **not** accepted — use the full title or the full ID.
- If more than one session matches exactly (e.g. two sessions share a title), the command refuses with an ambiguity error and exits non-zero rather than guessing. Disambiguate with the full ID.
- An unknown session prints `Session not found` and exits 1.

## Confirmation guard

- With `-f`/`--force`: deletes immediately, no prompt.
- Interactive terminal without `--force`: prompts `Delete "<title>"? This kills its tmux session and removes the worktree. [y/N]`. Only `y`/`yes` (case-insensitive) proceeds; anything else aborts.
- Non-interactive stdin without `--force` (automation, or the persistent commander session): **refuses and exits 1**. Automated deletions MUST pass `--force`.

## Instructions

`$1` is the session the user wants to delete. Resolve it to the exact title or full ID before calling — do not pass a prefix.

## Examples

```bash
# Delete a finished session by exact title, no prompt
claude-commander delete "fix-auth-bug" --force

# Delete by full session ID (use when the title is ambiguous)
claude-commander rm 5f3c9a21-8b4e-4d77-9c2a-1e6f0b3d8a4c --force
```

## Guidelines

- **You are running non-interactively, so always pass `--force`.** Without it the command refuses on non-interactive stdin and exits 1. The confirmation prompt only appears for a human at an interactive terminal.
- **Reference the session by its exact title or full ID, never a prefix.** Prefix matching is intentionally disabled for delete. If you only know the short ID prefix from `claude-commander list`, look up the full title or ID first.
- If the command reports an ambiguity error, two sessions share that title — re-run with the full session ID to disambiguate.
- **Never delete a session unless the user explicitly asks you to.** Deletion is irreversible and destroys uncommitted work. Wait for a clear instruction like "delete that session", "clean up the fix-auth-bug session", or "tear it down".
- **If you cannot run `git` (your tools don't allow it), do not delete.** Without the checks below you can't tell whether work would be lost; tell the user to delete it from the TUI instead.
- **Because you pass `--force`, the CLI's safety prompt is skipped — so you must guard against data loss yourself.** Before deleting, check the session's worktree for work that exists nowhere else: uncommitted changes (`git -C <worktree> status --porcelain`) or commits on the session's branch not pushed to a remote (`git -C <worktree> log HEAD --not --remotes --oneline`). If either is non-empty, do NOT delete — surface exactly what would be lost and get explicit user confirmation that they want it gone anyway before proceeding.
- Confirm what you deleted after the command succeeds, and tell the user the work in that worktree is gone.
- Always use the long-form flag (`--force`) rather than `-f`, to avoid ambiguity.
