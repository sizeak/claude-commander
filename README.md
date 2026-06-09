# Claude Commander

A high-performance terminal UI for managing Claude coding sessions, written in Rust.

## Features

- **Async-first architecture** - Non-blocking tmux and git operations
- **Hierarchical session model** - Projects contain worktree sessions
- **Git worktree isolation** - Each session has its own worktree and branch
- **Live preview** - Real-time pane content capture with caching
- **Info pane** - Session metadata, PR details, CI status, and AI-generated change summaries
- **Review & comment** - Full-screen diff of a session's changes (vs its PR base) where you select lines, attach comments, and apply them straight to the running agent
- **Agent state detection** - Detect if agent is waiting for input, processing, or errored
- **Persistent state** - Sessions survive restarts
- **Auto-pull project main** - Periodically fast-forwards each project's main branch from `origin` so it doesn't drift stale

## Requirements

- **Rust/Cargo** - Required to build from source ([install via rustup](https://rustup.rs/))
- **tmux** - Required for session management
- **git** - For worktree operations

## Installation

### Homebrew (macOS and Linux)

```bash
brew tap sizeak/tap
brew install claude-commander
```

### Arch Linux (AUR)

```bash
yay -S claude-commander
```

### Cargo

Install directly from GitHub:

```bash
cargo install --git https://github.com/sizeak/claude-commander.git
```

Or clone and install locally:

```bash
cargo install --path .
```

Or build from source without installing:

```bash
cargo build --release
./target/release/claude-commander
```

## Quick Start

```bash
claude-commander
```

In the TUI:

- `N` — add a project (a git repository to manage sessions for)
- `n` — create a new worktree session in the selected project
- `Enter` — attach to the selected session
- `Ctrl-q` — detach back to the session list
- `?` — show help, `,` — open settings, `q` — quit

See the full [keyboard shortcuts](#keyboard-shortcuts) below, and the
[Usage guide](docs/usage.md) for CLI commands, PR stacks, and AI summaries.

## Reference

### Status Symbols

Each session displays a status indicator to the left of its name:

| Symbol | Meaning |
|--------|---------|
| `⠋` (animated spinner) | Session is being created or mid-cascade-merge |
| `●` (rainbow cycling) | Agent is actively working |
| `?` | Agent is waiting for user input |
| `⏸` | Cascade merge paused here — resolve conflicts and resume from the palette |
| `◆` | Session has unread output |
| `●` | Running (agent idle) |
| `○` | Stopped |

Indicators are shown in priority order — for example, a running session with unread output shows `◆` rather than `●`.

### PR Badges

When a session has a GitHub PR, a badge appears next to the session name. The badge color indicates the PR state:

| Color | Meaning |
|-------|---------|
| Blue | Open |
| Green | Open and awaiting review |
| Grey | Draft |
| Red | Closed |
| Dark purple | Merged |

The info pane shows additional detail when a PR is present, including a CI checks indicator:

| Symbol | Meaning |
|--------|---------|
| `✓` (green) | All checks passing |
| `✗` (red) | Checks failing |
| `◌` (orange) | Checks pending |
| `—` (grey) | No checks configured |

### Project Badges

When automatic project-branch pulling is enabled (see `project_pull_enabled` in [Configuration](docs/configuration.md)), a `⚠` badge appears next to a project name if its main branch could not be fast-forwarded. The badge is derived state — it clears automatically on the next successful or no-op pull. The info pane shows the reason as `⚠ blocked — <reason>`:

| Reason | Meaning |
|--------|---------|
| `Working tree dirty` | Main is the active checkout but has uncommitted changes |
| `Branch diverged from origin` | Local main has commits not on `origin` |
| `Checked out in another worktree` | Main is checked out in a separate worktree |

### Keyboard Shortcuts

All keybindings below are defaults and can be customised via the `[keybindings]` config table (see [Configuration](docs/configuration.md)).

| Key | Action |
|-----|--------|
| `j/k` or `↑/↓` or `Ctrl-n/p` | Navigate session list |
| `Space` | Quick-switch palette (sessions and commands) |
| `Ctrl-Space` | Quick-switch palette (same shortcut as the in-session switcher) |
| `Shift+Space` | Command palette (commands only) |
| `>` (as first char in palette) | Filter palette to commands only |
| `Enter` | Attach to selected session |
| `n` | New worktree session |
| `t` | New session stacked on top of the selected session's stack |
| `N` | Add new project |
| `c` | Checkout existing branch into a new worktree session (fetches `origin` in the background, filterable list) |
| `d` | Delete session |
| `r` | Rename session (UI title only; underlying worktree, branch, and tmux session are unchanged) |
| `R` | Restart session (kill tmux + recreate; adds `--resume` when `resume_session = true`) |
| `D` | Remove project |
| `.` or `Ctrl-.` | Open in editor/IDE |
| `o` | Open PR in browser (when the session has a PR) |
| `S` | Scan directory for git repos and add them as projects |
| `s` | Open shell in worktree |
| `v` | Cycle session list view: Project → Sections → Section Stacks (requires `[[sections]]` config) |
| palette only | Collapse/expand section (press on any item in the section, or `Enter` on the section header) |
| `m` | Move session to section (manual override; see [Session List Sections](docs/configuration.md#session-list-sections)) |
| `i` | Review & comment on a session's diff — see [Usage](docs/usage.md#reviewing--commenting-on-changes) |
| `Tab` / `Shift-Tab` | Switch between panes (forward / reverse) |
| `<` / `>` | Shrink / grow left pane |
| `Ctrl-u/d` or `PageUp/Down` | Page up/down in preview |
| `1`–`99` | Jump to session by number |
| `g` | Generate AI summary (Info pane only) |
| `,` | Open settings |
| `?` | Show help |
| `q` or `Ctrl-c` | Quit |

### Attached Session Shortcuts

When attached to a session (via `Enter` or `claude-commander attach`):

| Key | Action |
|-----|--------|
| `Ctrl-q` | Detach and return to session list |
| `Ctrl-\` | Switch between Claude and shell pane |
| `Ctrl-Space` | Open the in-session switcher popup to jump to another claude-commander session without detaching |
| `Ctrl-.` | Open the session worktree in your editor (requires a terminal that emits CSI-u or xterm modifyOtherKeys sequences for Ctrl-.) |

## Documentation

- **[Usage guide](docs/usage.md)** — CLI commands, the session list, PR stacks (cascade merge / push stack), and AI summaries
- **[Configuration](docs/configuration.md)** — all config options, theme presets, session-list sections, and data-storage paths
- **[Contributing](CONTRIBUTING.md)** — releasing, the local dev loop, and architecture overview

## License

MIT
