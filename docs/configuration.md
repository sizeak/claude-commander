# Configuration

Configuration, theming, section grouping, and data-storage paths for Claude
Commander. For installation and the keyboard / status-symbol reference, see the
[README](../README.md). For the usage guide, see [Usage](usage.md).

Configuration file location depends on your platform:

- **macOS**: `~/Library/Application Support/com.claude-commander.claude-commander/config.toml`
- **Linux**: `~/.config/claude-commander/config.toml`

```toml
# Selectable agent harnesses for the New Session dialog's program picker. Each
# entry pairs a display `label` with the `command` launched (program plus any
# flags); the command's first token determines the harness, so Claude Code
# (`claude`) and OpenAI Codex (`codex`) are both recognised and get the right
# launch, resume, and working/waiting detection. The first entry is the default
# for new sessions. When `programs` is omitted, the picker offers a single
# built-in `claude` entry.
#
# Legacy configs using `default_program = "..."` are migrated automatically:
# the legacy command is moved or inserted as the first `[[programs]]` entry and
# `default_program` is removed from the file.
#
# In the New Session dialog, press Tab (or Shift+Tab to go back) to cycle focus
# between the name field, the project picker, and the program picker, then ↑/↓
# to choose. The project picker defaults to the currently-selected project and
# can be typed into to filter the list — so the usual "type a name, hit Enter"
# flow is unchanged.
#
# Program entries can be managed in-app from Settings ▸ Programs (settings
# modal, `,` key) — add (`n`), rename (`r`), edit the label/command fields, and
# remove (`d`) entries, or reorder them with `J`/`K`. Reordering changes the
# default, since the first entry is the default for new sessions.
#
# [[programs]]
# label = "Claude"
# command = "claude"
#
# [[programs]]
# label = "Claude (Opus, plan mode)"
# command = "claude --model opus --permission-mode plan"
#
# [[programs]]
# label = "Codex"
# command = "codex"
#
# [[programs]]
# label = "Codex (full auto)"
# command = "codex --full-auto"

# Branch name prefix for new sessions (empty = no prefix)
branch_prefix = ""

# Fetch latest changes from origin before creating a new session
fetch_before_create = true

# Skip git-LFS smudging during `git worktree add` so session creation is fast on
# LFS repos (the checkout leaves cheap pointer files). The real LFS content is
# fetched afterwards with `git lfs pull` — asynchronously in the TUI (with a
# `⇣ LFS` indicator on the session row) or synchronously on the CLI. While the
# background pull runs, the agent briefly sees pointer files rather than real
# content. Set to false to smudge during checkout as before.
skip_lfs_smudge = true

# Pass `--resume` when restarting/recreating a session so the agent picks up
# where it left off. Set to false to start the program fresh each time.
resume_session = true

# Automatically hibernate idle sessions to free memory (see "Idle-session
# hibernation" below). A background loop stops the tmux process of sessions
# that have sat idle-and-unattended past the timeout, keeping the worktree and
# metadata; the session transparently resumes on next attach. Disabled by
# default. Enabling (and the check interval) is restart-required; the idle
# timeout is read live.
# hibernate_enabled = false
# Idle duration (seconds) before an eligible session is hibernated. Default
# 86400 (1 day). The in-app settings editor enforces a minimum of 60;
# hand-edited values here are used as-is.
# hibernate_idle_timeout_secs = 86400
# Interval (seconds) between hibernation policy checks. Default 600 (10 min).
# The in-app settings editor enforces a minimum of 10; hand-edited values here
# are used as-is, and 0 disables the loop entirely.
# hibernate_check_interval_secs = 600

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

# Isolate every tmux command onto a throwaway socket dir (default: unset).
# For hermetic tests and the e2e harness ONLY — leave unset for normal use.
# When set, tmux commands run with TMUX_TMPDIR=<dir> and $TMUX/$TMUX_PANE
# stripped, so they hit a per-run tmux server instead of your real one.
# tmux_tmpdir = "/path/to/throwaway/tmux"

# Base directory for pasted-image temp files, remote image paste (default: the
# OS temp dir). For hermetic tests ONLY — leave unset for normal use; when set,
# image writes and their pruning go here instead of the OS temp dir.
# paste_images_dir = "/path/to/throwaway/paste-images"

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
# Run "Refresh PR status" from the command palette to force an immediate
# re-check without waiting for this interval to elapse.
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
# for a single-program setup. Disabled by default; set to true to show it.
# show_session_program = false

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
# CLI. Disabled by default; enabling it is restart-required (the open path and
# the footer chip both key off the value read at launch). While running, a
# `● Commander` chip in the footer status bar shows its live state.
# commander_enabled = false
# Program (with flags) for the commander; defaults to the first `[[programs]]`
# entry.
# commander_program = "claude --model opus-4-7"
# Working directory for the commander; defaults to <data dir>/commander.
# commander_dir = "/path/to/commander"

# Conversation mode: a full-screen chat (open with `Alt-c`) backed by a
# dedicated headless Claude session, whose replies stream in and are spoken
# aloud via an OpenAI-compatible TTS engine. See "Conversation mode" below.
# [conversation]
# enabled = true                          # master switch: Alt-c overlay + spoken replies (off by default)
# name = "Claudette"                       # assistant's display name / nickname
# command = "claude"                       # binary for the conversation session
# permission_mode = "auto"                 # --permission-mode for the agent (acts without approval prompts)
# base_url = "http://127.0.0.1:8002/v1"   # OpenAI-compatible TTS endpoint (include /v1)
# model = "kokoro"                         # TTS model name (engines serving one model ignore it)
# voice = "af_sky"                         # omit to use the server's default voice
# response_format = "wav"                  # wav | mp3 | opus | flac (wav = lowest local latency)
# speed = 1.0                              # 0.25–4.0
# speak_scope = "prose_only"               # prose_only | verbatim (per-sentence, streamed)
# volume = 1.0                             # 0.0–2.0

# Voice input (speech-to-text): hold a conversation by talking. Toggle recording
# with `Alt-v`, then it's transcribed via an OpenAI-compatible STT engine and sent
# to the conversation agent. See "Voice input (STT)" below.
# [stt]
# enabled = true                          # master switch for Alt-v voice input (off by default)
# base_url = "http://127.0.0.1:8000/v1"   # OpenAI-compatible transcription endpoint (include /v1)
# model = "Systran/faster-whisper-base"    # transcription model name
# language = "en"                          # ISO-639-1 hint; omit to auto-detect
# prompt = "..."                           # optional decoding prompt (domain vocab / spelling)
# api_key = "..."                          # sent as a Bearer header; omit for local servers
# pause_media = true                       # pause other players while recording, resume after the
#                                          # reply (best-effort via playerctl/osascript; on by default)

# Custom key bindings — override any default key with one or more alternatives
# [keybindings]
# navigate_up = ["k", "Up"]
# next_group = ["]"]
# previous_group = ["["]
# navigate_first = ["Home"]
# navigate_last = ["End"]
# quit = ["q", "Ctrl-c"]
# toggle_pane = ["Tab"]
# toggle_keep_alive = ["K"]                # palette-only by default; bind a key here

# Remote claude-commander servers. Each entry adds a server node to the
# session tree with that server's projects and sessions under it (full
# create/delete/review/attach parity over HTTP + WebSocket). Manage these
# from the palette ("Add remote server" / "Remove remote server", which
# includes a connection test) or by hand here — the file is hot-reloaded.
# These are a list-of-tables, so unlike the scalar options above they are
# NOT editable from the in-app settings modal; use the palette commands.
# The name "local" is reserved for the built-in local machine.
# [[remote_servers]]
# name = "buildbox"              # unique display name (the tree header)
# url = "http://buildbox:7878"   # base URL of claude-commander-server
# token = "..."                  # bearer token; omit only for servers
#                                # started with --allow-no-auth (loopback)
```

