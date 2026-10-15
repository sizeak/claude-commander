# Chat integration

Drive the [commander](configuration.md) from a chat platform. Allowlisted users
mention the bot in a channel or message it directly; each message is handled by
a short-lived headless commander that answers questions, creates or inspects
sessions, and replies in the thread. Worker sessions report back to the
originating thread with `claude-commander notify`.

Chat is an **augment, not a replacement** for the TUI: there is no output
mirroring and no long-running chat "session". Every mention or direct message is
one task — the agent does it, replies, and stops; a later message starts it
again with the thread as context.

**Backends.** Each platform is a *chat backend*. [Slack](#slack) is the one
available today; others (Discord, Teams) plug into the same interface. Several
can run at once: each has its own credentials and allowlist, a session remembers
which backend it came from, and `notify` answers on that one.

## Building with chat support

Chat backends are **opt-in at build time**, so default builds — including the
packaged ones — carry no chat SDK. Enable a backend with its cargo feature, on
whichever binary will serve:

```sh
cargo install --path crates/claude-commander --features slack          # TUI + in-process server
cargo install --path crates/claude-commander-server --features slack   # standalone server
```

A backend configured in `config.toml` but missing from the build logs a warning
at startup (`[slack] is configured, but this build has no Slack support`), and
`notify` answers that no chat backend is running.

**Install the `claude-commander` CLI from the same build.** The chat commander
drives Commander through the `claude-commander` CLI, and that CLI enforces every
limit on what a chat request may do. The server puts its own directory first on
the commander's `PATH`, and refuses to answer chat requests if the
`claude-commander` it would run is a different version (for example a packaged
release next to a source-built server).

## Where it runs

Backends run inside the Commander **server**: either `claude-commander-server`,
or the TUI's own in-process server (`[server] auto_start = true`, or
`claude-commander --serve`). Both start every backend that is built in and
configured, and both write the `server-info.json` that `notify` uses to find
them. **Never run both**: on different ports they would each connect the same
Slack app and answer every message twice.

Chat is gated by the backend tables alone, independent of `commander_enabled`.
**Backend changes require a restart**: `[slack]` and `[chat]` are read once when
the server starts; `POST /api/config/reload` does not reconfigure a running
bridge. The `[headless_commander]` tunables apply after a reload.

## Configuration

