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
- Use `tracing` macros (`info!`, `debug!`, `warn!`) for logging, not `println!` or `eprintln!` (except in CLI output paths in `main.rs`)

## Architecture

Terminal UI for managing Claude coding sessions via tmux and git worktrees. Each session gets an isolated git worktree, branch, and tmux session.

### Core flow

`App` (TUI main loop) owns a `SessionManager` which coordinates `TmuxExecutor` and `GitBackend`/`WorktreeManager`. State is shared via `Arc<RwLock<AppState>>` between the TUI and SessionManager.

### Modules

- **`session/`** — `types.rs` defines `Project` → `WorktreeSession` hierarchy (UUIDs, display as 8-char prefix). `manager.rs` orchestrates lifecycle: create/pause/resume/delete sessions, content/diff retrieval.
- **`tmux/`** — `executor.rs`: async tmux commands with semaphore throttling (default 16 concurrent). `capture.rs`: cached pane content (50ms TTL, xxh3 hash-based staleness). `state.rs`: pattern-based agent state detection. `attach.rs`: PTY-based session attachment. `input.rs`: non-blocking input forwarding.
- **`git/`** — `backend.rs`: pure Rust git via gitoxide (gix crate). `worktree.rs`: uses git CLI for worktree mutations. `diff.rs`: cached diff computation (500ms TTL).
- **`tui/`** — `app.rs` (~1200 lines, largest file): main event loop, rendering, modal system, pane management. `event.rs`: `AppEvent`/`UserCommand` enums, key mappings, `EventLoop` multiplexing crossterm + ticks + state updates. `theme.rs`: auto-detects terminal color capability via COLORTERM/TERM. `widgets/`: TreeList, Preview, DiffView.
- **`config/`** — `settings.rs`: TOML config via figment, layered defaults → file → env vars (prefix `CC_`). `storage.rs`: JSON state persistence.
- **`error.rs`** — thiserror-based hierarchy: SessionError, TmuxError, GitError, ConfigError, TuiError.

### Key patterns

- Event-driven TUI: `EventLoop` combines terminal input, render ticks, and mpsc state update channels into a single `AppEvent` stream
- Caching with TTLs: ContentCapture (50ms) and DiffCache (500ms), both with hash-based change detection
- Modals: Input/Confirm/Help/Error overlay the main UI, handled in `app.rs`
- Background updater task periodically refreshes agent states for all active sessions

## Config and state files

- Config: `~/.config/claude-commander/config.toml` (see `Config` struct in `config/settings.rs` for all fields)
- State: `~/.local/share/claude-commander/state.json`
- Worktrees: `~/.local/share/claude-commander/worktrees/`

## Testing

Integration tests in `tests/integration_test.rs` require tmux, use `tempfile` for isolated temp directories, and create real git repos. Unit tests are co-located in source files (`#[cfg(test)]`). All async tests use `#[tokio::test]`.

## Git conventions

**CRITICAL: Never force push under any circumstances. This includes `--force`, `--force-with-lease`, and amending commits that have been pushed. Always create new commits instead.**

- Branch names should be lowercase letters with hyphens separating words, no slashes e.g. `refactor-user-service`
- Pull request labels include `dev-review-required`, `ready-for-test`, `trivial`, `tidy`, `merge-on-ci-green`. Do not add labels unless instructed.
- Never skip GPG commit signing
- Precommit hooks may autoformat files while failing the commit; these changes will need to be restaged and the commit reattempted.
- Before committing, always ensure `cargo clippy` and `cargo build` pass with no warnings or errors. Fix any issues before creating the commit.