A remote server's `token` is **operator-equivalent**: anyone holding it can create
sessions (which run arbitrary programs on that machine) and address projects by
server-side path. Treat it like an SSH key — don't commit it, don't share it, and
scope it to people you'd give a shell. On disk this file is protected only by its
`0600` permissions, so keep those intact.

Don't add a remote server that shares this machine's `state.json` (for example a
loopback `http://localhost:7878` server backed by the same data directory as your
local instance). Its sessions are already shown under the local machine, so they
would appear twice — once under the local root and once under the server header —
and rows under the server header could be misrouted to the local backend. A
loopback server is fine only when it runs against a **separate** data directory.
claude-commander logs a warning when a configured server's URL is a loopback
address as a reminder.

## Idle-session hibernation

Each live session holds a `claude` process open in tmux (~400MB RAM) whether or
not you're using it. With many sessions open, idle ones waste memory. When
`hibernate_enabled = true`, a background loop stops the tmux process of any
session that has been idle past `hibernate_idle_timeout_secs`, **keeping its
worktree, branch, and metadata** — only the process is stopped. The session
shows as `Stopped` and transparently resumes (with `--resume`, so the
conversation is preserved) the next time you attach.

A session counts as **idle** only when all of these hold:

- its agent is `Idle` — a `Working`, `WaitingForInput` (pending approval), or
  momentarily undetectable session is never hibernated;