Each backend has its own table (see [Slack](#slack) for `[slack]`). Two tables
apply to every backend:

```toml
[chat]
# default_notify = "slack"         # backend for notifies from sessions with no
#                                  # chat origin (default: the first enabled one)

[headless_commander]
# invocation_timeout_secs = 300    # kill + one retry after this long
# linger_secs = 300                # keep a process warm for thread follow-ups
# warm_pool = true                 # pre-warm one process for new threads
# warm_respawn_secs = 3600         # respawn the warm process this often
```

Backend tokens are **secrets**: they're stripped from `GET /api/config` and
rejected by the remote config-patch route, so they can only be set by editing
`config.toml` directly.

## Usage

- **In a channel:** mention the bot — `@commander what are my sessions doing?` —
  and it replies in a thread. Backends that support it show progress on your
  message (Slack: 👀, then ✅ or ❌).
- **In a direct message:** message the bot; same flow, no mention needed.
- **Thread follow-ups:** reply in the same thread (mentioning the bot again in a
  channel) to continue — the bridge keeps one Claude conversation per thread, so
  it remembers the earlier context. A recently-used thread answers instantly
  (the process lingers for `linger_secs`).

Only a backend's allowlisted users are answered; everyone else is silently
ignored. The agent writes ordinary Markdown and the backend converts it to the
platform's format; replies lead with the answer, since they're often read on a
phone.

### Creating sessions from chat

Ask the bot to start work (e.g. "spin up a session to fix the login bug in the
api repo") and it runs `claude-commander new`, distilling the thread into an
initial prompt and picking the project from your project list. When the request
is ambiguous it asks a clarifying question in the thread rather than guessing. A
session created this way is stamped with its chat **origin** (`--origin-backend`,
`--origin-channel`, `--origin-thread`, and `--origin-permalink` when the platform
has links) so a `notify` reports back to that exact thread.

From chat the commander can only start a session in a project you've already
registered, on a new branch from that project's default base (see Security). The
worker runs unattended in the default (or `plan`) permission mode, so it stops at
its first permission prompt — and, in a new worktree, possibly Claude's
folder-trust question — until someone answers it in the TUI. The worker may run
`claude-commander notify` without asking, so it can say when it's blocked; the
commander's reply also mentions it.

## Reporting back with `notify`

A worker session tells chat it's done — or asks a question — with:

```sh
claude-commander notify --message "Tests pass, PR is up: <url>"
```

- **Session:** defaults to the session whose worktree contains the current
  directory; override with `--session <name-or-id>`.
- **Message:** `--message <text>` (Markdown), or piped on stdin if omitted.
- **Destination:** a session created from chat posts back into its **origin
  thread** on its origin backend; a session with no origin is sent as a
  **direct message** to the first allowlisted user of `[chat] default_notify`
  (or the first enabled backend), labelled with the session's name.

`notify` discovers the running server through its `server-info.json` runtime
file (written into the data dir, `0600`, and removed when the server stops) —
no URL or token to configure. It exits with a clear message if no server is
running, no backend is enabled, or the session's backend isn't running. Workers
never hold platform credentials; the server owns the only chat clients and
performs the delivery.

Worker agents learn when and how to call this from the bundled
`claude-commander-notify` skill, which Commander writes into every session's
worktree (see [Agent skills](usage.md#agent-skills)).

## Security

Chat is a powerful entry point — it can create sessions that run programs on the
server machine — so it's locked down by design:

- **Allowlist gates *triggering*, not *content*.** Only a backend's allowlisted users
  can trigger the bot; an empty allowlist disables the whole feature and there is no
  "allow everyone" mode. But the allowlist does **not** limit what *reaches* the
  agent: when an allowlisted user invokes the bot inside a thread, the bridge
  fetches that thread's history and includes it in the prompt — so text written
  by **anyone** who posted in the thread (allowlisted or not) is fed to the
  agent. Thread content is fenced and labelled as untrusted context, not
  commands, but treat it as attacker-controlled input and keep the allowlist
  tight and the channels trusted.
- **Agent lockdown, deny-by-default.** The headless commander may run the
  `claude-commander` CLI and read files **inside the worktrees directory** —
  nothing else: no `Write`/`Edit`, no other shell, no reads elsewhere (its own
  working directory aside). It loads only its own project settings
  (`--setting-sources project`) and no MCP servers (`--strict-mcp-config`), so
  your user-level allow rules and MCP servers never widen what it can do, and
  each invocation is capped by `invocation_timeout_secs`. Every CLI call that
  descends from it (detected by an inherited environment marker the agent cannot
  forge or strip) is restricted further:

  - **Subcommands:** only `list`, `projects`, `status`, `log` and `new`.
    `log` only reads sessions created from chat, at most 200 lines — your own
    sessions' scrollback never reaches a channel. Everything else — `delete`,
    `config`, `commander`, `listen-toggle`, `keep-alive`, `attach`, and any
    command added later — is refused.
  - **Where `new` may start:** only an already-registered project named with
    `--project` (name or id), on a **new** branch from its default base.
    `--path`, `--base-branch`, `--workspace` and `--remote` are refused, and so
    is a session name whose branch already exists locally or on `origin`:
    creating a worktree runs repository code (a flake's `nix develop`
    shellHook, for one), so a chat request must not choose the directory or
    branch, or a server this check can't see.
  - **What `new` may launch:** only a **Claude** program from `programs` or the
    configured default, carrying no permission-overriding flags
    (`--dangerously-skip-permissions`, `--permission-mode`, `--settings`,
    `--allowedTools`). Other agents are refused because only Claude's
    permission modes keep an unattended worker behind a prompt.
  - **How it runs:** only no mode, `default` or `plan`, and the mode is always
    passed explicitly, so the worker can't inherit your `defaultMode`. The
    initial prompt follows `--`, so prompt text can't be parsed as a flag. The
    worker gets one standing allowance — `claude-commander notify` — so it can
    report back (or say it's blocked) without a prompt nobody would see.

  This does not make the agent untrusted-input-proof in general: allowlisted
  **users** are trusted, thread **history** is treated as untrusted and fenced
  (see above), and the CLI's own guarded surface is the boundary — do not widen
  the configured programs to anything that shells out freely.
- **Read fencing.** Reads are allowed only inside the worktrees directory; a
  `permissions.deny` list additionally blocks `.env`/`.env.*` files and known
  credential locations (`~/.ssh`, `~/.aws`, `~/.gnupg`, git/gh/docker/kube
  credentials, Claude Code's login and transcripts, the commander **config
  dir** holding the chat tokens, and the **data dir**'s JSON state files) even
  there. Anything a session prints or commits inside its worktree is readable.
  `claude-commander config` prints the config with every credential redacted.
- **Secrets stay local.** Chat tokens never leave the server's `config.toml`
  (redacted from the API, not remotely patchable), and worker sessions relay
  through the server rather than holding platform credentials themselves.

## Slack

The Slack backend connects over **Socket Mode** (no public URL needed), answers
`@mentions` in channels and direct messages, shows progress as reactions, and
renders replies as Slack *mrkdwn*. Build with `--features slack`.

### 1. Create the Slack app

The fastest path is a **manifest**. In Slack, go to
<https://api.slack.com/apps> → **Create New App** → **From an app manifest**,
pick your workspace, choose the **YAML** tab, and paste:

```yaml
display_information:
  name: Commander
  description: Drive claude-commander from Slack
features:
  bot_user:
    display_name: commander
    always_online: true
oauth_config:
  scopes:
    bot:
      - app_mentions:read   # receive @mentions in channels
      - chat:write          # post replies and DMs
      - reactions:write     # add/remove the 👀 / ✅ / ❌ status reactions
      - channels:history    # read thread context in public channels
      - groups:history      # read thread context in private channels
      - im:history          # read/receive direct messages
      - im:write            # open a DM channel for notify fallback
settings:
  event_subscriptions:
    bot_events:
      - app_mention         # @mention in a channel
      - message.im          # direct message to the bot
  interactivity:
    is_enabled: false
  socket_mode_enabled: true
  org_deploy_enabled: false
  token_rotation_enabled: false
```

These are exactly the scopes and events the bridge uses — nothing more:

| Scope | Why it's needed |
|-------|-----------------|
| `app_mentions:read` | Deliver `app_mention` events when the bot is @mentioned in a channel |
| `chat:write` | Post the reply into the thread (`chat.postMessage`) and DM notify fallbacks |
| `reactions:write` | Add 👀 on receipt and swap it for ✅/❌ (`reactions.add` / `reactions.remove`) |
| `channels:history` | Fetch thread context in a **public** channel (`conversations.replies`) |
| `groups:history` | Fetch thread context in a **private** channel |
| `im:history` | Receive `message.im` DM events and fetch DM thread context |
| `im:write` | Open a DM channel for the notify DM fallback (`conversations.open`) |

The bridge also calls `auth.test` (to learn its own user id for precise mention
stripping) and `chat.getPermalink` — neither needs an extra scope. It does **not**
call `users.info`, so `users:read` is intentionally omitted.

### 2. Get the tokens

The bridge needs two tokens:

- **Bot token** (`xoxb-…`) — after creating the app, go to **OAuth &
  Permissions** → **Install to Workspace**, approve, then copy the **Bot User
  OAuth Token**. This is `bot_token`.
- **App-level token** (`xapp-…`) — Socket Mode needs its own token. Go to **Basic
  Information** → **App-Level Tokens** → **Generate Token and Scopes**, add the
  `connections:write` scope, generate, and copy it. This is `app_token`. (The
  manifest can't declare app-level tokens, so this step is manual even when you
  used the manifest above.)

Keep both secret — anyone with them can post as your bot and drive your
commander.

### 3. Find your Slack user id

`allowed_user_ids` is a list of Slack **user ids** (not display names or
`@handles`), each looking like `U0123456789`. To find yours: click your profile
in Slack → **⋮ (More)** → **Copy member ID**. Repeat for anyone else who should
be allowed to invoke the bot.

### 4. Configure claude-commander

Add the `[slack]` table to your `config.toml` (see
[Configuration](configuration.md) for the file location):

```toml
[slack]
app_token = "xapp-..."             # Socket Mode app-level token (secret)
bot_token = "xoxb-..."             # bot user OAuth token (secret)
allowed_user_ids = ["U0123456789"] # who may invoke the bot
```

The backend is enabled only when **both tokens are set and `allowed_user_ids` is
non-empty** — an empty allowlist keeps it off, so a token pair alone can't
accidentally accept everyone. Restart the server after editing; on connect it
logs `slack: connecting` and then `connecting via Socket Mode` (the TUI writes
its log to `/tmp/claude-commander.log` with `--debug`).

Then invite the bot to any channel you want to use it in (`/invite @commander`).
