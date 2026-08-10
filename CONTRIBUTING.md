# Contributing

Maintainer and developer documentation for Claude Commander. Coding conventions,
testing rules, and architecture notes for working in the codebase live in
[`CLAUDE.md`](CLAUDE.md); this file covers releasing, the local dev loop, and a
high-level architecture overview.

## Releasing

Releases are cut with [`cargo-release`](https://github.com/crate-ci/cargo-release):

```bash
cargo install cargo-release            # one-time

cargo release patch                    # X.Y.Z -> X.Y.(Z+1), dry-run
cargo release minor                    # X.Y.Z -> X.(Y+1).0, dry-run
cargo release major                    # X.Y.Z -> (X+1).0.0, dry-run
cargo release 1.2.3                    # explicit version, dry-run

cargo release <patch|minor|major|X.Y.Z> --execute   # actually release
```

Every invocation is a dry-run by default; add `--execute` once the printed plan looks right. The command bumps the version in `Cargo.toml`, refreshes `Cargo.lock`, creates a GPG-signed commit (`Bump version to X.Y.Z`) and a GPG-signed tag (`vX.Y.Z`), and pushes both to `origin/main`.

The tag push triggers `.github/workflows/publish-tap.yml`, which creates the GitHub release with auto-generated notes and bumps the formula in [`sizeak/homebrew-tap`](https://github.com/sizeak/homebrew-tap) so `brew upgrade claude-commander` sees the new version within ~60 seconds. In parallel, `.github/workflows/publish-aur.yml` rewrites the `sha256sums` line in `packaging/aur/PKGBUILD` against the new GitHub source tarball and pushes the result to the [`claude-commander`](https://aur.archlinux.org/packages/claude-commander) AUR package so `yay -Syu claude-commander` picks it up.

The AUR job depends on:

- A repo secret `AUR_SSH_PRIVATE_KEY` containing the private half of an SSH key registered against the maintainer's [aur.archlinux.org](https://aur.archlinux.org) account.
- The package already existing on AUR — the first publish must be done by hand (`git clone ssh://aur@aur.archlinux.org/claude-commander.git`, copy `packaging/aur/PKGBUILD`, run `makepkg --printsrcinfo > .SRCINFO`, commit and push). Every subsequent tag is handled by the workflow.

`cargo-release` keeps `pkgver` in `packaging/aur/PKGBUILD` in sync with `Cargo.toml` via the `pre-release-replacements` block in `release.toml`; the workflow fills in `sha256sums` at publish time.

## Development

```bash
# Everything CI checks for Rust: fmt, clippy, build, test
scripts/verify.sh

# Just the fast pair, for a tight loop
scripts/verify.sh --fast

# One crate, one test
scripts/verify.sh -p core worktree_add

# Add the client lanes (dart format, flutter analyze, flutter test, cdylib)
scripts/verify.sh --client

# Everything CI runs, plus shellcheck, script self-tests and the Flutter e2e
scripts/verify.sh --all

# Run the TUI with debug logging to /tmp/claude-commander.log
scripts/dev-run.sh tui --debug
```

`verify.sh` runs every selected check even after one fails, keeps full output
under `target/verify-logs/`, and exits with the failing lane's own code (10 fmt,
11 clippy, 12 build, 13 test, …) — see `scripts/verify.sh --list`. Its lane list
mirrors `.github/workflows/ci.yml`, so a green `--all` means a green PR (modulo
toolchain drift — see below).

One thing to expect from `--all`: it is a *superset* of CI, also running
`shellcheck`, the script self-tests, and the Flutter `e2e` that CI cannot — so a
red lane there needn't mean red CI. The `e2e` lane needs a display; it reports
`SKIP` (not a failure) when there is neither one nor `xvfb-run`.

By default the lanes use whatever `cargo`/`flutter` is already on your `PATH` and
only re-enter the Nix dev shell when the tool is missing, so a local toolchain
that differs from the flake's can disagree with CI in either direction. Set
`CC_FORCE_NIX=1` to make every lane go through `nix develop` the way CI does.
`scripts/dev-run.sh` covers the launch side (TUI, server, Linux app, Android
emulator + APK deploy); both are documented in
[`CLAUDE.md`](CLAUDE.md#commands).

This project uses [pre-commit](https://pre-commit.com/) to run `cargo fmt` and
`cargo clippy` on every commit, plus `dart format` when the commit touches Dart.
After cloning, run `pre-commit install`.

The Dart hook shells out to `client/tool/dart-format.sh`, which re-enters the
client dev shell if `dart` isn't already on your `PATH` — so it works whether or
not you're inside `nix develop .#client`. The same script backs CI's Format Dart
step (`--check`), so the two can't drift.

### Screenshots

The images in `docs/images/` are **generated**, not hand-captured: both capture
scripts render one hermetic demo workspace (three projects, ten sessions, two PR
stacks, a stand-in agent) so nothing of yours leaks into a screenshot and the
terminal and client images stay consistent. Regenerate after a UI change with
`docs/tool/capture-tui.sh` or
`nix develop .#clientCi -c docs/tool/capture-client.sh`; see
[`docs/tool/README.md`](docs/tool/README.md).

### Architecture

The TUI event loop (`App`) owns the terminal and render state. It sends user commands to a `SessionManager` which coordinates tmux and git operations via async channels. Git read operations use gitoxide (pure Rust); worktree mutations and tmux use CLI subprocesses with semaphore-based throttling.

```
┌───────────────────────────────────────────┐
│              TUI (ratatui)                │
│  Renders widgets, handles input           │
└─────────────────┬─────────────────────────┘
                  │ mpsc channels
┌─────────────────▼─────────────────────────┐
│           SessionManager                  │
│  Session lifecycle, state persistence     │
└──────┬────────────────────┬───────────────┘
       │                    │
┌──────▼──────┐      ┌──────▼──────┐
│ TmuxExecutor│      │ GitBackend  │
│ (async CLI) │      │ (gitoxide)  │
└─────────────┘      └─────────────┘
```
