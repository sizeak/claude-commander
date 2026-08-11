# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

**Use `scripts/verify.sh` and `scripts/dev-run.sh` rather than improvising the underlying commands.** They exist because the raw invocations were being retyped — and subtly varied — hundreds of times: an audit of this repo's session history found 1,033 `cargo` calls, 407 `flutter` calls (335 of them hand-wrapped in `nix develop .#client…`) and 205 `adb` calls, with the same six-step Android chain reassembled from memory each time. The scripts encode the correct form once. Reach for a raw command only when doing something the scripts do not cover, and if it's something you'll want twice, add a lane or target instead.

### Verifying

`scripts/verify.sh`'s lanes mirror `.github/workflows/ci.yml`, so a green `--all` means a green PR. `--all` is a *superset* of CI, though: it also runs `shellcheck`, the script `selftest`, and the `e2e` that CI cannot. A red lane there does not imply red CI.

- `scripts/verify.sh` — the Rust lanes: fmt, clippy, build, test
- `scripts/verify.sh --fast` — fmt + clippy only (seconds; the pre-commit hook's shape)
- `scripts/verify.sh --client` — Rust lanes plus pub get / dart format / flutter analyze / flutter test / cdylib tests
- `scripts/verify.sh --all` — every CI job, plus shellcheck, the script self-tests, and the Flutter e2e
- `scripts/verify.sh --nix` — Rust lanes plus `nix build` and the Homebrew-path packaging install
- `scripts/verify.sh -p core <filter>` — one crate: `cargo test -p claude-commander-core <filter>`. Aliases: `core`, `cli`/`commander`, `server`, `protocol`, `remote`, `client`, `test-support`
- `scripts/verify.sh --list` — the lane / exit-code table

Every selected lane runs even after an earlier one fails; output goes to `target/verify-logs/<lane>.log` and only a 40-line tail is printed on failure. **The process exits with the first failing lane's reserved code**, so you can tell what broke without reading logs: 10 fmt, 11 clippy, 12 build, 13 test, 20 pub-get, 21 dart-format, 22 analyze, 23 flutter-test, 24 cdylib, 25 e2e, 30 nix-build, 31 packaging, 32 shellcheck, 33 selftest. Plus 2 for bad arguments and 3 for a missing toolchain. The `e2e` lane reports `SKIP` with a reason when there's no display and no `xvfb-run` — CI has never been able to run it, so a local run is the only coverage the full stack gets (`--e2e` forces it).

The `pub-get` lane is a precondition, not a check, and it is load-bearing: `dart format` reads each file's language version from the nearest `.dart_tool/package_config.json`, and `client/rust_builder/cargokit/build_tool` is a *vendored* Dart package with its own `pubspec.yaml` and no `.dart_tool`. Against a stale package config the formatter falls back to a different language version and rewrites 15 vendored files — `dart-format` reports `120 files (15 changed)` locally where CI reports `0 changed`. So the client lanes resolve packages first, exactly as CI's Fetch Flutter Packages step does. If you ever see `dart-format` fail on files you didn't touch, that's this.

### Running

- `scripts/dev-run.sh tui [--debug]` — the TUI (`--debug` logs to `/tmp/claude-commander.log`)
- `scripts/dev-run.sh server [--port N] [--token T] [--isolated]` — the headless server. `--isolated` runs it over a throwaway XDG tree with `TMUX_TMPDIR` set and `$TMUX` unset, so a dev run cannot touch real state or the developer's tmux server
- `scripts/dev-run.sh linux [--log FILE]` — the Flutter Linux desktop app
- `scripts/dev-run.sh android [--device SERIAL] [--release] [--no-launch] [--window]` — boots the AVD if no device is attached, builds the APK, installs with `-r -g`, launches, then greps logcat for startup crashes (a monkey launch reports success even when the app dies). Failures are located by exit code: 40 emulator boot, 41 APK build, 42 install, 43 launch
- `scripts/dev-run.sh emulator start|stop|status` — headless AVD lifecycle on its own

Both scripts resolve their own toolchain: they use the tool already on PATH, else re-enter the nix dev shell that provides it — the rule `client/tool/dart-format.sh` already followed. So they work from a bare terminal and cost nothing extra inside `nix develop .#client`. **The caveat is toolchain drift:** CI runs its Rust lanes under `nix develop -c`, so an ambient `cargo`/`clippy` of a different version can disagree with CI in either direction. Set `CC_FORCE_NIX=1` to route every lane through `nix develop` the way CI does. `CC_CLIENT_SHELL`, `CC_ANDROID_SHELL` and `CC_AVD` override the shell refs and AVD name; `CC_E2E_TIMEOUT` bounds the e2e lane (default 1800s).

`verify.sh` exports `DO_NOT_TRACK=1` for its whole run — the same backstop `ci.yml` sets workflow-wide, because `cfg!(test)` only suppresses core's *own* unit tests and downstream crates' `cargo test`/`cargo run` would otherwise report to the live endpoint. A verification sweep is not usage. `dev-run.sh` deliberately does *not* set it for its non-isolated targets, which are genuine usage; `--isolated` does.

Shared logic lives in `scripts/lib/dev-common.sh`; its pure helpers are covered by `scripts/tests/run.sh` (plain bash assertions, no framework), which runs as the `selftest` lane. When you add a lane, add it to `cc_lanes_for_tier`, `cc_exit_code_for_lane` and `cc_lane_description` — the self-tests assert every lane has a distinct code that doesn't collide with the reserved statuses.

## Verification discipline

- Never pipe command output through `| tail` / `| head` when checking pass/fail — it masks exit codes. Use `set -o pipefail` or run the command bare and report the real exit status.
- Do not claim tests, CI, or builds are green unless you have the actual exit code / CI conclusion in front of you. If verification was blocked, say so explicitly instead of inferring success.
- After any rebase or merge, re-run a full build before declaring done (stale `crate::` paths and missing deps have broken builds repeatedly).
- Verify against the pinned toolchain, not your PATH. `scripts/verify.sh` uses whatever `cargo`/`flutter` is already on your `PATH` and only falls back to the dev shell when one is missing, so a local toolchain that differs from the flake's can disagree with CI in either direction. `CC_FORCE_NIX=1` routes every lane through `nix develop` the way CI does. This is **mandatory for golden changes** — see [Golden tests](#golden-tests).

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
- **Redact wire-bound text at the value's construction site, not at each place it is later read.** Redacting only where an error is displayed or logged fixes the call sites you found and none of the others you didn't — `CloneRejection::MalformedSlug` still leaked a credential through the one re-validation arm that a call-site-only pass had missed, even after every other known path was covered (`protocol/src/github.rs:541-549`, `redact_credentials`). Redacting when the value is captured — inside the rejection's constructor — means `Display`, `Debug`, and any future `Serialize` impl inherit safety for free, and no later caller has to remember.
- **A route whose request body can carry a credential must use `SafeJson`, not axum's `Json`.** Axum's own `Json` extractor answers a wrong-shaped body with serde's rejection message, which quotes the offending *value* — so posting a malformed body to `POST /api/projects/clone` echoed a pasted token in a 422 response before the handler ran, bypassing every redaction the codebase builds in (`crates/claude-commander-server/src/extract.rs`). `SafeJson` matches on the `JsonRejection` variant and reports only the category, never `body_text()`/`Display`. It's a checked decision per route, not a blanket replacement — serde's detail is worth keeping on a body that cannot hold a secret.
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

### Golden tests

The Flutter client's reference images live in `client/test/goldens/images/` and run as part of `flutter test`. They guard the chrome layer — see the Golden tests section of `client/README.md` for what the harness (`client/test/support/golden.dart`) has to do and why.

**Any change that adds, regenerates or deletes a golden must be verified with `CC_FORCE_NIX=1 scripts/verify.sh --goldens` before pushing** (`--update` regenerates them). A bare `flutter test` is not sufficient evidence: it can pick up a Flutter/Skia other than the pinned one, and the reference images are rasteriser-sensitive by nature. The lane warns when it is using an unpinned `flutter` rather than passing quietly. Then read the image diff before committing — an unexplained pixel change is the entire point of these tests.

What that does and does not buy you, because a golden has already passed locally and failed on the runner:

- **Reproduced by the pinned lane.** The Flutter SDK and its Skia (from the `clientCi` shell in `flake.nix`), the three bundled faces plus `MaterialIcons` resolved from the pinned `FLUTTER_ROOT`, and `devicePixelRatio` 1 (fixed by `useGoldenSurface`). No golden reads a system font, so there is nothing font-related to configure and the host's fontconfig is not consulted.
- **Not reproduced, and not reproducible by any local configuration.** The runner's text antialiasing. The LCARS phone-shell golden failed only on CI for exactly this: 1179 px confined to one 13-row band, geometry byte-identical, i.e. label antialiasing alone (measured from the comparator's own images, run `31333637408`). It was deleted in #280 rather than given a tolerance — a tolerance sized to hide 0.39% also hides a genuinely wrapped label — and its layout is now asserted geometrically in `client/test/phone_shell_test.dart`. Prefer that resolution: when a golden pins the rasteriser rather than the layout, replace it with a geometric assertion instead of loosening the threshold.

