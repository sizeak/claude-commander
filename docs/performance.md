# Responsiveness and refresh behavior

The performance work concentrates on keeping input handling independent of I/O,
refreshing after changes, and limiting work to visible content.

| Area | Behavior |
| --- | --- |
| TUI event loop | Draws when events invalidate the view. Idle ticks do not rebuild session lists or boards. Event draining has an 8 ms / 128-event budget. Animation, preview acquisition and age/config maintenance have separate deadlines. |
| TUI actions | Keep-alive, creation reconciliation, post-mutation snapshot refreshes, local project registration and directory scans complete in background tasks. Refresh revisions prevent older responses replacing newer views. |
| Preview | The protocol supports pane, shell, full diff and stats-only requests; callers omitting the selector retain the full payload. Live captures use a 100 ms deadline; Info stats use one second. Superseded tasks are cancelled and queued stale results are rejected. ANSI parsing, dimming and line metrics are cached; rendering borrows the cached paragraph. |
| Remote refresh | Authenticated `/api/changes?since=...` waits up to 20 seconds. Successful mutations, observable state changes and clone completion wake it. Native clients reuse workspace/agent payloads once, with mutation invalidation and a two-second age limit. Older servers keep their configured polling fallback. |
| Browser | Explicit refreshes stay independent of long waits. Healthy connections reconcile every 30 seconds; unsupported servers use 1.5 seconds. Hidden tabs reconcile every 60 seconds and refresh on visibility. Ended-terminal recovery temporarily restores the fast cadence. Unchanged cadence updates preserve the existing deadline. |
| Flutter lists/review | Session rows and review diff rows are built lazily with stable keys. Project/workspace membership indexes are reused per snapshot. Redundant nested store listeners were removed. Terminal throughput updates only its meter. Raw diff parsing has an exact-key cache limited to two reviews and 8 MiB of source keys. |
| Browser rendering | Unchanged project subtrees retain their DOM nodes. Review rows mount in 100-row chunks near the viewport, retaining mounted rows and composers. Comment anchors are indexed per file. |
| State/comments | No-op state mutations retain the existing durable file and generation. Pending-comment indexing is cached and invalidated on saves and external events, with a 30-second reconciliation limit. Nonrecursive watches cover only state/comments/reviewed directories, handle atomic replacement, and fall back to configured polling on watcher failure or directory replacement. |
| Git/pane caches | Concurrent misses for the same key share computation. Diff watchers cover worktrees, linked-worktree metadata and shared refs; healthy default caches reconcile every 30 seconds, while unavailable/failed watchers retain the configured TTL. Watch registration runs off async workers and is capped at 32 roots per cache. |
| Agent inspection | One tmux listing gathers active pane titles for a batch; conclusive titles avoid captures. Inconclusive inspection has bounded concurrency. Unsupported harnesses remain conservative. |

Periodic PR refresh, hourly project pulls, hibernation policy evaluation,
WebSocket heartbeats and input-reader handoff checks retain their existing
purposes. Terminal output continues to stream in order. Clone tracking wakes on
change notifications and retains its compatibility timeout. Pane scrollback is
bounded at the existing 1,000 lines, preserving scrolling behavior.

## Regression evidence

The tests check idle-frame invalidation, stale preview tokens, removed-backend
completion, refresh ordering, no-op writes, atomic file replacement, tracked
lockfile changes, shared diff cache invalidation, stats-only counts, authenticated
mutation wakeups, legacy fallback, payload failure recovery, snapshot request
counts, change-wait cleanup, and lazy large-diff rendering.

The native transport request-count fixture verifies that one poll plus its
consumer requires one workspace GET and one agent-state GET. Flutter's 5,000-row
fixture checks that fewer than 100 intrinsic-height row widgets are mounted.
The browser's 5,000-row fixture checks bounded DOM construction and reaching the
last row by scrolling. These are workload bounds, not wall-clock speed claims.

Run the full pinned checks with:

```sh
CC_FORCE_NIX=1 scripts/verify.sh --all
```

When the local default Flutter e2e port is occupied, set `CC_E2E_PORT` to an
available loopback port. Nix installations requiring explicit experimental
features can also set `NIX_CONFIG='experimental-features = nix-command flakes'`.

## Profiling after deployment

Use `scripts/dev-run.sh` for the relevant frontend. Compare identical fixtures
with 10, 100 and 500 sessions; a large single-file diff; many small files; long
ANSI captures; and multiple remote servers. Record:

- Input-to-draw latency and idle redraw/model-rebuild counts for the TUI.
- Flutter UI/raster frame times in profile mode and mounted row counts while scrolling.
- Browser long tasks, DOM row counts and network requests with visible/hidden tabs.
- Snapshot GETs and bytes, pane/Git subprocesses, and mutation-to-visible-update delay.
- Snapshot/comment-scan time and cache reuse under multiple clients.

Scorer FFI batching, terminal byte batching, layout-height changes and harness
specific lifecycle hooks require evidence from those profiles. Current code
preserves scoring, wrapping, terminal byte ordering and unsupported-agent
behavior. No production CPU, battery or frame-time improvement is asserted by
these source and regression checks.
