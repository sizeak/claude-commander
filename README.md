# Claude Commander

A high-performance terminal UI for managing Claude coding sessions, written in Rust.

## Features

- **Async-first architecture** - Non-blocking tmux and git operations
- **Hierarchical session model** - Projects contain worktree sessions
- **Git worktree isolation** - Each session has its own worktree and branch
- **Live preview** - Real-time pane content capture with caching
- **Diff view** - See changes made by the AI agent
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
# List all sessions
claude-commander list

# Create a new session
claude-commander new "feature-auth" --path /path/to/repo

# Attach to a session
claude-commander attach feature-auth

# Show configuration
claude-commander config

# Initialize config file
claude-commander config --init
```

### Session List

The left pane shows projects and their worktree sessions in a tree view. Projects are sorted alphabetically. Sessions within a project are sorted newest first (by creation time).

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `j/k` or `↑/↓` | Navigate session list |
| `Enter` | Attach to selected session |
| `n` | New worktree session |
| `N` | Add new project |
| `p` | Pause session |
| `r` | Resume session |
| `d` | Delete session |
| `D` | Remove project |
| `e` | Open in editor/IDE |
| `s` | Open shell in worktree |
| `Tab` | Switch between panes |
| `Ctrl+u/d` | Page up/down in preview |
| `?` | Show help |
| `q` | Quit |

## Configuration

Configuration file: `~/.config/claude-commander/config.toml`

All settings can also be set via environment variables with the `CC_` prefix (e.g. `CC_EDITOR=code`).

```toml
# Default program to run in new sessions
default_program = "claude"

# Branch name prefix for new sessions (empty = no prefix)
branch_prefix = ""

# Maximum concurrent tmux commands
max_concurrent_tmux = 16

# Content capture cache TTL in milliseconds
capture_cache_ttl_ms = 50

# Diff cache TTL in milliseconds
diff_cache_ttl_ms = 500

# UI refresh rate in FPS
ui_refresh_fps = 30

# Editor/IDE command for opening sessions (e.g. "code", "zed", "nvim")
# Falls back to $VISUAL, then $EDITOR if not set
# editor = "code"

# Whether the editor is a GUI application (true) or terminal-based (false)
# GUI editors are spawned in the background; terminal editors suspend the TUI
# Auto-detected from a known list if not set (code, zed, subl, JetBrains IDEs, etc.)
# editor_gui = true
```

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

- **Config**: `~/.config/claude-commander/config.toml`
- **State**: `~/.local/share/claude-commander/state.json`
- **Worktrees**: `~/.local/share/claude-commander/worktrees/`

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
