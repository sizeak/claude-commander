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

## Installation

```bash
cargo install --path .
```

Or build from source:

```bash
cargo build --release
./target/release/claude-commander
```

## Requirements

- **tmux** - Required for session management
- **git** - For worktree operations

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
| `Tab` | Switch between panes |
| `Ctrl+u/d` | Page up/down in preview |
| `?` | Show help |
| `q` | Quit |

## Configuration

Configuration file: `~/.config/claude-commander/config.toml`

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
```

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    TUI Actor (ratatui)                          │
│  Owns: terminal, widgets, render state                          │
│  Receives: StateSnapshot, ContentUpdate, DiffUpdate             │
│  Sends: UserCommand (create, pause, attach, kill)               │
└────────────────────────────┬────────────────────────────────────┘
                             │ mpsc channels
┌────────────────────────────▼────────────────────────────────────┐
│                    SessionManager Actor                         │
│  Owns: Vec<Session>, coordinates lifecycle                      │
│  Receives: UserCommand, TmuxEvent, GitEvent                     │
│  Sends: StateSnapshot to TUI                                    │
└──────┬─────────────────────┬────────────────────────────────────┘
       │                     │
┌──────▼──────┐       ┌──────▼──────┐
│ TmuxActor   │       │ GitActor    │
│ per session │       │ per session │
│ Owns: state │       │ Owns: repo  │
└─────────────┘       └─────────────┘
```

### Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| Actor pattern | Each subsystem owns its state, communicates via typed channels |
| gitoxide | Pure Rust git implementation - no CLI dependency for git ops |
| tokio async | Non-blocking IO prevents UI freezes |
| Semaphore throttling | 16 concurrent tmux commands for scalability |
| Cache TTLs | 50ms for content, 500ms for diffs - balances freshness vs load |

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
