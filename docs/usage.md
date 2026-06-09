# Usage

Deep usage guide for Claude Commander. For installation and the keyboard /
status-symbol reference, see the [README](../README.md). For configuration
options, see [Configuration](configuration.md).

## Interactive TUI (default)

```bash
claude-commander
```

## Commands

```bash
# List active sessions (add --all to include stopped, --json for machine-readable output)
claude-commander list

# Create a new session
claude-commander new "feature-auth" --path /path/to/repo

# Create a session with an initial prompt (starts working immediately)
claude-commander new "fix-auth" --initial-prompt "Fix the auth bypass in login.rs" --effort high

# Create a session in plan mode, forking from a specific branch
claude-commander new "feature-api" --base-branch develop --mode plan

# Create a session placed directly in a specific section
claude-commander new "feature-ui" --section "Needs Review"

# Attach to a session
claude-commander attach feature-auth

# Dump recent terminal output from a session (default 100 lines, max 10000)
claude-commander log feature-auth --lines 200

# Show configuration
claude-commander config

# Initialize config file
claude-commander config --init

# Use a custom config file
claude-commander --config /path/to/config.toml
```

## Session List

The left pane shows projects and their worktree sessions in a tree view. Projects are sorted alphabetically. Sessions within a project are sorted newest first (by creation time).

Each session row shows the title and, in `[brackets]`, the branch name — but only when the branch differs from what the title would sanitize to. A session titled "Feature Auth" with branch `feature-auth` (or `prefix/feature-auth` when `branch_prefix` is set) renders as just `Feature Auth`; the bracket reappears only when the branch carries new information, e.g. you renamed it to `feature-auth-v2` outside the app.

### PR Stacks

When a session's PR targets another session's branch (rather than `main`), the two form a stack. Stacked children render one indent deeper and sit directly beneath the session they're stacked on, in bottom-to-top stack order. The stack base keeps its normal position in the root session list sorted by creation time.

Press `t` on any session in a stack to create a new session on top of that stack — regardless of which member you have selected, the new branch is forked from the topmost session. When you launch Claude in the new session it is told to use `gh pr create --base <parent-branch>` so the PR targets the right place automatically.

Stacks are detected from the PR's `baseRefName` returned by the `gh` CLI, so they stay accurate across GitHub's auto-retargeting when a stack member is merged.

Stack grouping is only active in the default project-grouped view. When [Session List Sections](configuration.md#session-list-sections) is configured, sessions are ordered by their section instead and stacked children render at the normal indent — the `t` hotkey and `stack_parent_session_id` still work, but a base and its child may land in different sections depending on their PR state.

#### Cascade merge main through a stack

When `main` moves forward, use **Cascade merge main** from the command palette to propagate it up the stack in one step: the command merges `main` into the stack base, then the base into its child, and so on to the leaf. Running it from any session in a stack works — the cascade always starts from the base.

Before touching any worktree, the cascade fetches `origin`, verifies no live agent in the stack is `Working` or `WaitingForInput` (racing a `git merge` against Claude writing files is unrecoverable), and refuses if any worktree has uncommitted changes. Each session shows a `⟳` spinner while its step is running.

On the first conflict the cascade pauses: the affected session gets a persistent `⏸` glyph (survives a restart of the TUI), and the worktree is left in the usual `git merge` in-progress state so you can resolve it however you like — typically by attaching to the session and asking the live Claude. Once you've committed the resolved merge, **Cascade resume** from the palette picks up where it stopped and propagates the new commit on up the chain. **Cascade abandon** clears the pause without continuing, if you decide to back out.

#### Push stack

**Push stack** (palette) runs `git push -u origin <branch>` across every session in the stack, base first then each child up the chain — pushing the base before its children keeps GitHub's PR base refs consistent. Each session shows the spinner glyph while its own push is in flight.

Pre-flight is the same as cascade merge: no live agent may be `Working` or `WaitingForInput`, and worktrees must have no uncommitted changes. On the first `git push` failure (rejection, auth, non-fast-forward, etc.) the chain stops and the toast shows git's stderr — no "resume" command is needed since `git push` is idempotent, so fix the root cause and re-run **Push stack** to continue.

## AI Summary

The Info pane can display an AI-generated summary of branch changes, powered
by the Claude CLI. Press `g` (configurable) while viewing the Info pane to
generate a summary. Summaries are cached per-session — once generated, they
display instantly when you revisit the session. Press `g` again to regenerate
after making changes.

Requires the `claude` CLI to be installed and authenticated. If unavailable,
the summary section shows a placeholder instead.

| Setting | Default | Description |
|---------|---------|-------------|
| `ai_summary_enabled` | `true` | Set to `false` to disable AI summaries entirely |
| `ai_summary_model` | `claude-haiku-4-5-20251001` | Claude model used for summaries (Haiku recommended for cost efficiency) |

Environment variable overrides: `CC_AI_SUMMARY_ENABLED`, `CC_AI_SUMMARY_MODEL`.

## Reviewing & commenting on changes

Press **`i`** on a selected session (or open the **command palette** with
`Space` and run **"Review diff & comment"**) to open a full-screen review view. It shows everything a PR
against the session's base branch would contain — committed, staged, unstaged,
and untracked changes — composed from `merge-base(base, HEAD)` through the
working tree.

The diff is rendered in a `lumen`/`hunk`-style colour scheme: dark green/red
line fills, a brighter highlight on the changed span within a line, and (on
true-color terminals) syntax highlighting of the code. It degrades to
foreground-only colouring on 256- and 16-colour terminals.

- **Navigate**: the changed files are shown as a collapsible **tree** (lazygit
  style — single-child directory chains are compressed). `Tab` switches focus
  between the tree and the diff body; in the tree, `↑↓`/`jk` move and `Enter`
  expands/collapses a directory; `[` / `]` jump between files; in the body,
  `↑↓`/`jk` move the cursor and the mouse wheel scrolls. `t` toggles between
  inline and side-by-side layouts.
- **Comment**: in the body, press `v` to start a line selection (arrows grow
  or shrink it; mouse drag also selects), then `Enter` to attach a comment.
  Comments are *staged* — they persist across restarts until applied, and
  show as `*` in the gutter (and a per-file badge). Each comment also
  renders as an inline box beneath its line; press `z` to fold it down to a
  single-line header or expand it again.
- **Apply**: press `a` to hand all staged comments to the session's agent.
  They're written to a markdown brief and the agent is prompted to address
  them — sent immediately when idle/working (it queues natively), held until a
  permission prompt clears, or deferred if the agent is stopped.
- **Drift**: if the code under a comment changes before you apply, the view
  re-anchors it by its captured snippet. If it can't be located unambiguously
  the comment is marked `⚠` (drifted) and blocks apply until you review or
  delete it (`d` removes the comment under the cursor).

Comments are stored per session under the data directory (alongside
`state.json`); the brief handed to the agent is written to a temp file outside
the worktree, so it's never committed.
