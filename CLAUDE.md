# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

- `cargo build` — debug build
- `cargo build --release` — release build (LTO enabled, symbols stripped, single codegen unit)
- `cargo test` — all tests (integration tests require tmux installed)
- `cargo test <test_name>` — single test
- `cargo clippy` — lint
- `cargo run -- --debug` — run TUI with debug logging to `/tmp/claude-commander.log`

## Coding conventions

- Minimise duplication: extract shared logic into helpers or existing utility functions rather than repeating code across modules
- Use idiomatic Rust patterns: leverage the type system, enums, pattern matching, iterators, and the `?` operator; prefer `impl Into<T>` / `AsRef<T>` over concrete types in function signatures where it improves ergonomics
- Follow the existing error handling style: `thiserror` derive macros for error enums, `Result<T>` alias from `error.rs`
- When several fallible steps share the same cleanup-on-error (e.g. removing a half-created session), group them in a single `?`-scoped `async` block and handle the error once, rather than repeating the cleanup after each call. Adding a step then inherits the cleanup automatically. Note: clippy/rust-analyzer cannot catch this duplication — it is a review-time check.
- Use `tracing` macros (`info!`, `debug!`, `warn!`) for logging, not `println!` or `eprintln!` (except in CLI output paths in `main.rs`)
- Keep `main.rs` thin: it should wire CLI args to library calls and print output. Any logic worth testing belongs in `SessionManager`/library code where unit tests can reach it, not inline in `main.rs` (which is untestable without spawning the binary)

## Architecture

Terminal UI for managing Claude coding sessions via tmux and git worktrees. Each session gets an isolated git worktree, branch, and tmux session.

### Core flow

`App` (TUI main loop) routes through a `CommanderService` (`api.rs`), which owns the `SessionManager` (coordinating `TmuxExecutor` and `GitBackend`/`WorktreeManager`) plus the state/config stores. State is shared via `Arc<RwLock<AppState>>`.

**Layering (CommanderService → API → TUI).** All feature logic lives in the library and is exposed through `CommanderService`; both the CLI and the TUI call it rather than wiring `SessionManager`/stores together themselves. The TUI only renders and dispatches commands — anything worth testing (diff composition/parsing, comment re-anchoring, apply gating, etc.) belongs in the library, where unit tests can reach it without a terminal. When adding a feature, put its logic behind a `CommanderService` method and keep the `tui/` side thin.

### Modules

- **`session/`** — `types.rs` defines `Project` → `WorktreeSession` hierarchy (UUIDs, display as 8-char prefix). `manager.rs` orchestrates lifecycle: create/restart/delete sessions, content/diff retrieval.
- **`api.rs`** — `CommanderService`: the single coordination layer the CLI and TUI both depend on. Query/mutation methods plus `Serialize` response structs (`SessionInfo`, `SessionDetail`, `ReviewSnapshot`, …).
- **`comment/`** — local diff-review comments: `Comment` model + persisted `CommentStore`, snippet-based `reanchor` (drift detection), markdown composition, vim visual-mode selection math, and apply-delivery decision (`decide_send`/`ApplyOutcome`).
- **`tmux/`** — `executor.rs`: async tmux commands with semaphore throttling (default 16 concurrent). `capture.rs`: cached pane content (50ms TTL, xxh3 hash-based staleness). `state.rs`: pattern-based agent state detection. `attach.rs`: PTY-based session attachment. `input.rs`: non-blocking input forwarding.
- **`git/`** — `backend.rs`: pure Rust git via gitoxide (gix crate). `worktree.rs`: uses git CLI for worktree mutations. `diff.rs`: cached diffstat computation (500ms TTL). `review_diff.rs`: structured `file→hunk→line` unified-diff parser and `compose_review_diff` (base→working-tree) for the review view.
- **`tui/`** — `app/` (split into `mod.rs`, `render.rs`, `input.rs`, `modals.rs`, `review.rs`, `settings.rs`, `state.rs`, `actions.rs`, …): main event loop, rendering, modal system, pane management. `event.rs`: `AppEvent`/`UserCommand` enums, key mappings, `EventLoop` multiplexing crossterm + ticks + state updates. `theme.rs`: auto-detects terminal color capability via COLORTERM/TERM. `widgets/`: TreeList, Preview, InfoView. The full-screen review-diff view is rendered in `app/review.rs` (not a widget).
- **`config/`** — `settings.rs`: TOML config via figment, layered defaults → file. `storage.rs`: JSON state persistence. `keybindings.rs`: `BindableAction` ↔ key map (palette-only actions may be unbound).
- **`telemetry/`** — privacy-respecting usage telemetry. Fixed, typed event schema (`feature(&'static str)` + `session_start`) — **never** free-form text, so typed/session content can't leak by construction. Batched, fire-and-forget HTTP ingest to OpenObserve via a background task; no-op when disabled (config off, `DO_NOT_TRACK`, or no baked credential). Frontends must identify themselves via `FrontendInfo` (panics if empty) — it's a required arg to `CommanderService::new`/`for_cli`. Domain features are recorded inside `CommanderService` methods (covers all frontends); UI-only features are recorded once at the TUI `handle_command` chokepoint via `UserCommand::telemetry_feature`.
- **`error.rs`** — thiserror-based hierarchy: SessionError, TmuxError, GitError, ConfigError, TuiError.

### Key patterns

- Event-driven TUI: `EventLoop` combines terminal input, render ticks, and mpsc state update channels into a single `AppEvent` stream
- Caching with TTLs: ContentCapture (50ms) and DiffCache (500ms), both with hash-based change detection
- Modals: Input/Confirm/Help/Error/ReviewDiff overlay the main UI, handled in `tui/app/modals.rs` + `input.rs`
- Background updater task periodically refreshes agent states for all active sessions