So a green `--goldens` lane is strong evidence, not proof. If a golden fails on CI and passes locally, do not re-run blindly — download the `golden-failures` artifact the Client Tests job uploads on failure (`isolated`/`masterImage`/`testImage` PNGs) and measure what moved before deciding whether it is layout or antialiasing.

`TERM` and `COLORTERM` are irrelevant to the goldens themselves — a golden is rasterised by Skia into a PNG with no terminal in the picture. They are not equivalent elsewhere, though, and `scripts/verify.sh` treats them differently:

- `COLORTERM` is read only by `ColorMode::detect()` (`tui/theme.rs`). Tests *do* reach that function — `Theme::default()` is `for_color_mode(detect())` and four board-widget tests construct one (`tui/widgets/board/render.rs`) — but no test **assertion** depends on the result: those four assert cell symbols and returned button/heading/hit regions, never styles, so the detected palette is built and discarded. It is left unset, because pinning it would imply an assertion depends on it.
- `TERM` also reaches `HeadlessAttach::spawn` (`tmux/headless_attach.rs`), which the server's `/ws/attach` integration tests exercise. `fallback_term` normalises an unset/empty/`dumb`/`unknown` `TERM` to `xterm-256color` but passes a real one straight through to `tmux attach`, so a developer in kitty or tmux hands those tests a different `TERM` than a headless host does. `verify.sh` exports `TERM=xterm-256color` for its whole run — the value the fallback would pick anyway — so that path behaves the same in both places.

