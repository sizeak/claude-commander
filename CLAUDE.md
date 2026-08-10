# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

- `cargo build` — debug build
- `cargo build --release` — release build (LTO enabled, symbols stripped, single codegen unit)
- `cargo test` — all tests (integration tests require tmux installed)
- `cargo test <test_name>` — single test
- `cargo clippy` — lint
- `cargo run -- --debug` — run TUI with debug logging to `/tmp/claude-commander.log`

## Verification discipline

- Never pipe command output through `| tail` / `| head` when checking pass/fail — it masks exit codes. Use `set -o pipefail` or run the command bare and report the real exit status.
- Do not claim tests, CI, or builds are green unless you have the actual exit code / CI conclusion in front of you. If verification was blocked, say so explicitly instead of inferring success.
- After any rebase or merge, re-run a full build before declaring done (stale `crate::` paths and missing deps have broken builds repeatedly).

## Workflow: TDD then review then merge

For every bug fix or behaviour change: (1) write a failing test that reproduces the issue, (2) implement the minimal fix, (3) run the full suite + lint + typecheck, (4) request a peer/Fable review and address findings, (5) open the PR with the required label, watch CI, merge only when all checks are green.

## Coding conventions

- Minimise duplication: extract shared logic into helpers or existing utility functions rather than repeating code across modules
- Use idiomatic Rust patterns: leverage the type system, enums, pattern matching, iterators, and the `?` operator; prefer `impl Into<T>` / `AsRef<T>` over concrete types in function signatures where it improves ergonomics
- Follow the existing error handling style: `thiserror` derive macros for error enums, `Result<T>` alias from `error.rs`
- When several fallible steps share the same cleanup-on-error (e.g. removing a half-created session), group them in a single `?`-scoped `async` block and handle the error once, rather than repeating the cleanup after each call. Adding a step then inherits the cleanup automatically. Note: clippy/rust-analyzer cannot catch this duplication — it is a review-time check.
- Use `tracing` macros (`info!`, `debug!`, `warn!`) for logging, not `println!` or `eprintln!` (except in CLI output paths in `main.rs`)
- Keep `main.rs` thin: it should wire CLI args to library calls and print output. Any logic worth testing belongs in `SessionManager`/library code where unit tests can reach it, not inline in `main.rs` (which is untestable without spawning the binary)
- **Core has no clap dependency, and must not gain one.** Defining a CLI is a binary's job: each binary owns its clap tree (`crates/claude-commander/src/cli_args.rs`, `crates/claude-commander-server/src/main.rs`) and an embedder of the library never inherits an argument parser it has no use for. This is also a correctness rule, not just tidiness — a clap derive takes its program name and version from the package it compiles in, so while `Cli` lived in core, `--version` printed `claude-commander-core` and the commander's generated CLI reference documented unrunnable `claude-commander-core <sub>` commands. Core takes the *rendered* markdown instead (`commander::ensure_session` takes a `cli_reference: &str`, `App::new` an owned `String`); `cli_args::cli_reference()` produces it.
- **Wire contracts live in `claude-commander-protocol`, and that includes limits — not just types.** If the server and its clients must agree on a *rule* (an accept allow-list, a size cap, a validation predicate), it belongs in protocol, not in core with clients duplicating it. Worked example: `protocol/src/paste.rs` owns `MAX_IMAGE_BYTES`, the `ImageFormat` allow-list and `validate` — one definition, enforced independently by the server's `DefaultBodyLimit` (`router.rs`), the service's up-front re-check (`api.rs`), `PasteImageStore::store`, `RemoteClient::paste_image`'s pre-upload check, the TUI's Ctrl+V size gate (`tmux/attach.rs`), and the Flutter client's file-length pre-check. Prefer a typed value over a stringly one when the contract *is* the allow-list: `validate` returns `ImageFormat`, whose `ext()`/`content_type()` make the accepted set exhaustive and unrepresentable-if-wrong, rather than an `&str` extension every caller has to re-interpret. The *effectful* half stays in core (`paste_image.rs`: RGBA→PNG encode, the pruned temp-file store, the pane injection). Protocol is deliberately serde-only, so a shared error type there gets a hand-written `Display` rather than a `thiserror` dependency. This exists because the Flutter cdylib cannot depend on core at all — see below.
- **`claude-commander-client` and `client/rust` must never gain a normal dependency on `claude-commander-core`.** That is what keeps the Flutter cdylib cross-compilable to Android: core unconditionally pulls gix, ratatui/crossterm, pty-process/nix and tmux-dependent code, and its `error.rs` `#[from]`-wraps gix/pty/reqwest errors, so *any* core dependency drags the whole set in. Core is a **dev**-dependency of `client/rust` only (host tests). When a client needs logic that currently lives in core, move the pure part to protocol and rewrite the call sites — do not add the edge, and do not leave a re-export behind.
- **No re-export shims when moving code.** Rewrite the imports at every call site instead of aliasing the old path. A shim means nothing forces callers to acknowledge the new layout, so the old path lingers and the boundary never really moves. Core's existing `pub use`s of ~25 protocol DTOs (`api.rs`, `comment/mod.rs`, `session/types.rs`) are legacy, not a pattern to copy.
- Same rule for binary identity generally: `NAME`/`VERSION` in `main.rs` are the binary's and are what user-facing output uses; `claude_commander_core::VERSION` is the library's (telemetry `lib_version`, server-build comparison) and shouldn't be presented as the app's version. One known exception predates this rule: `tmux/executor.rs` reports core's version as `TERM_PROGRAM_VERSION` under the name `claude-commander`, because a `TmuxExecutor` has no frontend identity to draw on. Harmless while the workspace shares one version; fixing it means plumbing `FrontendInfo` into the executor.
- **A comment claiming behaviour of code outside this repo must carry its receipt.** This codebase explains itself in prose, which is a strength until a comment asserts something about a *boundary* — the Flutter SDK's key dispatch, a plugin's C source, tmux's env precedence, a rasteriser. Claims about our own code are cheap to check and reviewers do check them; boundary claims get reasoned from plausibility, then read as fact forever. So cite the receipt inline (`window_manager_plugin.cc:512-519`; `KeyEventManager` dispatching to the focus tree regardless of a handler's result), or name the test that pins it, or give the repro method — otherwise phrase it as an assumption ("appears to", "untested on X"). Two corollaries. Where the claim is testable, **the test outranks the comment**: `client/lib/window/window_frame.dart` documented a `HardwareKeyboard` handler as keeping F11 out of the terminal for an entire branch; it never did, and the missing test was the real defect, not the wording. And in review, a receipt-less boundary claim is **unverified by default** — ask where the receipt is, which is how both defects of this kind were caught.