- no tmux client is attached to it, and it wasn't attached within the last
  check interval or 30 seconds, whichever is longer (so recent attaches are
  protected slightly longer when the check interval is set below 30s);
- `keep_alive` is off for that session.

**Waking is automatic**: attaching (`Enter`) recreates the tmux session and
resumes the agent, even if `resume_session = false` globally — hibernation
always resumes, since that's what makes it non-destructive.

**Keeping a session alive**: run the **Toggle keep-alive** action from the
command palette on a session (or `claude-commander keep-alive <session>
[--on|--off]`) to exempt it from hibernation — useful for a long-running build,
a watched log, or anything you want to keep warm. A kept-alive session shows an
anchor (`⚓`) marker in the session list. The action has no default hotkey; bind
one via `toggle_keep_alive` under `[keybindings]` (e.g. `toggle_keep_alive =
["K"]`) if you want a shortcut. The flag persists across restarts.

Enabling hibernation and changing `hibernate_check_interval_secs` are
restart-required (the loop is spawned once at launch); `hibernate_idle_timeout_secs`
is read live.

## Conversation mode (TTS)

Press **`Alt-c`** to open a full-screen **conversation overlay** — a chat with a dedicated
Claude session whose replies **stream in and are spoken aloud** through any **OpenAI-compatible**
TTS engine (it posts to `{base_url}/audio/speech`). Type a message, press Enter; the reply is
spoken sentence-by-sentence *as it's generated*. Closing the overlay (Esc / `Alt-c`) leaves the
session running, so the conversation continues where you left off.

This session is **separate from the commander** (`Shift-C`) and unrelated to it. It's driven over
Claude Code's stream-json protocol (`claude -p --input-format stream-json --output-format
stream-json --include-partial-messages`), which is the supported way to get clean token-level
text — so TTS can start within a sentence of Claude beginning to type, rather than waiting for the
whole reply. A new message interrupts in-flight speech. If the TTS server is unreachable, the chat
still works (text-only) and never blocks the UI.

`enabled` is the master switch for the whole feature and is **off by default** — set it (in
Settings ▸ Conversation or config) before `Alt-c` will open the overlay. We develop against a
local [Kokoro](https://github.com/sizeak/kokoro-tts-rocm) container (default
`http://127.0.0.1:8002/v1`), but any OpenAI-compatible endpoint works.

```toml
[conversation]
enabled = true                          # speak replies via TTS (off = text-only chat)
command = "claude"                       # binary for the conversation session
base_url = "http://127.0.0.1:8002/v1"   # OpenAI-compatible TTS endpoint (include /v1)
model = "kokoro"                         # TTS model name (engines serving one model ignore it)
voice = "af_sky"                         # omit to use the server's default voice
response_format = "wav"                  # wav | mp3 | opus | flac (wav = lowest local latency)
speed = 1.0                              # 0.25–4.0
speak_scope = "prose_only"               # prose_only | verbatim
volume = 1.0                             # 0.0–2.0
```

`speak_scope` controls what's spoken (applied per sentence as it streams):

