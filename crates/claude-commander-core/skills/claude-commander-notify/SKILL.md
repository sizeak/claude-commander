---
name: claude-commander-notify
description: Report back to chat about this Claude Commander session (task finished, PR opened, blocked on a question). Use when your task came from a chat request (Slack or another platform; the initial prompt mentions a chat request or thread), or when the user asks you to "let them know on Slack", "post an update", "notify me" or "message the channel".
---

# Report back to chat

`claude-commander notify` relays a message about your session to chat. A session
created from a chat request posts into the **thread it came from**; any other
session is sent as a direct message to the user on the default chat backend,
labelled with the session's name.

```bash
claude-commander notify --message "Tests pass, PR is up: https://github.com/org/repo/pull/123"
```

- Run it from inside your worktree and it finds your session automatically
  (`--session <name-or-id>` overrides that).
- Without `--message`, the text is read from stdin.
- The complete flag list is in the `claude-commander-cli-reference` skill.

## When to send one

- **Your task came from chat.** Send one message when you finish (what you did,
  plus the PR link if there is one), and one if you are blocked on a decision
  only the requester can make. The requester is not watching your terminal.
- **The user asks you to.** Otherwise don't: unprompted progress chatter is
  noise in someone's chat.
- Never more than a handful per task. Don't use it to narrate steps.

## Writing the message

Write ordinary Markdown; it's converted for the chat platform. It's read on a
phone, so:

- Lead with the outcome in a sentence or two.
- Include links as plain URLs or `[text](url)`.
- When you are blocked, ask the one question you need answered, with the
  options if there are any.

## If it fails

The command needs a running Commander server with a chat backend enabled, and
it exits non-zero with a plain message when that isn't the case. Say so once in
your final reply and carry on; don't retry in a loop or try to reach the chat
platform any other way.
