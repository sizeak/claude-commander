# Claude Commander client

Cross-platform Flutter client for [claude-commander-server](../crates/claude-commander-server) —
the GUI counterpart to the terminal UI, for whichever device you're not sitting at
a `claude-commander` shell on. Verified targets today are **Linux desktop** and
**Android**. iOS/macOS are deferred (no Xcode toolchain here), but the app is kept
macOS-safe as it grows: `reqwest` uses `rustls` (no OpenSSL to cross-build), and
Linux-only desktop dependencies are gated behind `isLinux` in the dev shell.

The app talks to the server's HTTP REST API (`/api/…`) and its WebSocket terminal
attach endpoint (`/ws/attach`) — the same server the TUI drives locally or attaches
to remotely.

## Architecture

```
Flutter UI  ──frb──►  Rust cdylib  ──claude-commander-client──►  claude-commander-server
             (handle)  (registry)      (RemoteClient + Poller)
```

**Flutter UI** (`client/lib/`) — Material 3, dark theme. An [`AdaptiveShell`](lib/pages/adaptive_shell.dart)
renders a stacked phone flow (list → detail → terminal/review, via `Navigator.push`)
below `kWideBreakpoint` (900 logical px), and a desktop master-detail layout above
it: a server sidebar + grouped session list on the left, a persistent detail pane
on the right whose Detail/Terminal/Review tabs switch in place. The same page
*bodies* (`SessionListBody`, `SessionDetailBody`, `TerminalBody`, `ReviewBody`)
back both layouts; only the surrounding shell differs.

A [`CommanderStore`](lib/state/commander_store.dart) (`ChangeNotifier`) is the
single reactive source of truth for a connected server. It owns the opaque
per-server **handle** for its whole lifetime, refetches the workspace snapshot and
agent states whenever the server's change-feed generation counter bumps (no
wall-clock polling on the Dart side), and tracks a `ConnectionStateDto` off a
separate connection-health feed. Widgets rebuild via `ListenableBuilder`.

**Rust cdylib** (`client/rust/`) — `rust_lib_claude_commander_client`, compiled as
`cdylib` (Android/Linux `.so`) and `staticlib` (iOS, future). Its
[`api/registry.rs`](rust/src/api/registry.rs) is the opaque-handle seam: `connectServer`
builds a `claude-commander-client::RemoteClient` + background `Poller` and registers
both under a fresh UUID handle; every subsequent route/terminal/feed call is keyed
by that handle. One registry entry per connected server today — the seam a
multi-server client (design already written; see below) grows into without
changing the call shape.

- `api/simple.rs` — most HTTP routes: sessions, projects, create-options,
  programs, cascade/push-stack operations.
- `api/terminal.rs` — WebSocket attach bridge (a shared `tokio` multi-thread
  runtime drives the socket off the Dart isolate; events stream to Dart via
  `flutter_rust_bridge` `StreamSink`) plus the change-feed and connection-feed
  streams the store listens to.
- `api/review.rs` — review/diff and comments HTTP endpoints.
- `api/mirrors.rs` — `#[frb(mirror(…))]` declarations so frb generates typed Dart
  classes from the protocol types (compile-checked: a field mismatch is a Rust
  compile error, not a runtime surprise).

**`claude-commander-client` crate** (`crates/claude-commander-client`) — the shared
transport: `RemoteClient` speaks the wire DTOs against the server's `/api` surface
and `/ws/attach`, classifying failures into transport-neutral `ClientError`
categories, with a background `Poller` driving the change-feed generation counter
and connection-state machine (exponential backoff). This crate depends only on
`claude-commander-protocol` plus network crates — never on `claude-commander-core`
(tmux/gix don't cross-compile to Android) — so it backs *both* this cdylib and the
desktop TUI's remote-session support (via the thin `claude-commander-remote`
adapter). One transport, two frontends.

**`claude-commander-protocol` crate** (`crates/claude-commander-protocol`) — the
single source of truth for wire types (`SessionInfo`, `SessionDetail`,
`ReviewSnapshot`, `ClientControl`/`ServerControl` WS frames, etc.). Server, client
transport, and cdylib all depend on it; nobody maintains a private DTO mirror.

**DTO convention** — `flutter_rust_bridge` renders data-carrying Rust enums as Dart
`freezed` classes, which requires the `build_runner` toolchain. The client
deliberately avoids that dependency. Where the protocol types use data enums or
tuples (`ApplyOutcome`, `BinaryKind`, `line_range`), the cdylib converts them into
plain structs + unit enums before returning to frb. See `api/terminal.rs`
(`TerminalEvent`) and `api/review.rs` (`ApplyResult`, `ReviewFileDto`, etc.) for the
pattern.

**Auth** — `flutter_secure_storage` stores the server URL and bearer token in the
platform keystore (Android Keystore / Keychain / libsecret). The token is never
written to plain shared preferences.

## Features

