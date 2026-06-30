# Claude Commander — client

Cross-platform client for Claude Commander, living in this monorepo alongside the
core library, TUI, and server. It talks to the `claude-commander-server` HTTP +
WebSocket API with a Flutter UI (including an `xterm.dart` terminal widget) over a
Rust `cdylib` core bridged via
[`flutter_rust_bridge`](https://github.com/fzyzcjy/flutter_rust_bridge).

The Rust core takes a **path dependency** on the in-repo `claude-commander-protocol`
crate (`../crates/claude-commander-protocol`) for the shared wire types, so the
client stays in lockstep with the server by construction.

**Target order:** Android first, then iOS, with desktop (Linux/macOS/Windows) to
follow — hence `client/`, not `mobile/`.

## Toolchain: the `client` dev shell

The Flutter + Android NDK toolchain is heavy and only needed by people building the
client, so it lives in a **separate dev shell within the repo-root `flake.nix`**
(`devShells.client`) rather than the default shell. The root `nix develop` (core
TUI/CLI/server) stays lean and never pulls Flutter/Android.

Enter it from the repo root:

```sh
nix develop .#client
```

Or with direnv (recommended), from this directory (`client/.envrc` points at the
root flake's `client` shell):

```sh
direnv allow
```

The shell provides:

- `flutter` + `dart` (nixpkgs)
- A Rust stable toolchain (via [fenix](https://github.com/nix-community/fenix)) with the
  Android cross-compile targets: `aarch64-linux-android`, `armv7-linux-androideabi`,
  `x86_64-linux-android`, `i686-linux-android`
- `cargo-ndk`, `flutter_rust_bridge_codegen`
- Android SDK + NDK r28 (via nixpkgs `androidenv`), JDK 17
- `cmake`, `ninja`, `pkg-config`, `clang`, and Linux GTK deps for desktop builds

It exports `ANDROID_HOME` / `ANDROID_SDK_ROOT` / `ANDROID_NDK_ROOT` / `JAVA_HOME`
and points Flutter at the Nix-provided Android SDK.

### If `flutter_rust_bridge_codegen` is missing from your nixpkgs pin

Remove it from the `devShells.client` package list in the root `flake.nix` and
install it in-shell instead:

```sh
cargo install flutter_rust_bridge_codegen
# or as a Dart global tool:
dart pub global activate flutter_rust_bridge_codegen
```

## Status

Toolchain-only. The Flutter app and Rust core have not been created yet. The Rust
`cdylib` (once created at `client/rust/`) will be excluded from the root Cargo
workspace so `cargo build` for the TUI/server never pulls in the client deps.
