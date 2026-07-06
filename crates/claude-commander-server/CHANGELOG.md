# Changelog — claude-commander-server

Notable changes to the HTTP/WebSocket API surface. Client authors should track
route-contract changes here.

## Unreleased

- `/projects/scan` is now `POST` (was `GET`). Scanning *mutates* state — it adds
  every discovered repository as a project — so it takes a JSON body
  (`{ "path": "…" }`) and returns `{ "added", "skipped" }`. Update any client
  that still issues a `GET`.
