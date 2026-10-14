---
name: claude-commander-slack-notify
description: Report back to Slack about this Claude Commander session — task finished, PR opened, blocked on a question. Use when your task came from Slack (the initial prompt mentions a Slack request or thread), or when the user asks you to "let them know on Slack", "post an update", or "notify Slack".
---

# Report back to Slack

`claude-commander slack notify` relays a message to Slack about your session. A
session created from a Slack request posts into the **thread it came from**; any
other session is sent as a DM to the user, labelled with the session's name.

```bash
claude-commander slack notify --message "Tests pass, PR is up: https://github.com/org/repo/pull/123"
```

- Run it from inside your worktree and it finds your session automatically
  (`--session <name-or-id>` overrides that).
- Without `--message`, the text is read from stdin.
- The complete flag list is in the `claude-commander-cli-reference` skill.

## When to send one

- **Your task came from Slack.** Send one message when you finish (what you did,
  plus the PR link if there is one), and one if you are blocked on a decision
  only the requester can make. The requester is not watching your terminal.
- **The user asks you to.** Otherwise don't: unprompted progress chatter is
  noise in someone's Slack.
- Never more than a handful per task. Don't use it to narrate steps.

## Writing the message

It's read on a phone, in Slack's *mrkdwn* dialect, not Markdown:

- Lead with the outcome in a sentence or two.
- Bold is `*single asterisks*`, links are `<https://example.com|text>`, and
  there are no `#` headings.
- When you are blocked, ask the one question you need answered, with the
  options if there are any.

## If it fails

The command needs a running `claude-commander-server` with Slack configured, and
it exits non-zero with a plain message when that isn't the case. Say so once in
your final reply and carry on; don't retry in a loop or try to reach Slack any
other way.
