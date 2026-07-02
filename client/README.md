# Claude Commander client

Android-first Flutter client for [claude-commander-server](../crates/claude-commander-server).
Linux desktop is a supported bonus target. iOS/macOS are not yet implemented (need Xcode on a Mac).

The app talks to the server's HTTP REST API (`/api/…`) and its WebSocket terminal attach endpoint (`/ws/attach`).

## Architecture

```
Flutter UI  ──frb──►  Rust cdylib  ──reqwest/tungstenite──►  claude-commander-server
                            │
                    claude-commander-protocol
                    (shared wire types)
```

**Flutter UI** (`client/lib/`) — Material 3, dark theme. Six pages: connection/auth, session list, session detail + lifecycle (kill/restart/delete), create session, live terminal, and review/diff+comments.

**Rust cdylib** (`client/rust/`) — `rust_lib_claude_commander_client`, compiled as both `cdylib` (Android/Linux `.so`) and `staticlib` (iOS, future). Owns all networking and auth:

- `api/simple.rs` — blocking `reqwest` (rustls, no OpenSSL) for every HTTP endpoint.
- `api/terminal.rs` — WebSocket attach bridge; a shared `tokio` multi-thread runtime drives the socket off the Dart isolate; events stream to Dart via `flutter_rust_bridge` `StreamSink`.
- `api/review.rs` — review/diff and comments HTTP endpoints.
- `api/mirrors.rs` — `#[frb(mirror(…))]` declarations so frb generates typed Dart classes from the protocol types (compile-checked: a field mismatch is a Rust compile error, not a runtime surprise).

**`claude-commander-protocol` crate** (`crates/claude-commander-protocol`) — the single source of truth for wire types (`SessionInfo`, `SessionDetail`, `ReviewSnapshot`, `ClientControl`/`ServerControl` WS frames, etc.). Both the server and the client cdylib depend on it; neither maintains private DTO mirrors. The cdylib does **not** depend on `claude-commander-core` (tmux/gix don't cross-compile to Android).

**DTO convention** — `flutter_rust_bridge` renders data-carrying Rust enums as Dart `freezed` classes, which requires the `build_runner` toolchain. The client deliberately avoids that dependency. Where the protocol types use data enums or tuples (`ApplyOutcome`, `BinaryKind`, `line_range`), the cdylib converts them into plain structs + unit enums before returning to frb. See `api/terminal.rs` (`TerminalEvent`) and `api/review.rs` (`ApplyResult`, `ReviewFileDto`, etc.) for the pattern.

**Auth** — `flutter_secure_storage` stores the server URL and bearer token in the platform keystore (Android Keystore / Keychain / libsecret). The token is never written to plain shared preferences.

## Dev environment

Enter the client dev shell with:

```sh
nix develop .#client
```

This is a separate dev shell in the root `flake.nix`; the default shell (`nix develop`) is lean and never pulls the Flutter/Android toolchain.

The shell provides: Flutter/Dart, Rust (stable) + four Android cross-compile targets (`aarch64`, `armv7`, `x86_64`, `i686` Linux Android) via fenix, the Android SDK (platforms 34/35/36, build-tools, NDK r28 `28.0.13004108`, emulator + x86\_64 system images), JDK 17, `cargo-ndk`, `flutter_rust_bridge_codegen`, CMake/Ninja/Clang for the native build, and `pkg-config`/`libclang`.

On Linux the shell also provides the GTK/X11 stack Flutter needs for the desktop target (`gtk3`, `glib`, `pcre2`, `libepoxy`, `libx11`, `libsecret`). macOS uses Cocoa built via Xcode — those libs are not in the shell on Darwin.

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
| cdylib unit | `client/rust/src/api/*.rs` `#[cfg(test)]` | pure helpers (URL mapping, id/DTO parsing) |
| cdylib ↔ server integration | `client/rust/tests/server_flows.rs` | every blocking HTTP fn against a real in-process server (connect, create/list/detail/kill, restart/delete, join-by-prefix, review round-trip) |
| Dart widget | `client/test/*_test.dart` | each page with a hand-rolled `FakeCommanderApi` (no live bridge) |
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