## Architecture

Terminal UI for managing Claude coding sessions via tmux and git worktrees. Each session gets an isolated git worktree, branch, and tmux session.

### Core flow

`App` (TUI main loop) routes through a `CommanderService` (`api.rs`), which owns the `SessionManager` (coordinating `TmuxExecutor` and `GitBackend`/`WorktreeManager`) plus the state/config stores. State is shared via `Arc<RwLock<AppState>>`.

**Layering (CommanderService → API → TUI).** All feature logic lives in the library and is exposed through `CommanderService`; both the CLI and the TUI call it rather than wiring `SessionManager`/stores together themselves. The TUI only renders and dispatches commands — anything worth testing (diff composition/parsing, comment re-anchoring, apply gating, etc.) belongs in the library, where unit tests can reach it without a terminal. When adding a feature, put its logic behind a `CommanderService` method and keep the `tui/` side thin.

**Known debt: `core` is not really "core".** Over half of it is the ratatui `tui/` module, and `pub mod tui;` isn't feature-gated — core's only features are `audio` and `clipboard` — so the headless `claude-commander-server` compiles the entire terminal UI. `picker.rs` is likewise a standalone ratatui popup app sitting at the top level. Extracting a `claude-commander-tui` crate is planned as separate work with its own plan; don't infer from core's current shape that a terminal frontend belongs in a shared library.

Two rules that outlive that refactor, and apply to *any* crate split here:

- **Never opportunistically rename anything serde-persisted while moving code.** `config.toml`, `state.json` and `tui.json` shapes must come through untouched by construction. (`config.toml` is never rewritten, so every form its parsers accept is permanently load-bearing — see [Migrations](#migrations).)
- **A crate leaving core brings or defines its own error type**, and never holds a `#[from]` back into core's. `protocol::paste::ImageRejection` is the precedent: a self-contained error with a hand-written `Display`, so the moved code owes core nothing.

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

TUI-only preferences (session-list view, last selection, pane width) live in a separate `tui.json` alongside `state.json` — see `tui/prefs.rs` (`TuiPrefs`). It's kept out of `state.json` so a remote backend's session data never lands in a file the local TUI persists.

### Migrations

There is deliberately **no single migration framework** — the persisted files have different constraints, so pick the mechanism by what changed, not by habit. In rough order of preference:

- **`#[serde(alias = "…")]`** for a renamed field/variant. Cheapest, and mandatory when renaming anything persisted (a missing alias wipes user data on next load). For `config.toml` the alias is permanent (the file is never rewritten); for JSON state it converges to the new spelling on the next save.
- **Shape-consumed transform** when the change has a self-destructing trigger (an obsolete key, a missing field). Idempotent for free — no marker needed. Two flavours: one-time raw-TOML rewrite for `config.toml` (`config/migrations.rs`, `toml_edit`), and repair-on-every-read on the deserialized model for `state.json` (`AppState::backfill_base_branch`).
- **Schema counter** (`schema_version` + a `migrate_*` fn, e.g. `tui/prefs.rs`) for a one-time mutation that has *no* self-destructing signal — typically resetting a value that can legitimately reappear (e.g. "make X the new default, once, for everyone"). This is the only case that needs a persisted marker.

Two hard rules:

- **Never gate a migration on `AppState.version`** — it's stamped on write but never read, so it can't tell you whether anything ran.
- **Never version-gate `state.json`.** It's multi-writer (two instances under `flock`, *and an older binary may still write it*), so a one-shot gate is unsound: after it flips, an old binary can write old-shape records back and the gate never re-fires. Use repair-on-every-read there. A schema counter is only sound for single-owner machine files like `tui.json`.

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

**Screenshots are generated, never hand-edited.** Everything in `docs/images/` comes out of `docs/tool/` — one hermetic fixture (`fixture.sh`: temp XDG tree, throwaway tmux server, three demo repos, ten sessions, a stand-in agent) rendered by `capture-tui.sh` (TUI → SVG via Rich) and `capture-client.sh` (the real Flutter app → PNGs). When a UI change dates an image, re-run the capture rather than editing the file; when the demo *content* is wrong, change `cc_seed_fixture`. Never point a capture at real sessions — the fixture exists so a screenshot can't leak a project, branch or prompt.

## Pre-commit hooks

This project uses [pre-commit](https://pre-commit.com/) to run `cargo fmt` and `cargo clippy` on every commit, and `dart format` (via `client/tool/dart-format.sh`) when the commit touches Dart. After cloning, run:

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
- The end-to-end sequence for a change — failing test, minimal fix, full suite, review, PR, green CI, merge — is [Workflow: TDD then review then merge](#workflow-tdd-then-review-then-merge). Follow it in order; the bullets above are its repo-specific details.

