# Configuration

Configuration, theming, section grouping, and data-storage paths for Claude
Commander. For installation and the keyboard / status-symbol reference, see the
[README](../README.md). For the usage guide, see [Usage](usage.md).

Configuration file location depends on your platform:

- **macOS**: `~/Library/Application Support/com.claude-commander.claude-commander/config.toml`
- **Linux**: `~/.config/claude-commander/config.toml`

All settings can also be set via environment variables with the `CC_` prefix (e.g. `CC_EDITOR=code`).

```toml
# Default program to run in new sessions
default_program = "claude"

# Branch name prefix for new sessions (empty = no prefix)
branch_prefix = ""

# Fetch latest changes from origin before creating a new session
fetch_before_create = true

# Pass `--resume` when restarting/recreating a session so the agent picks up
# where it left off. Set to false to start the program fresh each time.
resume_session = true

# Launch sessions inside `nix develop` when the project has a `flake.nix` at
# its root and `nix` is on PATH, so the agent and shell get the project's dev
# environment. Applies to Claude sessions and shell sessions. Projects without
# a flake are unaffected.
nix_develop = true

# Maximum concurrent tmux commands
max_concurrent_tmux = 16

# Content capture cache TTL in milliseconds
capture_cache_ttl_ms = 50

# Diff cache TTL in milliseconds
diff_cache_ttl_ms = 500

# UI refresh rate in FPS
ui_refresh_fps = 30

# Custom worktrees directory (default: platform-specific, see Data Storage below)
# worktrees_dir = "/path/to/worktrees"

# Organize worktrees into per-repository subdirectories (default: false)
# per_repo_worktree_dirs = true

# Shell program for shell sessions (default: $SHELL or "bash")
# shell_program = "zsh"

# Editor/IDE command for opening sessions (e.g. "code", "zed", "nvim")
# Falls back to $VISUAL, then $EDITOR if not set
# editor = "code"

# Whether the editor is a GUI application (true) or terminal-based (false)
# GUI editors are spawned in the background; terminal editors suspend the TUI
# Auto-detected from a known list if not set (code, zed, subl, JetBrains IDEs, etc.)
# editor_gui = true

# Interval in seconds between GitHub PR checks (0 = disabled)
pr_check_interval_secs = 120

# Periodically fast-forward each project's main branch from origin.
# When enabled, runs `git fetch origin <main>` and advances the local
# `<main>` ref whenever a fast-forward is possible. If `<main>` is the
# currently checked-out branch in the project repo, `git merge --ff-only`
# is used when the working tree is clean. The pull is held back (and the
# project row shows a ⚠ badge) when the working tree is dirty, the branch
# has diverged from origin, or `<main>` is checked out in another worktree.
# Default: enabled.
project_pull_enabled = true

# Interval in seconds between project-branch pulls. Minimum 60.
# Default: 3600 (one hour).
project_pull_interval_secs = 3600

# Use rounded border corners (╭╮╰╯) instead of square (┌┐└┘)
rounded_borders = false

# When opening the review view, precompute every file's render caches
# (word-diff segments + syntax highlighting) up front behind a loading spinner,
# instead of building each file's cache lazily on first navigation. Trades a
# one-off wait when opening for instant file switching afterwards. Default true;
# set to false for lazy, instant-open behaviour.
# precompute_review_caches = true

# Dim the right pane (preview/diff/shell) when the session list is focused
dim_unfocused_preview = true

# How much to dim unfocused pane colors (0.0 = fully dimmed/black, 1.0 = no dimming)
# Uses a foreground color override for cross-terminal compatibility (no Modifier::DIM)
dim_unfocused_opacity = 0.4

# Leader key for quick-switch session search
# Supports: " ", "space", "ctrl+k", "f1", etc.
# leader_key = " "

# Render PR labels as colored text on the default background (the pre-pill
# style). Default false renders them as colored "pill" blocks that stand out
# more in the session list.
# invert_pr_label_color = false

# Show the running program as a "(program)" suffix on session rows. Only
# renders when sessions use more than one distinct program, so it's a no-op
# for a single-program setup. Set to false to always hide it.
# show_session_program = true

# Debounce delay in ms when typing multi-digit session numbers
# session_number_debounce_ms = 250

# Interval in milliseconds for syncing state file changes from other instances (0 = disabled)
state_sync_interval_ms = 2000

# Log file path (if set, logs to file; use with --debug)
# log_file = "/tmp/claude-commander.log"

# Enable AI-generated branch summaries in the Info pane (default: true)
# ai_summary_enabled = true

# Claude model used for AI summaries (default: Haiku for cost efficiency)
# ai_summary_model = "claude-haiku-4-5-20251001"

# Persistent "commander" session — a project-less Claude session (opened with
# `C` or `claude-commander commander`) that coordinates other sessions via the
# CLI. Disabled by default; enabling it is restart-required.
# commander_enabled = false
# Program (with flags) for the commander; defaults to `default_program`.
# commander_program = "claude --model opus-4-7"
# Working directory for the commander; defaults to <data dir>/commander.
# commander_dir = "/path/to/commander"

# Custom key bindings — override any default key with one or more alternatives
# [keybindings]
# navigate_up = ["k", "Up"]
# quit = ["q", "Ctrl-c"]
# toggle_pane = ["Tab"]
```

## Theme Presets

