# claude-commander-web

A standalone **web UI** for [claude-commander-server](../claude-commander-server).
It serves the browser SPA and reverse-proxies `/api` + `/ws/attach` to a running
server, so the browser is always same-origin (no CORS to configure). It is a pure
*client* — it never links `claude-commander-core` (no tmux/gix), so it stays
small and portable.

```
Browser ──http/ws──► claude-commander-web ──http/ws──► claude-commander-server
  (SPA + same-origin /api + /ws)     (reverse proxy + auth)
```

## Auth modes

The mode is chosen by whether you pass the commander bearer token at launch:

- **BFF** (`--commander-token …`): the browser logs in with **HTTP Basic auth**
  (`--username`/`--password`); the binary injects the bearer token on every
  upstream call and the in-band `auth` WS frame. **The token never reaches the
  browser.**
- **Pass-through** (no token): the browser supplies the commander token itself (a
  connect screen), and the binary forwards it upstream verbatim.

## Run

```sh
# BFF: token stays server-side; you log in with a password
claude-commander-web \
  --commander-url http://127.0.0.1:7878 \
  --commander-token "$CC_SERVER_TOKEN" \
  --username admin --password "$WEB_PASSWORD" \
  --bind 127.0.0.1 --port 8420

# Pass-through: browser holds the token
claude-commander-web --commander-url http://127.0.0.1:7878 --port 8420
```

`--commander-token` / `--password` also read `CC_WEB_COMMANDER_TOKEN` /
`CC_WEB_PASSWORD` from the environment.

## HTTPS

Bind loopback and put it behind TLS on untrusted networks. With Tailscale:

```sh
sudo tailscale serve --bg --https=443 http://127.0.0.1:8420
```

`GET /webui/config` returns `{"mode":"bff"|"direct"}` so the SPA knows whether to
show a password login or a token connect screen.