- **Connect + auth** — server URL + bearer token, tested against `/health` before
  saving; reconnect goes through the same `CommanderStore` so a settings change
  can't abandon a handle.
- **Session list** — grouped by project in workspace order, with unread markers,
  live agent-state chips, and a connection-health indicator (connected /
  connecting / degraded) fed by the poller.
- **Session detail** — lifecycle actions (kill/restart/delete), rename, set
  section, keep-alive toggle; pane preview and diff stat fetched on demand.
- **Create session** — form driven by the server's create-options: a program
  picker (falls back to free text if options fail to load), section, base branch,
  optional initial prompt.
- **Live terminal** — WebSocket attach (agent or shell) rendered with the
  `xterm.dart` fork; a desktop pane on wide layouts, a pushed route on phone.
- **Review** — diff view with inline comments, snippet-based re-anchoring, and
  apply; on-demand blob loading for images, with per-file reviewed toggles.
- **Programs list editing** — a dedicated settings page (`ProgramsPage`) edits the
  server's launch-program list (`PUT /config/programs`), the same list the create
  form offers.
- **Session management** — unread markers on the list with mark-read on open,
  plus rename, set-section, and a keep-alive toggle from the detail view.
- **Projects** — a dedicated `ProjectsPage` (from the settings menu) lists,
  adds, removes, and scans server-side project paths and browses each project's
  branches (`addProject`/`removeProject`/`scanDirectory`/`listBranches`).
- **Cascade / push-stack** — triggered from the session detail view with their
  operation outcome reported (`cascadeMerge`/`pushStack`); a paused cascade shows
  a global resume/abandon banner (`cascadeResume`/`cascadeAbandon`).

## Multi-server

The app connects to one server today. The handle-per-server registry (cdylib
side) and the reserved server-sidebar slot (`_ServerSidebar` in `adaptive_shell.dart`,
currently rendering a single row) are the seams a multi-server client grows into —
a design for that exists but isn't built; it's a follow-up branch.

## Dev environment

Enter the client dev shell with:

```sh
nix develop .#client
```

This is a separate dev shell in the root `flake.nix`; the default shell (`nix develop`)
is lean and never pulls the Flutter/Android toolchain.

The shell provides: Flutter/Dart, Rust (stable) + four Android cross-compile
targets (`aarch64`, `armv7`, `x86_64`, `i686` Linux Android) via fenix, the Android
SDK (platforms 34/35/36, build-tools, NDK r28 `28.0.13004108`, emulator + x86\_64
system images), JDK 17, `cargo-ndk`, `flutter_rust_bridge_codegen`, CMake/Ninja/Clang
for the native build, and `pkg-config`/`libclang`.

On Linux the shell also provides the GTK/X11 stack Flutter needs for the desktop
target (`gtk3`, `glib`, `pcre2`, `libepoxy`, `libx11`, `libsecret`), gated behind
`isLinux` so the same shell definition works on macOS. macOS uses Cocoa built via
Xcode — those libs are not in the shell on Darwin, and the desktop target itself is
not yet verified there.

**What the shellHook sets up:**

- `ANDROID_HOME`, `ANDROID_SDK_ROOT`, `ANDROID_NDK_ROOT/HOME`, `ANDROID_NDK_VERSION`, `JAVA_HOME` — pointed at the Nix-provided SDK.
- `flutter config --android-sdk` — points Flutter at the Nix SDK (read-only; no auto-install).
- `LD_LIBRARY_PATH=/usr/lib:…` — lets Nix-built libepoxy find the system Mesa EGL at runtime (Linux desktop only).
- `client/rust/target/release → debug` symlink — frb's `ioDirectory` looks for `release/`; the symlink means a debug build is found immediately.
- Creates the Android AVD `cctest` (android-35, google\_apis, x86\_64, Pixel 6) on first entry if it doesn't already exist.

> The Nix SDK is read-only. Every SDK component the build needs must be declared in `flake.nix` (in `platformVersions`, `buildToolsVersions`, `ndkVersions`, etc.) rather than being auto-installed by Gradle or `flutter doctor`.

## Build and run

### Linux desktop

```sh
cd client
flutter run -d linux
```

Requires a display (`DISPLAY` or `WAYLAND_DISPLAY`). The `release → debug` symlink in the shellHook means `flutter run` (debug mode) finds the cdylib without a separate `cargo build` step.

### Android emulator

Boot the AVD the shellHook created (KVM-accelerated, Linux only):

```sh
emulator -avd cctest -no-window -gpu swiftshader_indirect \
         -no-audio -no-boot-anim -accel on &
adb wait-for-device
until adb shell getprop sys.boot_completed 2>/dev/null | grep -q 1; do sleep 3; done
```

Then run:

```sh
cd client
flutter run -d emulator-5554
```

`cargo-ndk` cross-compiles the cdylib for the emulator's x86\_64-linux-android target; Gradle links it into the APK. The `ANDROID_NDK_VERSION` env var (set by the shellHook) pins Gradle to the Nix-provided NDK in both `android/app/build.gradle.kts` and `rust_builder/android/build.gradle`.