| Value | Behaviour |
|-------|-----------|
| `prose_only` (default) | Strip code blocks and markdown; speak the natural-language prose |
| `verbatim` | Speak the text unchanged |

> **Build note:** in-process playback (`rodio`) and microphone capture (`cpal`) link **ALSA** on
> Linux. They're gated behind the `audio` cargo feature, which is **on by default** — so building
> the TUI (`claude-commander`) from source needs the ALSA development headers (`libasound2-dev` on
> Debian/Ubuntu, `alsa-lib` on Arch); the default Nix dev shell provides them automatically. The
> headless server and the Flutter client build with `audio` off (`default-features = false`) and
> never link ALSA — remote clients do capture/playback on-device instead.

## Voice input (STT)

Press **`Alt-v`** to talk to the conversation agent. It's a **toggle**: the first press starts
recording the microphone, the next press stops it, transcribes the audio through any
**OpenAI-compatible** speech-to-text engine (it posts a WAV to `{base_url}/audio/transcriptions`),
and sends the resulting text to the conversation session — exactly as if you'd typed it. The reply
then streams back and is spoken aloud (if TTS is enabled). Voice input works **whether the overlay
is open or not**, mirroring spoken replies.

`stt.enabled` is a separate switch from `conversation.enabled` and is **off by default**. Voice
input feeds the conversation session, so it's only useful alongside conversation mode. Microphone
capture uses `cpal` (also ALSA on Linux — see the build note above). If no microphone is available
or the STT server is unreachable, voice input degrades gracefully (a status message) and never
blocks the UI.

```toml
[stt]
enabled = true                          # master switch for Alt-v voice input (off = no voice input)
base_url = "http://127.0.0.1:8000/v1"   # OpenAI-compatible transcription endpoint (include /v1)
model = "Systran/faster-whisper-base"    # transcription model name
language = "en"                          # ISO-639-1 hint; omit to auto-detect
# prompt = "Vitest, Kotlin, ..."         # optional decoding prompt (domain vocab / spelling hints)
# api_key = "..."                        # sent as a Bearer header; omit for local servers
pause_media = true                       # pause other players while recording, resume after the reply
```

While you're recording (and until the assistant has finished its spoken reply), `pause_media`
pauses any other media players so they don't talk over the conversation, then resumes whatever was
playing once things go quiet. It's best-effort — `playerctl` on Linux, `osascript` (Spotify/Music)
on macOS — and a silent no-op when neither is available, so it never blocks or breaks voice input.
On by default; set to `false` to leave your media alone.

Audio is captured at the microphone's native rate, downmixed to mono, and encoded as 16-bit PCM
WAV; the server resamples as needed. Recording isn't chunked yet — the whole utterance is uploaded
when you stop — so very long dictations wait until the end to transcribe.

### Global voice hotkey

`Alt-v` only fires when the terminal window is focused — a terminal app can't read key events
otherwise. To toggle voice input from **anywhere on the desktop**, bind a desktop-level global
shortcut to the `listen-toggle` command:

```sh
claude-commander listen-toggle          # toggle (also: --start / --stop)
```

This connects to the running TUI over a per-user Unix socket
(`$XDG_RUNTIME_DIR/claude-commander.sock`, falling back to the OS temp dir on macOS) and toggles
recording exactly as `Alt-v` would — and it works even while you're attached to a session. The
socket server starts automatically when the TUI launches with `stt.enabled = true`; the command
prints `recording`/`stopped` and exits non-zero (with a message) if no TUI is running.

- **KDE Plasma / Linux:** System Settings ▸ Shortcuts ▸ **Add ▸ Command or Script**, set the command
  to `claude-commander listen-toggle`, and assign your key. Pick a combo **different from `Alt-v`** so
  the global grab doesn't shadow the in-app binding.
- **macOS:** bind the same command via skhd, Karabiner-Elements, Raycast, or Shortcuts.app.

> A command shortcut is used (rather than the in-process XDG `GlobalShortcuts` portal) because the
> portal can't assign a non-sandboxed terminal binary a stable app identity, so the compositor never
> persists a reliably-bindable shortcut for it. The command route has no such limitation.

