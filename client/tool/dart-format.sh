#!/usr/bin/env bash
#
# Format (or check) the client's Dart sources.
#
#   dart-format.sh                 format the whole tracked client tree
#   dart-format.sh --check         fail if anything is unformatted (CI)
#   dart-format.sh FILE...         format just these files (pre-commit)
#
# The dir list lives here rather than being repeated in ci.yml and
# .pre-commit-config.yaml. It deliberately names the tracked source dirs
# instead of `client/`: a bare `dart format client` also rewrites generated
# build artifacts under client/build.
#
# Arguments are repo-root-relative (that is what pre-commit passes); the script
# cds to the repo root so both they and the `.#clientCi` flake ref resolve.
#
# Dart comes from the client dev shell. If it is not already on PATH we
# re-enter the shell ourselves, so the hook works from a plain terminal too.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

mode=()
if [ "${1:-}" = "--check" ]; then
  mode=(--output=none --show=changed --set-exit-if-changed)
  shift
fi

if [ "$#" -gt 0 ]; then
  targets=("$@")
else
  targets=(
    client/lib
    client/test
    client/integration_test
    client/test_driver
    client/rust_builder
  )
fi

if command -v dart >/dev/null 2>&1; then
  exec dart format "${mode[@]}" "${targets[@]}"
elif command -v nix >/dev/null 2>&1; then
  # `clientCi` is the Linux CI shell — GTK, xvfb and Mesa — and it does not
  # evaluate on darwin at all, so a macOS contributor's pre-commit hook fails
  # here rather than formatting anything. `clientApple` is the darwin
  # equivalent and carries the same Flutter/Dart toolchain.
  case "$(uname -s)" in
    Darwin) shell=".#clientApple" ;;
    *) shell=".#clientCi" ;;
  esac
  exec nix develop "$shell" -c dart format "${mode[@]}" "${targets[@]}"
else
  echo "dart-format: no 'dart' on PATH and no 'nix' to provide one." >&2
  echo "             Enter the client dev shell first: nix develop .#client" >&2
  exit 1
fi