`verify.sh` exports both `DO_NOT_TRACK` and `TERM` for the whole run rather than per lane, so a lane added later inherits them. That works because a bash `export` overrides whatever the caller had; do not assume a task runner's declarative `env:` block does the same — go-task's, for one, only fills a variable that is *absent* from the caller's environment, and `TERM` is always set in an interactive shell. If you change how either is pinned, verify it by echoing the variable from inside a lane under a deliberately wrong caller value, because a run that merely passes proves nothing here.

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
- Before committing, always ensure `scripts/verify.sh` passes (fmt, clippy, build, test — no warnings or errors). Fix any issues before creating the commit. Add `--client` when the commit touches `client/`.
- Bug fixes need a regression test too, not just features: follow the red-green TDD rule under [Testing](#testing) — add a test that fails without the fix and passes with it. If the fix lives somewhere untestable (e.g. `main.rs`), push the logic down into testable library code rather than skipping the test.
- Cutting a release: `cargo release {patch,minor,major} --execute` (see CONTRIBUTING.md). Never bump `Cargo.toml` manually.
- The end-to-end sequence for a change — failing test, minimal fix, full suite, review, PR, green CI, merge — is [Workflow: TDD then review then merge](#workflow-tdd-then-review-then-merge). Follow it in order; the bullets above are its repo-specific details.