Both this and in-app `Alt-v` feed the same recording state, so they stay consistent. Only one running
TUI instance owns the socket; a second instance logs and skips.

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
| `max_sessions` | positive integer | Advisory WIP limit. Section header shows `count/limit` and highlights when at or over the limit. Never blocks creation. |

### Process order and forward-only

Config order is the pipeline. A session's section is re-evaluated on every PR refresh, but the scan **only considers sections at or after the session's current position** — auto never moves a session backwards. This keeps `"Needs Review"` sticky when a reviewer removes the label without leaving other signals; the session doesn't slide back to `"In Progress"`.

### Moving sessions manually

Select a session and press `m` (or open the palette with `Space`, or `Shift+Space` for commands-only, and run **Move session to section…**), then pick a target. An **Auto** entry clears an existing pin. The override is persisted to `state.json` and survives restarts; auto-moves are suppressed until the pin is released.

### Creating sessions inside a section

In the section-grouped views, a session created with `n` lands in the section the cursor was in, not the "In Progress" catch-all. For a manual-only waypoint (no predicates) this sets the same pin as a manual move; for a predicate-bearing section it's a soft placement — the session starts there but still auto-advances through the pipeline as its PR progresses. Creating from "In Progress" keeps the default behaviour. The CLI's `claude-commander new --section` flag follows the same rules.

### WIP limits

Set `max_sessions = N` on any section to flag it when it accumulates too much work. The header renders `count/N` and switches to the warning colour once `count >= N`. The catch-all "In Progress" section uses the top-level `in_progress_limit` instead:

```toml
in_progress_limit = 3

[[sections]]
name = "In Review"
pr_state = "open"
has_reviewer = true
max_sessions = 5
```

Limits are advisory — they never block session creation or section transitions. Sessions still flow through the pipeline as their PRs progress.

### Reordering, adding, or removing sections

These are edit-`config.toml`-and-restart actions — there's no hot reload. The cached `current_section` on each session is reconciled against the new config on next startup; if the referenced section no longer exists, the session falls back to `"In Progress"` and continues forward-only from there.

## Usage Telemetry

Claude Commander reports anonymous **feature-usage** telemetry so we can see which
features are used and retire the ones that aren't. It is **on by default** and
**opt-out**.

**What is sent:** the name of each feature you use (e.g. `review.open`,
`session.create`), a coarse environment fingerprint (OS, architecture, terminal
program, shell *name*, terminal colour mode), a non-sensitive config snapshot
(theme preset, view mode, which optional features are enabled), the frontend
name + version, the library version, and a random, resettable install id.

**What is never sent:** typed text, prompts, Claude session content, comment
bodies, branch/session names, repository paths, command arguments, or arbitrary
environment variables. The event schema is a fixed set of typed fields — there
is no code path that forwards free-form text.

**To disable**, either set the config flag:

```toml
[telemetry]
enabled = false

# Self-hosters can point telemetry at their own OpenObserve instead:
# endpoint = "https://o2.example.com/api/<org>/<stream>/_json"
# token = "<base64 of \"email:token\">"   # HTTP Basic credential
```

…or export the standard [`DO_NOT_TRACK`](https://consoledonottrack.com/) variable
(any non-empty, non-`0` value), which disables it regardless of config:

```sh
export DO_NOT_TRACK=1
```

The ingest credential is committed in the source tree, so **all** builds —
including ones compiled from source — report by default. Distro packagers who
want telemetry compiled out entirely can build with an empty token:

```sh
CC_TELEMETRY_TOKEN="" cargo build --release
```

## Data Storage

Paths are platform-specific, determined by the `directories` crate:

| File | macOS | Linux |
|------|-------|-------|
| Config | `~/Library/Application Support/com.claude-commander.claude-commander/config.toml` | `~/.config/claude-commander/config.toml` |
| State | `~/Library/Application Support/com.claude-commander.claude-commander/state.json` | `~/.local/share/claude-commander/state.json` |
| Worktrees | `~/Library/Application Support/com.claude-commander.claude-commander/worktrees/` | `~/.local/share/claude-commander/worktrees/` |
