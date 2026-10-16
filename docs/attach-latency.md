# Session attach and Ctrl-Q latency

Measurement reference: v0.36.0 `4640009`, instrumentation `c7b6dfb`
(`t3code/instrument-v036-attach`), recording dated 2026-09-30. The supplied log
has 17 attach requests and 16 completed returns, including a 19m39s attachment.
The implementation starts from `8faa85b`: its batched/concurrent detector from
`0e55db1` is already different from v0.36.0, but attach return still awaited
fresh detection. Local main `4830680` also retained that await when inspected.

## Exit measurements

ANSI colour sequences were stripped before parsing `stage` fields. Board timing
uses `input_reader_restarted` as its origin. Entry timing pairs each request with
its first connection-open and first-output records: four later switch/toggle
opens are excluded (there are 21 open/output records but only 17 requests).

| Step | Samples | Minimum / median / maximum |
| --- | ---: | --- |
| Ctrl-Q to tmux client exit | 16 | 0 / 1 / 1 ms |
| Input restart to first board frame | 16 | 538 / 714.5 / 1,501 ms |
| Synchronous post-attach fresh scan | 16 | 530 / 706 / 1,493 ms |
| Agent sweeps making two tmux queries | 357 | 266 / 475 / 1,655 ms |
| Agent sweeps making no tmux queries | 60 | 0 / 0 / 0 ms |

The median scan accounts for about 98.8% of the median first-frame interval;
the median of individual scan/frame ratios is approximately 98.6%. The output
pumps and raw-mode restoration also finish within 0–1 ms of Ctrl-Q. Detach is
not the observed half-second-to-one-second pause.

In the long attachment, Ctrl-Q arrives at 11:00:35.874118 UTC, tmux exits at
11:00:35.875872, input restarts at 11:00:35.876103, fresh detection completes at
11:00:36.430965, and the board frame appears at 11:00:36.432389. The logged
refresh span is 547 ms; it starts after mark-read. The frame interval is 556 ms.

The tick producer continues during attach. That run logs 625 queued ticks during
attach, 519 drained on input restart, and 619 processed in the first board batch.
The batch ends 0.119 ms after the first frame. Producer refills during draining
explain counts above the channel's 256-event capacity. The burst is distinct
from the measured first-frame blocker. Polling and tick policy are unchanged.

The recorded sweep cadence is approximately 10 seconds. Source defaults to
3 seconds, but the reference host's runtime config was not supplied. Neither
local Linux config location contained a runtime config. No cadence change is
justified by the default alone.

## Entry profile

These phase boundaries use existing `ensure_attachable`, existence, dead-pane,
spawn, and first-output log messages; they do not require a new session or a
change to attach-entry behavior. All 17 initial attaches have these boundaries.

| Step | Median |
| --- | ---: |
| Attach request to `ensure_attachable` | 101.98 ms |
| Session lookup/status validation before existence query | 0.06 ms |
| Existence query (`has-session`) | 242.23 ms |
| Dead-pane query (`list-panes`) | 257.95 ms |
| After dead-pane check through state stamp and PTY spawn | 7.24 ms |
| Request to opened connection | approximately 529 ms |
| Opened connection to first output | 259.77 ms |
| Request to first output | 740.19 ms (576.95–2,007.51 ms) |

Medians of component durations do not add up to the median total. The first
existence query includes a separately logged tmux version probe in that sample.
The subprocess timings include semaphore waiting, process startup and tmux
response: the log cannot separate those costs. It also cannot separate state
persistence from PTY spawn inside the final ~7 ms phase. Entry optimization is
left for a separate measured change.

## First-frame change and coverage

Attach return still marks matched viewed sessions read through the final attached
backend, preserving the existing routing.
The fresh `agent_states(true)` call runs in a backend-owned task and keeps the
service's shared unread-transition cache update. Its event merges only the
viewed sessions into that backend's cached states; local attaches also update
the local UI map. Unviewed states and other backends stay intact. The existing
switcher focus and pending-review paths are unchanged. Removal of a backend
aborts its task. Older cache reads cannot overwrite a completed fresh result.

`attach_return_renders_before_slow_fresh_agent_detection` uses a gated mock,
never terminal input or a real tmux server. Before the fix its 100 ms return
budget failed (exit 101). After the fix an isolated TestBackend frame took
2.51 ms while the scan completed at 703.89 ms with a deliberate 700 ms delay.
These are test-harness timings, not a new measurement on the reference host.

Tests cover mark-read delivery, agent/shell switcher visits, unviewed-session
preservation, backend routing, local cache synchronization, stale events,
removed-backend events, failed scans and the shared unread baseline. The mark-read
calls remain awaited; a slow remote mark-read can still delay return. Opening a
requested review after detach can also do work before the board is shown.

The existing switcher records visited names without their backend ids. Visits
across backend boundaries are therefore matched only against the final attached
backend; this pre-existing limitation is preserved. Tests cover visits and shell
toggles within one backend, and isolation between backends.

Peer review caught a partial-update ordering issue: applying fresh states must
not discard an older-request workspace snapshot that carries mark-read. Separate
snapshot and agent-state revisions now let that snapshot fold while rejecting
its stale state payload; the regression covers unread=false arriving after the
fresh-state event.

## Verification

All commands used the pinned Nix toolchain (`CC_FORCE_NIX=1`, with Nix's
`nix-command flakes` experimental features enabled in this environment).

- `scripts/verify.sh -p tui attach_return_renders_before_slow_fresh_agent_detection`:
  expected exit **101** before the first-frame fix (100 ms timeout).
- `scripts/verify.sh -p tui attach_refresh_local_merge_and_stale_events`:
  expected exit **101** with the review defect reintroduced (unread remained true).
- `scripts/verify.sh -p tui attach_ -- --nocapture`: exit **0**, 9 tests passed
  after both fixes (final timing: frame 2.51 ms, scan completion 703.89 ms).
- Final `scripts/verify.sh`: exit **0**; fmt, clippy, full workspace build and
  workspace tests passed after the revision-ordering fix. Logs are in
  `target/verify-logs/`. The existing ignored TUI performance test remains ignored.

Peer review completed and its ordering finding was addressed. No merge or rebase
was performed. No real user sessions or runtime configuration were changed.