### iOS / macOS

Not yet built — needs a Mac with Xcode. The Rust side is kept buildable toward it
(`staticlib` output, `rustls`-only TLS, no Linux-only deps in shared code paths),
but there is no `staticlib` cross-compile target, no Xcode project wiring, and no
verification here.

## frb codegen loop

`flutter_rust_bridge` generates the Dart FFI glue from the Rust API surface. After editing any file under `client/rust/src/api/`:

```sh
cd client
flutter_rust_bridge_codegen generate   # regenerates lib/src/rust/* and rust/src/frb_generated.rs
cargo build                            # verify the cdylib still compiles
```

Commit the regenerated files (`client/lib/src/rust/` and `client/rust/src/frb_generated.rs`) together with the Rust changes. The generated files are checked in so contributors without the full toolchain can still run `flutter analyze` and widget tests.

> `flutter_rust_bridge_codegen` is provided by the dev shell. If it is ever absent from the nixpkgs pin, install it with `cargo install flutter_rust_bridge_codegen --version 2.11.1`.

## Testing

Four layers, all runnable from the client dev shell (`nix develop .#client`, or the
slim `.#clientCi` used by CI):

| Layer | Where | What it covers |
|-------|-------|----------------|
| cdylib unit | `client/rust/src/api/*.rs` `#[cfg(test)]` | pure helpers (URL mapping, id/DTO parsing, handle registry) |
| cdylib ↔ server integration | `client/rust/tests/server_flows.rs` | every blocking HTTP fn against a real in-process server (connect, create/list/detail/kill, restart/delete, join-by-prefix, review round-trip) |
| Dart widget | `client/test/*_test.dart` | each page with a hand-rolled `FakeCommanderApi` (no live bridge), plus `CommanderStore` unit tests |
| Full-stack e2e | `client/integration_test/app_flows_test.dart` | the real app on `-d linux` against a hermetic server |

```sh
# Dart widget tests (fast; no Rust bridge, no server):
cd client && flutter test

# cdylib unit + integration tests (needs tmux; boots a hermetic server in-process):
cd client/rust && cargo test

# Full-stack e2e (boots a hermetic, XDG-isolated server, then drives the app on
# the Linux desktop target). Needs a display; use xvfb-run when headless:
client/tool/e2e.sh              # local, with a display
xvfb-run -a client/tool/e2e.sh  # headless / CI
```

`client/tool/e2e.sh` redirects `XDG_CONFIG_HOME`/`XDG_DATA_HOME` **and** `TMUX_TMPDIR`
into a `mktemp` dir, so the server it launches touches neither your real
config/state/worktrees nor your default tmux server — the whole tree (including the
isolated tmux server) is torn down on exit, even if a test fails mid-run. The
integration harness (`crates/claude-commander-test-support`)
is shared by the server's own integration tests and the cdylib's. CI runs all four
layers via the `client` job in `.github/workflows/ci.yml`.

The integration/e2e server tests self-skip when tmux is absent (a runtime check, not
`#[ignore]`), so they run in CI where tmux is present.

## Feature status

| Phase | Feature | Status |
|-------|---------|--------|
| 0 | `claude-commander-protocol` crate, shared wire types | Done |
| 1 | Connect + auth, session list | Done |
| 2 | Session detail + lifecycle (kill/restart/delete/create) | Done |
| 3 | Live terminal (WebSocket, `xterm.dart`) | Done |
| 4 | Review/diff + inline comments, apply | Done |
| 5 | iOS / macOS | Not started (needs Mac + Xcode) |
| 6 | Shared `claude-commander-client` transport crate (also backs the TUI's remote sessions) | Done |
| 7 | Adaptive desktop shell (master-detail), programs-list editing, multi-server seams | Done / in progress |

**Measured throughput (Phase 3 spike, debug builds):**
- Linux desktop: 23.7 MB/s end-to-end (frb stream → UTF-8 decode → xterm.dart VT parse/write → paint)
- Android emulator (x86\_64, KVM): 21.6 MB/s

Real PTY output sustains well under 2 MB/s, so both targets have comfortable headroom.

Review images load on demand (`GET /sessions/{id}/blob`) and per-file reviewed marks are togglable (`POST /sessions/{id}/files/reviewed`). Non-image binaries still show a placeholder.

## xterm.dart fork

The terminal view uses a fork of xterm.dart pinned to `github.com/sizeak/xterm.dart`, branch `commander` (at the v4.0.0 commit). The upstream package is lightly maintained; the fork carries mobile/touch fixes and lets the project cherry-pick community PRs without waiting on upstream. Carry local patches on the `commander` branch and upstream them where possible.

The pubspec dependency:

```yaml
xterm:
  git:
    url: https://github.com/sizeak/xterm.dart.git
    ref: commander
```