Set `preset` under `[theme]` in your config to switch the entire color palette:

```toml
[theme]
preset = "rose-pine"
```

Available presets:

| Preset | Description |
|--------|-------------|
| `basic` | 16-color ANSI (maximum compatibility) |
| `indexed` | 256-color palette |
| `truecolor` | 24-bit Catppuccin-inspired pastels (default on capable terminals) |
| `monokai-dimmed` | Muted/desaturated Monokai — dark grays with soft gold, green, and blue accents |
| `zedokai` | Vibrant Monokai variant inspired by the Zed editor — vivid pink, green, and orange |
| `rose-pine` | Soft pink/rose aesthetic — deep navy-rose backgrounds with warm rose, iris, and foam accents |

When `preset` is unset (or `"(auto)"`), the theme auto-detects your terminal's color capability.

Individual color overrides (e.g. `border_focused = "#ff6600"`) still apply on top of the chosen preset.

## Session List Sections

Group the session list under configurable headers based on GitHub PR state.

By default `[[sections]]` is empty and the list keeps its project-grouped view. Once you declare one or more sections, the list switches to a section-grouped layout: section headers at the top level, each repo nested beneath as a sub-header, and sessions indented below their repo.

Sections replace [PR-stack grouping](usage.md#pr-stacks) — when sections are configured, stacked children are no longer visually nested under their stack base. The underlying stack links are still tracked and the `t` hotkey still stacks new sessions onto the top of a stack, but ordering within the list follows the section rules.

An implicit **"In Progress"** section is always the first row and acts as the catch-all — any session whose PR state doesn't match a later section's predicate lands here. It also lists every repo that hasn't placed a session into a later section, so newly added projects remain visible.

### Example

This mirrors a label-driven code review workflow: author adds `dev-review-required` when the PR is ready, a reviewer removes the label and picks it up, the PR gets approved and merged.

```toml
[[sections]]
name = "Needs Review"
has_label = "dev-review-required"

[[sections]]
name = "In Review"
pr_state = "open"          # scope to open PRs — otherwise merged PRs
has_reviewer = true        # with reviewers would also match here

[[sections]]
name = "Merged"
pr_state = ["merged", "closed"]

[[sections]]
name = "Stale"             # no predicates → manual-only waypoint
```

Visually:

```
In Progress (12)
   terraform [main] (3)
      session-a
      session-b
      session-c
   genio     [main] (0)

Needs Review (1)
   genio     [main] (1)
      fix-dns-spam

In Review (2)
   terraform [main] (1)
      new-metrics-port
   genio     [main] (1)
      claude/add-elasticsearch-readonly-creds

Merged (3)
   …

Stale (0)
```

### Predicate fields

All fields are optional; a section matches when **every declared field** matches (AND). A section declared with no predicates is a **manual-only waypoint** — auto-matching never puts sessions there; you only move in via the palette.

| Field | Type | Notes |
|---|---|---|
| `pr_state` | `"open"` \| `"closed"` \| `"merged"` — scalar or array (any-of) | |
| `is_draft` | `bool` | |
| `has_pr` | `bool` | |
| `has_label` | string (literal) or array (any-of) | |
| `review_decision` | `"approved"` \| `"changes_requested"` \| `"review_required"` — scalar or array (any-of) | Mirrors GitHub's `reviewDecision` field |
| `has_reviewer` | `true` / `false`, a specific login, or an array of logins (any-of) | `true` excludes Copilot via case-insensitive `"copilot"` substring match; specific/array forms match literally |

### Process order and forward-only

Config order is the pipeline. A session's section is re-evaluated on every PR refresh, but the scan **only considers sections at or after the session's current position** — auto never moves a session backwards. This keeps `"Needs Review"` sticky when a reviewer removes the label without leaving other signals; the session doesn't slide back to `"In Progress"`.

### Moving sessions manually

Select a session and press `m` (or open the palette with `Space`, or `Shift+Space` for commands-only, and run **Move session to section…**), then pick a target. An **Auto** entry clears an existing pin. The override is persisted to `state.json` and survives restarts; auto-moves are suppressed until the pin is released.

### Creating sessions inside a section

In the section-grouped views, a session created with `n` lands in the section the cursor was in, not the "In Progress" catch-all. For a manual-only waypoint (no predicates) this sets the same pin as a manual move; for a predicate-bearing section it's a soft placement — the session starts there but still auto-advances through the pipeline as its PR progresses. Creating from "In Progress" keeps the default behaviour. The CLI's `claude-commander new --section` flag follows the same rules.

### Reordering, adding, or removing sections

These are edit-`config.toml`-and-restart actions — there's no hot reload. The cached `current_section` on each session is reconciled against the new config on next startup; if the referenced section no longer exists, the session falls back to `"In Progress"` and continues forward-only from there.

## Data Storage

Paths are platform-specific, determined by the `directories` crate:

| File | macOS | Linux |
|------|-------|-------|
| Config | `~/Library/Application Support/com.claude-commander.claude-commander/config.toml` | `~/.config/claude-commander/config.toml` |
| State | `~/Library/Application Support/com.claude-commander.claude-commander/state.json` | `~/.local/share/claude-commander/state.json` |
| Worktrees | `~/Library/Application Support/com.claude-commander.claude-commander/worktrees/` | `~/.local/share/claude-commander/worktrees/` |
