# Slack integration

Drive the [commander](configuration.md) from Slack. Allowlisted users
`@mention` the bot in a channel or DM it directly; each message is handled by a
short-lived headless commander that answers questions, creates or inspects
sessions, and replies in-thread. Worker sessions can report progress back to the
originating Slack thread with `claude-commander slack notify`.

Slack is an **augment, not a replacement** for the TUI: there is no output
mirroring and no long-running Slack "session". Every mention or DM is one task —
the agent does it, replies, and stops; a later message starts it again with the
thread as context.

> **Where it runs:** the bridge lives inside `claude-commander-server`, not the
> TUI. You need a running server (see the [server crate](../crates/claude-commander-server))
> for Slack to work. The bridge is gated by the `[slack]` config table alone,
> independent of `commander_enabled`.

## 1. Create the Slack app

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

## 2. Get the tokens

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

## 3. Find your Slack user id

`allowed_user_ids` is a list of Slack **user ids** (not display names or
`@handles`), each looking like `U0123456789`. To find yours: click your profile
in Slack → **⋮ (More)** → **Copy member ID**. Repeat for anyone else who should
be allowed to invoke the bot.

## 4. Configure claude-commander

Add the `[slack]` table to your `config.toml` (see
[Configuration](configuration.md) for the file location and the full field
reference):

```toml
[slack]
app_token = "xapp-..."             # Socket Mode app-level token (secret)
bot_token = "xoxb-..."             # bot user OAuth token (secret)
allowed_user_ids = ["U0123456789"] # who may invoke the bot
# invocation_timeout_secs = 300    # kill + one retry after this long
# linger_secs = 300                # keep a process warm for thread follow-ups
# warm_pool = true                 # pre-warm one process for new threads
# warm_respawn_secs = 3600         # respawn the warm process this often
```

The bridge is enabled only when **both tokens are set and `allowed_user_ids` is
non-empty** — an empty allowlist keeps it off, so a token pair alone can't
accidentally accept everyone. Restart `claude-commander-server` after editing;
on connect it logs `Slack bridge connecting via Socket Mode`.

The two tokens are **secrets**: they're stripped from `GET /api/config` and
rejected by the remote config-patch route, so they can only be set by editing
`config.toml` directly.

## Usage

Invite the bot to any channel you want to use it in (`/invite @commander`), then:

- **In a channel:** `@commander what are my sessions doing?` — the bot reacts 👀,
  works, and replies in a thread, swapping the reaction to ✅ (or ❌ on error or
  timeout).
- **In a DM:** message the bot directly; same flow, no @mention needed.
- **Thread follow-ups:** reply in the same thread (re-@mentioning in a channel)
  to continue the conversation — the bridge keeps one Claude conversation per
  thread, so it remembers the earlier context. A recently-used thread answers
  instantly (the process lingers for `linger_secs`).

Only users in `allowed_user_ids` are answered; everyone else is silently
ignored. Replies use Slack *mrkdwn* (single-asterisk `*bold*`, no `#` headings)
and lead with the answer, since they're often read on a phone.

### Creating sessions from Slack

Ask the bot to start work (e.g. "spin up a session to fix the login bug in the
api repo") and it runs `claude-commander new`, distilling the thread into an
initial prompt and picking the project from your project list. When the request
is ambiguous it asks a clarifying question in-thread rather than guessing. A
session created this way is stamped with its Slack **origin** (channel +
thread + permalink) so a `notify` reports back to that exact thread.

## Reporting back with `notify`

A worker session tells Slack it's done — or asks a question — with:

```sh
claude-commander slack notify --message "Tests pass, PR is up: <url>"
```

- **Session:** defaults to the session whose worktree contains the current
  directory; override with `--session <name-or-id>`.
- **Message:** `--message <text>`, or piped on stdin if omitted.
- **Destination:** a session created from Slack posts back into its **origin
  thread**; a session with no origin is **DM'd to the first allowlisted user**,
  with the message labelled by session name so it's identifiable out of context.

`notify` discovers the running server through its `server-info.json` runtime
file (written on boot into the data dir, `0600`) — no URL or token to configure.
It exits with a clear message if the server isn't running or Slack isn't
enabled. Workers never hold Slack credentials; the server owns the only Slack
client and performs the delivery.

> The skill that teaches worker agents to call `slack notify` lives in the
> external claude-marketplace repo, not here.

## Security

Slack is a powerful entry point — it can create sessions that run programs on the
server machine — so it's locked down by design:

- **Allowlist.** Only `allowed_user_ids` are answered; an empty allowlist
  disables the whole feature. There is no "allow everyone" mode.
- **Agent lockdown.** The headless commander runs with a restricted tool set (the
  `claude-commander` CLI plus read-only Read/Grep/Glob) — no arbitrary shell —
  and each invocation is capped by `invocation_timeout_secs`.
- **Untrusted thread text.** Message and thread content fetched from Slack is
  passed to the agent as **context, not commands** — treat anything a user (or a
  quoted message) types in a thread as untrusted input. Keep the allowlist tight.
- **Secrets stay local.** The tokens never leave the server's `config.toml`
  (redacted from the API, not remotely patchable), and worker sessions relay
  through the server rather than holding Slack credentials themselves.
</content>
</invoke>
