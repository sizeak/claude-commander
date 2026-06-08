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
# Run tests
cargo test

# Run with debug logging
claude-commander --debug

# Check for issues
cargo clippy
```

This project uses [pre-commit](https://pre-commit.com/) to run `cargo fmt` and
`cargo clippy` on every commit. After cloning, run `pre-commit install`.

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
