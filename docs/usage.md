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

# Create a session placed directly in a specific section (pinned for
# manual-only sections, soft placement for predicate-bearing ones — see
# "Creating sessions inside a section" in configuration.md)
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

## Reviewing & commenting on changes

Press **`r`** on a selected session (or open the **command palette** with
`Space` and run **"Review diff & comment"**) to open a full-screen review view. It shows everything a PR
against the session's base branch would contain — committed, staged, unstaged,
and untracked changes — composed from `merge-base(base, HEAD)` through the
working tree.

While attached to a Claude session you can jump straight to its review diff
with **`Alt-r`**, and **`Alt-r`** again from inside the diff switches back to
the session — the same direct toggle the shell pane has with `Ctrl-\`, without
detaching to the session list. `Alt-r` is used rather than `Ctrl-r` so a
shell's `Ctrl-r` reverse-history-search is never shadowed; the toggle is wired
up for Claude sessions only.

The diff is rendered in a `lumen`/`hunk`-style colour scheme: dark green/red
line fills, a brighter highlight on the changed span within a line, and (on
true-color terminals) syntax highlighting of the code. It degrades to
foreground-only colouring on 256- and 16-colour terminals.

- **Navigate**: the changed files are shown as a collapsible **tree** (lazygit
  style — single-child directory chains are compressed). `Tab` switches focus
  between the tree and the diff body; in the tree, `↑↓`/`jk` move and `Enter`
  expands/collapses a directory; `[` / `]` jump between files; in the body,
  `↑↓`/`jk` move the cursor and the mouse wheel scrolls. Clicking a file in the
  tree selects it (clicking a directory expands/collapses it). `t` toggles
  between inline and side-by-side layouts. In the inline layout, lines longer
  than the pane soft-wrap onto continuation rows (GitHub-style) so nothing runs
  off-screen; side-by-side truncates to keep its columns aligned.
- **Mark reviewed**: press `m` to toggle a ✓ reviewed mark on the current
  file. Marking dims the file in the tree and auto-advances to the next
  unreviewed file (wrapping); unmarking stays put. Marks persist per session
  across restarts and clear automatically when a file's diff changes or it
  leaves the diff (GitHub "Viewed" semantics), so a re-edited file demands a
  fresh look. The tree title shows progress (`Files (3/7 reviewed)`).
- **Binary & image files**: image files (PNG, JPEG, GIF, WebP, BMP, …) render
  inline in the diff body using the terminal's graphics protocol (Kitty,
  iTerm2, or Sixel), falling back to Unicode half-blocks on terminals without
  one. For a modified image, `o` toggles between the **before** and **after**
  versions; added and deleted images show their only side. Image bytes are
  loaded lazily when the file scrolls into view. Other (non-image) binaries
  show a size/placeholder line instead of a textual diff.
- **Comment**: in the body, press `v` to start a line selection (arrows grow
  or shrink it; mouse drag also selects), then `Enter` to attach a comment.
  Right-clicking or double-clicking a line is a mouse shortcut for the same
  thing — it selects that line and opens the comment box directly.
  Comments are *staged* — they persist across restarts until applied, and
  show as `*` in the gutter and a coloured `*N` count on the file (and its
  parent directories) in the tree. A session with pending comments is also
  flagged with a `*` in the main session list. Each comment also renders as
  an inline box beneath its line; press `z` (or click the box) to fold it
  down to a single-line header or expand it again.
- **Apply**: press `a` to hand all staged comments to the session's agent.
  They're written to a markdown brief and the agent is prompted to address
  them — sent immediately when idle/working (it queues natively), held until a
  permission prompt clears, or deferred if the agent is stopped.
- **Refresh**: the diff is a snapshot taken when you opened the view, so the
  agent's edits (e.g. after applying comments) don't appear until it's
  re-composed. This happens automatically when the session's agent goes idle —
  i.e. finishes a turn — folding its changes in while keeping your place (same
  file, clamped cursor/scroll). Press `r` to re-compose on demand at any time;
  it reports "Review refreshed" or "Review already up to date".
- **Drift**: if the code under a comment changes before you apply, the view
  re-anchors it by its captured snippet. If it can't be located unambiguously
  the comment is marked `⚠` (drifted) and blocks apply until you review or
  delete it (`d` removes the comment under the cursor).

Comments are stored per session under the data directory (alongside
`state.json`); the brief handed to the agent is written to a temp file outside
the worktree, so it's never committed.

By default each file's render caches (word-diff segments + syntax highlighting)
are built up front behind a brief loading spinner when you open the review, so
file switching is instant afterwards. Set `precompute_review_caches = false`
(see [Configuration](configuration.md)) for lazy behaviour instead — the view
opens instantly and each file's cache is built the first time you navigate to
it, at the cost of a brief jank on the first scroll through a large file.