## Config and state files

Paths are determined by the `directories` crate (`ProjectDirs::from("com", "claude-commander", "claude-commander")`) and differ by platform:

| File | macOS | Linux |
|------|-------|-------|
| Config | `~/Library/Application Support/com.claude-commander.claude-commander/config.toml` | `~/.config/claude-commander/config.toml` |
| State | `~/Library/Application Support/com.claude-commander.claude-commander/state.json` | `~/.local/share/claude-commander/state.json` |
| Worktrees | `~/Library/Application Support/com.claude-commander.claude-commander/worktrees/` | `~/.local/share/claude-commander/worktrees/` |

See `Config` struct in `config/settings.rs` for all config fields. The worktrees directory can be overridden via the `worktrees_dir` config option.

## Testing

Unit tests are co-located in source files (`#[cfg(test)]`). Integration tests in `tests/integration_test.rs` require tmux. All async tests use `#[tokio::test]`.

### Test isolation

Tests must not read or modify anything on the real filesystem. Any disk access must go through `tempfile::TempDir` (already in dev-deps) for OS-portable temp paths. Never hardcode `/tmp/...` as a real path. Dummy `PathBuf` values stored in struct fields (never accessed on disk) are acceptable.

**tmux isolation:** tmux clients resolve their socket from the `$TMUX` env var (set inside any tmux session) **in preference to** `$TMUX_TMPDIR` — so a test script that only exports `TMUX_TMPDIR` is NOT isolated when run from inside tmux: its tmux commands (including a cleanup `tmux kill-server`) hit the developer's real server and can kill every open session. Any script that isolates tmux via `TMUX_TMPDIR` must also `unset TMUX TMUX_PANE` (see `client/tool/e2e.sh`), and never run a bare `tmux kill-server` without `$TMUX` provably unset. The Rust integration tests are now genuinely isolated via the `tmux_tmpdir` config knob (set by `crates/claude-commander-test-support`'s `test_state` and core's `create_isolated_config_store`): the `TmuxExecutor` and the `HeadlessAttach` bridge apply `TMUX_TMPDIR` + strip `$TMUX`/`$TMUX_PANE` per-command when it is set, so each test gets its own throwaway tmux server (which exits with its last session) rather than landing on the developer's default server.

### Writing new tests

Use red-green TDD: write a failing test first, then implement the fix. Key areas covered by regression tests:

- **State management** (`config/storage.rs`) — bidirectional session-project linking, cascade delete, active session filtering
- **Status state machine** (`session/types.rs`) — transition guards, timestamp updates, display strings
- **Key mappings** (`tui/event.rs`) — every documented keybinding has a test; release/repeat events ignored
- **Config resolution** (`config/settings.rs`) — editor precedence chain, GUI editor auto-detection
- **Widget state** (`tui/widgets/`) — TreeListState navigation/wrap/clamp, PreviewState follow mode/scroll
- **Review view** (`tui/app/review.rs`) — `DiffReviewState` file/cursor/scroll navigation, visual-mode selection math, side-by-side row pairing, mouse row mapping
- **Caching** (`tmux/capture.rs`, `git/diff.rs`) — hash determinism, TTL staleness, parse_diff_stat edge cases
- **Name sanitization** (`session/manager.rs`) — branch name generation, special char handling
- **Error types** (`error.rs`) — all variant displays, type conversions

When adding new behavior, add a corresponding unit test that would fail without the change.

## Documentation

When adding or changing config options, hotkeys, or keybindings:

- **README.md** — Update the Keyboard Shortcuts table (kept in the README) to reflect the change
- **docs/configuration.md** — Update the Configuration TOML block (moved here from the README) to reflect the change
- **Help modal** — Update the help text rendered in `app.rs` (`render_help_modal`) so the in-app `?` help stays in sync with the README
- **Settings modal** — Add new config options to `build_settings_rows()` in `app.rs` (General tab) and the corresponding `apply_settings_edit()` match arm so they are editable from the in-app settings UI
- **CLAUDE.md** — No update needed for individual options; the Architecture section points to `Config` struct as the source of truth

## Pre-commit hooks

This project uses [pre-commit](https://pre-commit.com/) to run `cargo fmt` and `cargo clippy` on every commit. After cloning, run:

```
pre-commit install
```

The `cargo fmt` hook auto-fixes formatting. If `cargo clippy` fails, fix the warnings before committing.

## Git conventions

**CRITICAL: Never force push under any circumstances. This includes `--force`, `--force-with-lease`, and amending commits that have been pushed. Always create new commits instead.**

- Branch names should be lowercase letters with hyphens separating words, no slashes e.g. `refactor-user-service`
- Pull request labels include `dev-review-required`, `ready-for-test`, `trivial`, `tidy`, `merge-on-ci-green`. Do not add labels unless instructed.
- Never skip GPG commit signing
- Precommit hooks may autoformat files while failing the commit; these changes will need to be restaged and the commit reattempted.
- Before committing, always ensure `cargo clippy` and `cargo build` pass with no warnings or errors. Fix any issues before creating the commit.
- Bug fixes need a regression test too, not just features: follow the red-green TDD rule under [Testing](#testing) — add a test that fails without the fix and passes with it. If the fix lives somewhere untestable (e.g. `main.rs`), push the logic down into testable library code rather than skipping the test.
- Cutting a release: `cargo release {patch,minor,major} --execute` (see CONTRIBUTING.md). Never bump `Cargo.toml` manually.

