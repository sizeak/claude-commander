# Claude Commander

A high-performance terminal UI for managing Claude coding sessions, written in Rust.

![Claude Commander kanban board](docs/images/board.svg)

## Features

- **Async-first architecture** - Non-blocking tmux and git operations
- **Hierarchical session model** - Projects contain worktree sessions
- **Git worktree isolation** - Each session has its own worktree and branch
- **Kanban board UI** - Full-screen board with sections as columns and sessions as project-coloured cards
- **Live preview pane** - In the list views, a right-hand pane tails the selected session's agent or shell output as it happens (`Tab` switches, `<`/`>` resizes)
- **Info modal** - On-demand session metadata, PR details, CI status, and AI-generated change summaries (`i`)
- **Review & comment** - Full-screen diff of a session's changes (vs its PR base) where you select lines, attach comments, mark files as reviewed, and apply comments straight to the running agent
- **Agent state detection** - Detect if agent is waiting for input, processing, or errored
- **Persistent state** - Sessions survive restarts
- **Auto-pull project main** - Periodically fast-forwards each project's main branch from `origin` so it doesn't drift stale

![Session info modal](docs/images/info-modal.svg)

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
- `Ctrl-q` — detach back to the board
- `?` — show help, `,` — open settings, `q` — quit

See the full [keyboard shortcuts](#keyboard-shortcuts) below, and the
[Usage guide](docs/usage.md) for CLI commands, PR stacks, and AI summaries.

## Reference

### Views

The session list can be shown four ways, cycled with `v`: three full-width
**list** views — grouped by **project**, by **section**, or by **section with
PR stacks** — and the full-screen kanban **board**. The list starts in the
project-grouped view; press `v` to rotate project → sections → stacks → board →
project. When no `[[sections]]` are configured the two section views render
identically to the project view, so `v` skips them and simply toggles between
the project list and the board. The chosen view is remembered across restarts.

There is no side panel in any view — open a session's details with `i` (Info),
its shell with `Enter`/`s`, and its review diff with `r`. In the section-grouped
list views a section under the cursor can be collapsed or expanded with the
**Toggle section** command (unbound by default; bind a key in Settings or run it
from the command palette).

#### Board

Each **column** is a section, and
every session is its own **card**: the border is coloured by project and its
title is the session's number and name (project identity lives in the colour and
the sidebar, not in card text). The card's single interior line shows the status
glyph, row markers, PR pill / `[branch]`, and — right aligned — three clickable
action buttons: `[>_]` open shell, `[±]` open review diff, and `[i]` open info.
Clicking a button selects that card and fires the action; clicking elsewhere on a
card selects it and double-clicking attaches. A stacked session renders as its
own card nested (indented) beneath its parent, and the whole stack moves between
columns as a unit.

Within a column, cards sort by how likely a session is to need you: a
**needs-you** band on top (waiting for input, paused cascade, or unread output),
an **active** band (working / idle), and **stopped** sessions at the bottom;
newer sessions float to the top of their band. The leftmost column is a narrow
**project sidebar** listing every project with its session count; selecting a
project there enables the project-scoped actions — new session, project shell
(`s`), and remove project.

**Selecting** a project in the sidebar (`Enter`, or double-click) **filters**
the board to just that project's cards (the sidebar still lists every project);
the top bar shows the active filter. Selecting the same project again — or
`Esc` — clears it; selecting a different project refilters. Just moving the
cursor over the sidebar does not filter. Jumping to a session via the
quick-switch palette clears the filter if needed so the jump always lands.

