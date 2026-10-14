---
name: claude-commander-cli-reference
description: Complete reference for the `claude-commander` CLI — every subcommand and flag, matching the installed version. Use it before running a `claude-commander` command whose arguments you are not certain of.
---

<!-- Generated from crates/claude-commander/src/cli_args.rs by skills_gen.rs. Do not edit; regenerate with: CC_SKILLS_REGENERATE=1 cargo test -p claude-commander skills_gen -->

# claude-commander CLI reference

Each section is the `--help` output of one subcommand.

### `claude-commander tui`

```
Start the interactive TUI (default)

Usage: claude-commander tui

Options:
  -h, --help
          Print help
```

### `claude-commander list`

```
List all sessions

Usage: claude-commander list [OPTIONS]

Options:
  -a, --all
          Show all sessions including stopped ones

      --json
          Output as JSON

  -w, --workspace <WORKSPACE>
          Only list sessions in this workspace (by name, case-insensitive; `main` or Main's label for the built-in workspace)

  -h, --help
          Print help
```

### `claude-commander status`

```
Show detailed status of a session

Usage: claude-commander status [OPTIONS] <SESSION>

Arguments:
  <SESSION>
          Session name or ID prefix

Options:
      --json
          Output as JSON

  -h, --help
          Print help
```

### `claude-commander delete`

```
Delete a session: kill its tmux session and remove the git worktree

Usage: claude-commander delete [OPTIONS] <SESSION>

Arguments:
  <SESSION>
          Exact session title (case-insensitive) or full session ID. Prefixes are rejected so a typo can't delete the wrong session

Options:
  -f, --force
          Delete without confirmation

  -h, --help
          Print help
```

### `claude-commander keep-alive`

```
Toggle (or explicitly set) keep-alive on a session, exempting it from automatic idle-hibernation. With no flag, toggles the current value

Usage: claude-commander keep-alive [OPTIONS] <SESSION>

Arguments:
  <SESSION>
          Exact session title (case-insensitive) or full session ID

Options:
      --on
          Turn keep-alive on (never auto-hibernate this session)

      --off
          Turn keep-alive off (allow auto-hibernation when idle)

  -h, --help
          Print help
```

### `claude-commander log`

```
Dump recent terminal output from a session

Usage: claude-commander log [OPTIONS] <SESSION>

Arguments:
  <SESSION>
          Session name or ID prefix

Options:
  -l, --lines <LINES>
          Number of scrollback lines to capture (default: 100, max: 10000)
          
          [default: 100]

  -h, --help
          Print help
```

### `claude-commander new`

```
Create a new session

Usage: claude-commander new [OPTIONS] <NAME>

Arguments:
  <NAME>
          Session name

Options:
  -p, --program <PROGRAM>
          Program to run (default: claude)

  -d, --path <PATH>
          Project path (default: current directory)

      --project <PROJECT>
          Existing project to create the session in, by name (case-insensitive). Resolves to the project's repo path via the backend, so a remote project needs no `--path`. If the name matches more than one project (names aren't unique), use `--path` to disambiguate. Mutually exclusive with `--path` (which seeds a new project)

  -i, --initial-prompt <INITIAL_PROMPT>
          Initial prompt to send to the Claude agent

  -e, --effort <EFFORT>
          Claude effort level

  -m, --mode <MODE>
          Claude permission mode

      --model <MODEL>
          Model to run the agent with (Claude, Codex, OpenCode or omp)

  -b, --base-branch <BASE_BRANCH>
          Branch to fork from (default: origin/main)

  -s, --section <SECTION>
          Place session in a specific section

      --remote <REMOTE>
          Create the session on a configured remote server (by name from `[[remote_servers]]`) instead of locally. Pair with `--project` to pick an existing server-side project by name, or `--path` to seed a new one (the path is resolved on the server, not this machine)

      --slack-channel <SLACK_CHANNEL>
          Slack channel id to record as this session's origin. Set by the commander agent when creating a session from a Slack request so a worker's `slack notify` can report back to the thread. Requires `--slack-thread-ts`

      --slack-thread-ts <SLACK_THREAD_TS>
          Slack thread timestamp for the session's origin. Requires `--slack-channel` (the two are all-or-none)

      --slack-permalink <SLACK_PERMALINK>
          Permalink to the originating Slack message, embedded in the session origin. Requires `--slack-channel` (and thus `--slack-thread-ts`)

  -w, --workspace <WORKSPACE>
          Workspace for the project when `--path` registers a new one (an existing project keeps its workspace). An unknown name creates the workspace; `main` is the built-in one

  -h, --help
          Print help
```

### `claude-commander attach`

```
Attach to an existing session

Usage: claude-commander attach [OPTIONS] <SESSION>

Arguments:
  <SESSION>
          Session name or ID

Options:
      --remote <REMOTE>
          Attach to a session on a configured remote server (by name from `[[remote_servers]]`) instead of a local one

  -h, --help
          Print help
```

### `claude-commander commander`

```
Open the persistent commander session — a project-less Claude session that coordinates other sessions via this CLI. Creates it on first use

Usage: claude-commander commander

Options:
  -h, --help
          Print help
```

### `claude-commander slack`

```
Slack integration commands

Usage: claude-commander slack <COMMAND>

Commands:
  notify  Relay a message to Slack about a session. A session created from Slack posts back into its originating thread; any other session is DM'd to the first allowlisted user. Worker sessions run this to report progress
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### `claude-commander slack notify`

```
Relay a message to Slack about a session. A session created from Slack posts back into its originating thread; any other session is DM'd to the first allowlisted user. Worker sessions run this to report progress.

Requires a running `claude-commander-server` (the server owns the Slack client and is discovered via its runtime info file).

Usage: claude-commander slack notify [OPTIONS]

Options:
      --session <SESSION>
          Session to notify about (name or id prefix). Defaults to the session whose worktree contains the current directory

  -m, --message <MESSAGE>
          Message text. If omitted, the message is read from stdin to EOF

  -h, --help
          Print help (see a summary with '-h')
```

### `claude-commander config`

```
Show configuration

Usage: claude-commander config [OPTIONS]

Options:
      --init
          Initialize config file with defaults

  -h, --help
          Print help
```

### `claude-commander listen-toggle`

```
Toggle voice input in the running TUI. Intended for binding to a desktop global keyboard shortcut (e.g. a KDE Plasma custom shortcut) so STT can be triggered system-wide, not just when the terminal is focused. Talks to the already-running instance over a local socket; with no flag it toggles

Usage: claude-commander listen-toggle [OPTIONS]

Options:
      --start
          Start recording (instead of toggling)

      --stop
          Stop recording and transcribe (instead of toggling)

  -h, --help
          Print help
```
