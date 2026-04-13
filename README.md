# Claude Commander

A high-performance terminal UI for managing Claude coding sessions, written in Rust.

## Features

- **Async-first architecture** - Non-blocking tmux and git operations
- **Hierarchical session model** - Projects contain worktree sessions
- **Git worktree isolation** - Each session has its own worktree and branch
- **Live preview** - Real-time pane content capture with caching
- **Info pane** - Session metadata, PR details, CI status, and AI-generated change summaries
- **Agent state detection** - Detect if agent is waiting for input, processing, or errored
- **Persistent state** - Sessions survive restarts

## Requirements

- **Rust/Cargo** - Required to build from source ([install via rustup](https://rustup.rs/))
- **tmux** - Required for session management
- **git** - For worktree operations

## Installation

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

## Usage

### Interactive TUI (default)

```bash
claude-commander
```

### Commands

```bash
# List active sessions (add --all to include stopped)
claude-commander list

# Create a new session
claude-commander new "feature-auth" --path /path/to/repo

# Attach to a session
claude-commander attach feature-auth

# Show configuration
claude-commander config

# Initialize config file
claude-commander config --init

# Use a custom config file
claude-commander --config /path/to/config.toml
```

### Session List

The left pane shows projects and their worktree sessions in a tree view. Projects are sorted alphabetically. Sessions within a project are sorted newest first (by creation time).

### Keyboard Shortcuts

All keybindings below are defaults and can be customised via the `[keybindings]` config table (see [Configuration](#configuration)).

| Key | Action |
|-----|--------|
| `j/k` or `↑/↓` or `Ctrl-n/p` | Navigate session list |
| `Space` | Quick-switch (fuzzy session search) |
| `Enter` | Attach to selected session |
| `n` | New worktree session |
| `N` | Add new project |
| `p` | Pause session |
| `r` | Resume session |
| `d` | Delete session |
| `R` | Restart session (kill tmux + recreate with /resume) |
| `D` | Remove project |
| `e` | Open in editor/IDE |
| `s` | Open shell in worktree |
| `Tab` / `Shift-Tab` | Switch between panes (forward / reverse) |
| `<` / `>` | Shrink / grow left pane |
| `Ctrl-u/d` or `PageUp/Down` | Page up/down in preview |
| `1`–`99` | Jump to session by number (requires `show_session_numbers`) |
| `g` | Generate AI summary (Info pane only) |
| `?` | Show help |
| `q` or `Ctrl-c` | Quit |

### Attached Session Shortcuts

When attached to a session (via `Enter` or `claude-commander attach`):

| Key | Action |
|-----|--------|
| `Ctrl-q` | Detach and return to session list |
| `Ctrl-\` | Switch between Claude and shell pane |
| `Ctrl-<letter>` | Open worktree in editor/IDE — opt-in via `editor_ctrl_hotkey_in_tmux_session`; uses the letter from your `open_in_editor` binding |

## Configuration

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

# Shell program for shell sessions (default: $SHELL or "bash")
# shell_program = "zsh"

# Editor/IDE command for opening sessions (e.g. "code", "zed", "nvim")
# Falls back to $VISUAL, then $EDITOR if not set
# editor = "code"

# Whether the editor is a GUI application (true) or terminal-based (false)
# GUI editors are spawned in the background; terminal editors suspend the TUI
# Auto-detected from a known list if not set (code, zed, subl, JetBrains IDEs, etc.)
# editor_gui = true

# When true, pressing Ctrl+<your `open_in_editor` letter> inside an attached
# tmux session opens the worktree in the configured editor.
# editor_ctrl_hotkey_in_tmux_session = false

# Interval in seconds between GitHub PR checks (0 = disabled)
pr_check_interval_secs = 600

# Dim the right pane (preview/diff/shell) when the session list is focused
dim_unfocused_preview = true

# How much to dim unfocused pane colors (0.0 = fully dimmed/black, 1.0 = no dimming)
# Uses a foreground color override for cross-terminal compatibility (no Modifier::DIM)
dim_unfocused_opacity = 0.4

# Leader key for quick-switch session search
# Supports: " ", "space", "ctrl+k", "f1", etc.
# leader_key = " "

# Show sequential numbers next to sessions and enable digit-key jumping (1–99)
# show_session_numbers = true

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

# Custom key bindings — override any default key with one or more alternatives
# [keybindings]
# navigate_up = ["k", "Up"]
# quit = ["q", "Ctrl-c"]
# toggle_pane = ["Tab"]
```

### AI Summary

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

## Architecture

The TUI event loop (`App`) owns the terminal and render state. It sends user commands to a `SessionManager` which coordinates tmux and git operations via async channels. Git read operations use gitoxide (pure Rust); worktree mutations and tmux use CLI subprocesses with semaphore-based throttling.

```
┌───────────────────────────────────────────┐
│              TUI (ratatui)                │
│  Renders widgets, handles input           │
└─────────────────┬─────────────────────────┘
                  │ mpsc channels
┌─────────────────▼─────────────────────────┐
│           SessionManager                  │
│  Session lifecycle, state persistence     │
└──────┬────────────────────┬───────────────┘
       │                    │
┌──────▼──────┐      ┌──────▼──────┐
│ TmuxExecutor│      │ GitBackend  │
│ (async CLI) │      │ (gitoxide)  │
└─────────────┘      └─────────────┘
```

## Data Storage

Paths are platform-specific, determined by the `directories` crate:

| File | macOS | Linux |
|------|-------|-------|
| Config | `~/Library/Application Support/com.claude-commander.claude-commander/config.toml` | `~/.config/claude-commander/config.toml` |
| State | `~/Library/Application Support/com.claude-commander.claude-commander/state.json` | `~/.local/share/claude-commander/state.json` |
| Worktrees | `~/Library/Application Support/com.claude-commander.claude-commander/worktrees/` | `~/.local/share/claude-commander/worktrees/` |

## Development

```bash
# Run tests
cargo test

# Run with debug logging
claude-commander --debug

# Check for issues
cargo clippy
```

## License

MIT