With no `[[sections]]` configured, three default columns are shown and sessions
are assigned to them automatically from PR state: **In Progress** (the catch-all
for anything without a matching PR), **In Review** (open PR), and **Merged**
(merged PR). Defining any `[[sections]]` in config replaces these defaults — see
[Session List Sections](docs/configuration.md#session-list-sections). Press `m`
to move a card to another column. Empty columns are hidden by default
(`hide_empty_sections`), so a board shows only sections that have work.

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

A `*` after a session's title means it has pending [review comments](docs/usage.md#reviewing--commenting-on-changes) that haven't been applied to the agent yet.

### PR Badges

When a session has a GitHub PR, a badge appears next to the session name. The badge color indicates the PR state:

| Color | Meaning |
|-------|---------|
| Blue | Open |
| Green | Open and awaiting review |
| Grey | Draft |
| Red | Closed |
| Dark purple | Merged |

The Info modal (`i`) shows additional detail when a PR is present, including a CI checks indicator:

| Symbol | Meaning |
|--------|---------|
| `✓` (green) | All checks passing |
| `✗` (red) | Checks failing |
| `◌` (orange) | Checks pending |
| `—` (grey) | No checks configured |

### Project Badges

When automatic project-branch pulling is enabled (see `project_pull_enabled` in [Configuration](docs/configuration.md)), a `⚠` badge appears next to a project name (in the sidebar and on that project's cards) if its main branch could not be fast-forwarded. The badge is derived state — it clears automatically on the next successful or no-op pull. The pull is held back for one of these reasons:

| Reason | Meaning |
|--------|---------|
| `Working tree dirty` | Main is the active checkout but has uncommitted changes |
| `Branch diverged from origin` | Local main has commits not on `origin` |
| `Checked out in another worktree` | Main is checked out in a separate worktree |

### Keyboard Shortcuts

All keybindings below are defaults and can be customised via the `[keybindings]` config table (see [Configuration](docs/configuration.md)).

The status bar surfaces the most useful actions as clickable buttons, with the hotkey letter bracketed (`[n]ew session`, `[d]elete`); the review view's footer works the same way. Clicking a button fires the same action as its key, so the hotkeys below can also be discovered and triggered with the mouse.

| Key | Action |
|-----|--------|
| `v` | Cycle the view: project list → section list → section-stack list → board → (repeat); section views are skipped when no `[[sections]]` are configured |
| `j/k` or `↑/↓` or `Ctrl-n/p` | Move up / down (within a board column, or through the list) |
| `h/l` or `←/→` | Move between columns / groups (the project sidebar is the board's leftmost column) |
| `]` / `[` | Next / previous column or group |
| `PageUp` / `PageDown` | Move up / down a screenful (within the board column, or through the list; stops at the ends rather than wrapping) |
| `Home` / `End` or `Ctrl-u/d` | Jump to first / last item |
| `1`–`99` | Jump to session by number |
| palette only | Toggle section — collapse/expand the section under the cursor in the section list views (unbound by default) |
| `Space` | Quick-switch palette (sessions and commands) |
| `Ctrl-Space` | Quick-switch palette (same shortcut as the in-session switcher) |
| `Shift+Space` | Command palette (commands only) |
| `>` (as first char in palette) | Filter palette to commands only |
| `Enter` | Attach to selected session |
| `Esc` | Clear the active project filter (set by selecting a project in the sidebar) |
| `i` | Show session info (Info modal — metadata, diffstat, PR details, stack chain, `g` for AI summary) |
| `n` | New worktree session |
| `t` | New session stacked on top of the selected session's stack |
| `N` | Add new project |
| `c` | Checkout existing branch into a new worktree session (fetches `origin` in the background, filterable list) |
| `d` | Delete session |
| `R` | Restart session (kill tmux + recreate; adds `--resume` when `resume_session = true`) |
| `D` | Remove project |
| `.` or `Ctrl-.` | Open in editor/IDE (also works inside the review diff, opening that session's worktree) |
| `o` | Open PR in browser (when the session has a PR) |
| palette only | Refresh PR status (force an immediate re-check for all sessions instead of waiting for the `pr_check_interval_secs` cadence) |
| `C` | Open the commander session (a persistent, project-less Claude session that coordinates others; requires `commander_enabled = true`). While it is running, a `● Commander` chip in the footer status bar shows its live state (`· working` / `· waiting` / `· idle`) |
| `Alt-c` | Open/close the conversation overlay: a full-screen chat with a dedicated Claude session whose replies stream in and are spoken aloud via an OpenAI-compatible TTS engine. Enable it first in Settings ▸ Conversation (off by default); see [Conversation mode](docs/configuration.md#conversation-mode-tts). The session keeps running when the overlay is closed |
| `Alt-v` | Voice input (push-to-talk by toggle): press once to start recording the microphone, press again to stop, transcribe via an OpenAI-compatible speech-to-text engine, and send the text to the conversation agent. Works whether the overlay is open or not. Enable it in Settings ▸ Conversation (`stt_enabled`, off by default). Can also be triggered **system-wide** via a desktop global shortcut — see [Global voice hotkey](docs/configuration.md#global-voice-hotkey) |
| `S` | Scan directory for git repos and add them as projects |
| `s` | Open shell in worktree (or a project shell when a project is selected in the sidebar) |
| `m` | Move a card to another column (a stacked session moves with its whole stack; manual override — see [Session List Sections](docs/configuration.md#session-list-sections)) |
| `r` or `Alt-r` | Review & comment on a session's diff — see [Usage](docs/usage.md#reviewing--commenting-on-changes) |
| palette only | Rename session (UI title only; underlying worktree, branch, and tmux session are unchanged) |
| palette only | Change program (agent) — pick a different program (e.g. `claude`, `codex`, `opencode`) for the selected session and relaunch it with a fresh conversation |
| `g` | Generate AI summary (Info modal only) |
| `Tab` / `Shift-Tab` | Switch the right pane between Preview and Shell (list views only; the board is full-screen) |
| `<` / `>` | Narrow / widen the session list, moving the divider between it and the right pane (list views only) |
| `,` | Open settings |
| `?` | Show help |
| `q` or `Ctrl-c` | Quit |

### Attached Session Shortcuts

When attached to a session (via `Enter` or `claude-commander attach`):

| Key | Action |
|-----|--------|
| `Ctrl-q` | Detach and return to the board |
| `Ctrl-\` | Switch between the Claude and shell sessions |
| `Alt-r` | Switch to this session's review diff (and `Alt-r` in the diff switches back) — Claude sessions only. Uses `Alt-r` rather than `Ctrl-r` so the shell's `Ctrl-r` reverse-history-search is never shadowed |
| `Ctrl-Space` | Open the in-session switcher popup to jump to another claude-commander session without detaching |
| `Ctrl-.` | Open the session worktree in your editor (requires a terminal that emits CSI-u or xterm modifyOtherKeys sequences for Ctrl-.) |
| `Ctrl-v` | **Remote sessions only:** paste an image from your local clipboard into the Claude prompt. The image is uploaded to the server, saved to a temp file, and its path is typed into the prompt. On a local session `Ctrl-v` is forwarded to Claude, which reads your clipboard directly. If the clipboard holds no image, `Ctrl-v` is forwarded unchanged |

### Remote Servers

The TUI can manage sessions on other machines running `claude-commander-server`.
Each configured server appears as its own node in the session tree, with that
server's projects and sessions underneath — create, delete, restart, review
diffs, and attach to remote terminals exactly as you would locally (attach
streams over WebSocket). Live agent-state dots and PR chips come from the
server's own background refresh, and an unreachable server degrades to a
greyed node with the error shown, retrying in the background without ever
blocking local work.

Add or remove servers from the command palette (**"Add remote server"** walks
name → URL → token with a connection test), or edit `[[remote_servers]]` in
the config file directly — see [Configuration](docs/configuration.md). Changes
hot-reload; no restart needed.

Each server's **program list** (the new-session picker options) lives in that
server's own config, so a fresh server offers only the built-in `claude` until
you configure it. Edit it without leaving the TUI: click the ⚙ on a server's
tree header, or run **"Edit server's program list…"** from the palette, to open
Settings → Programs targeting that server. In the Programs tab, `t` cycles which
backend (local or a remote server) you're editing; edits are saved to the chosen
backend as you make them.

## Documentation

- **[Usage guide](docs/usage.md)** — CLI commands, the board, PR stacks (cascade merge / push stack), and AI summaries
- **[Configuration](docs/configuration.md)** — all config options, theme presets, session-list sections (with optional advisory WIP limits), and data-storage paths
- **[Contributing](CONTRIBUTING.md)** — releasing, the local dev loop, and architecture overview
- **[Flutter client](client/README.md)** — cross-platform GUI client (Linux desktop + Android) for `claude-commander-server`

## Telemetry & Privacy

Claude Commander reports anonymous **feature-usage** telemetry (on by default) so
we can learn which features are used and retire the ones that aren't. It sends
feature names, a coarse environment fingerprint (OS, terminal, shell), a
non-sensitive config snapshot (e.g. theme), and a random install id — **never**
typed text, prompts, session content, branch names, or paths. Opt out with
`telemetry.enabled = false` in your config or by exporting `DO_NOT_TRACK=1`. See
[Configuration → Usage Telemetry](docs/configuration.md#usage-telemetry) for the
full list and self-hosting options.

## License

MIT
