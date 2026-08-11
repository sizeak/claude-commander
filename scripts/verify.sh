#!/usr/bin/env bash
#
# Run this repo's checks. Prefer it over typing the underlying commands: the
# lane list mirrors .github/workflows/ci.yml, so a green `verify.sh --all` means
# a green PR, and each lane has a reserved exit code (see --list).
#
#   verify.sh                     rust lanes: fmt, clippy, build, test
#   verify.sh --fast              fmt + clippy only (seconds, pre-commit shape)
#   verify.sh --client            rust lanes + dart format/analyze/test/cdylib
#   verify.sh --all               every CI job, plus shellcheck, self-tests, e2e
#   verify.sh --nix               rust lanes + src-filter guard, nix build, packaging
#   verify.sh --e2e               rust lanes + the Flutter e2e (forced)
#   verify.sh --goldens           just the client's golden images
#   verify.sh --goldens --update  regenerate them, then read the image diff
#   verify.sh -p core <filter>    single crate: cargo test -p <crate> <filter>
#   verify.sh --list              print the lane / exit-code table
#
# Every selected lane runs even after an earlier one fails; each lane's output
# goes to target/verify-logs/<lane>.log and only a tail is printed on failure.
# The process exits with the FIRST failing lane's reserved code, so a caller can
# tell "clippy is dirty" (11) from "the tests broke" (13) without reading logs.
#
# Toolchains resolve themselves: a lane uses the tool already on PATH, else it
# re-enters the nix dev shell that provides it (same rule as
# client/tool/dart-format.sh). Override the client shell with CC_CLIENT_SHELL.
set -euo pipefail

# SCRIPTDIR, not the invocation dir: verify.sh is run from anywhere.
# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib/dev-common.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib/dev-common.sh"

cd "$CC_REPO_ROOT"

# The same backstop ci.yml:9-18 sets workflow-wide, and for the same reason it
# gives: `cfg!(test)` only suppresses the *core* crate's own unit tests, so
# downstream crates' `cargo test`/`cargo run` link core as a normal dependency and
# would report to the production OpenObserve endpoint via the baked ingest
# credential. Per-fixture config is deliberately not trusted as the only guard.
# This covers the `test`, `cdylib` and `packaging` lanes (packaging *runs* the
# installed binary); e2e.sh sets its own. A verification sweep is not usage, so it
# must never appear in the telemetry stream -- unlike dev-run.sh's non-isolated
# targets, which are genuine usage and deliberately do NOT set this. Exported so
# it survives `nix develop -c`, which is what ci.yml relies on too.
export DO_NOT_TRACK=1

# A verification sweep should not inherit the developer's terminal either.
# `HeadlessAttach::spawn` (crates/claude-commander-core/src/tmux/headless_attach.rs)
# reads TERM and, via `fallback_term`, normalises an unset/empty/"dumb"/"unknown"
# one to xterm-256color while passing a *real* one straight through to
# `tmux attach`. The server's /ws/attach integration tests drive that path, so a
# developer in kitty or tmux hands them different escape sequences than a
# headless CI runner does. Pinning TERM to the value the fallback itself picks
# makes the path behave the same in both places, without asserting anything about
# a runner's environment. Exported, so it survives `nix develop -c`.
#
# COLORTERM is deliberately NOT pinned. It is read only by ColorMode::detect()
# (tui/theme.rs), and while tests do reach that -- `Theme::default()` is
# `for_color_mode(detect())` and four board-widget tests construct one -- no test
# *assertion* depends on the result: those assert cell symbols and returned
# regions, never styles. Pinning it would imply an assertion depends on it.
export TERM=xterm-256color

FORCE_E2E=0
GOLDENS_UPDATE=0

usage() {
  cc_usage_from_header "${BASH_SOURCE[0]}"
  cat <<EOF

Exit codes:
  0   every selected lane passed
  $CC_EXIT_USAGE   bad arguments
  $CC_EXIT_TOOLCHAIN   a required toolchain is missing and nix could not supply it
  10+ a lane failed; see --list for the per-lane code
EOF
}

list_lanes() {
  # Listed in run order (cheapest first), not by code. This is every lane that
  # exists; the Tiers block below says which flag selects which, since `--all` is
  # the CI mirror and does not include every row (goldens is a subset of
  # flutter-test).
  printf '%s%-13s %-5s %s%s\n' "$CC_C_BOLD" "LANE (run order)" "EXIT" "CHECK" "$CC_C_RESET"
  local lane
  for lane in $(cc_lane_run_order); do
    printf '%-13s %-5s %s\n' \
      "$lane" "$(cc_exit_code_for_lane "$lane")" "$(cc_lane_description "$lane")"
  done
  cat <<EOF

Tiers:
  (default)     $(cc_lanes_for_tier rust)
  --fast        $(cc_lanes_for_tier fast)
  --client      $(cc_lanes_for_tier rust) + $(cc_lanes_for_tier client)
  --goldens     $(cc_lanes_for_tier goldens)
  --all         every lane above
EOF
}

# ---------------------------------------------------------------------------
# Lanes
#
# Each returns non-zero to fail, or $CC_LANE_SKIPPED with $CC_SKIP_REASON set to
# report itself not applicable. Multi-step lanes chain explicitly (&& or
# `|| return`) rather than relying on `set -e`: they run inside an `if` condition
# in cc_lane, where bash suppresses errexit for the whole call tree, so an
# unchained failing step would not stop the ones after it.
# ---------------------------------------------------------------------------

lane_fmt() { cc_run_in_shell "" cargo "cargo fmt --all -- --check"; }
lane_clippy() { cc_run_in_shell "" cargo "cargo clippy --workspace --all-targets -- -D warnings"; }
lane_build() { cc_run_in_shell "" cargo "cargo build --workspace --all-targets"; }
lane_test() { cc_run_in_shell "" cargo "cargo test --workspace"; }

lane_pub_get() {
  # CI's first client step, and it is load-bearing rather than mere setup.
  # `dart format` resolves each file's language version from the nearest
  # .dart_tool/package_config.json; client/rust_builder/cargokit/build_tool is a
  # VENDORED Dart package with its own pubspec.yaml and no .dart_tool of its own,
  # so when client/.dart_tool is stale the formatter falls back to a different
  # language version and rewrites 15 vendored files. Observed directly in this
  # repo: dart-format reported "120 files (15 changed)" against a stale package
  # config and "120 files (0 changed)" -- matching CI exactly -- after a pub get.
  # Unconditional, like CI: a stale package_config that points at absent pub-cache
  # entries is not detectable from mtimes.
  cc_run_in_shell "$CC_CLIENT_SHELL" flutter "cd client && flutter pub get"
}

lane_dart_format() {
  # dart-format.sh re-enters .#clientCi itself when dart is absent, so probing
  # for dart here just avoids paying for the shell twice.
  cc_run_in_shell "$CC_CLIENT_SHELL" dart "client/tool/dart-format.sh --check"
}

lane_analyze() {
  cc_run_in_shell "$CC_CLIENT_SHELL" flutter \
    "cd client && flutter analyze lib test integration_test"
}

lane_flutter_test() {
  cc_run_in_shell "$CC_CLIENT_SHELL" flutter "cd client && flutter test"
}

lane_goldens() {
  # The golden images are rasteriser-sensitive, so this lane is the one place the
  # PATH-first toolchain rule is actively wrong: a local Flutter that differs from
  # the flake's can pass against images CI will reject. Hence the CC_FORCE_NIX
  # nudge rather than a silent pass -- see the Golden tests section of CLAUDE.md
  # for what a green run does and does not prove.
  if [ -z "${CC_FORCE_NIX:-}" ] && command -v flutter >/dev/null 2>&1; then
    cc_warn "using the flutter on PATH; set CC_FORCE_NIX=1 to rasterise with the pinned SDK"
  fi
  local args=""
  [ "$GOLDENS_UPDATE" -eq 1 ] && args=" --update-goldens"
  cc_run_in_shell "$CC_CLIENT_SHELL" flutter "cd client && flutter test test/goldens$args"
}

lane_cdylib() {
  cc_run_in_shell "$CC_CLIENT_SHELL" cargo "cd client/rust && cargo test"
}

lane_e2e() {
  # CI has never been able to run this one (no headless GUI runtime), so a local
  # run is the only coverage the full stack gets. Skip loudly rather than
  # silently when there is nowhere to open a window.
  if [ "$FORCE_E2E" -ne 1 ] && ! cc_have_display && ! command -v xvfb-run >/dev/null 2>&1; then
    CC_SKIP_REASON="no DISPLAY/WAYLAND_DISPLAY and no xvfb-run (force with --e2e)"
    return "$CC_LANE_SKIPPED"
  fi

  local cmd="client/tool/e2e.sh"
  cc_have_display || cmd="xvfb-run -a $cmd"

  # Bounded, unlike the other lanes: this one drives a real GUI app, so it can
  # hang rather than fail, and an unbounded lane would wedge the whole run. Note
  # it can legitimately take a while -- `.#clientCi` pins a different Rust
  # toolchain from the default shell, so e2e.sh's `cargo build -p
  # claude-commander-server` recompiles from scratch into the same target dir.
  # -k: follow TERM with KILL after a grace period. TERM alone is enough for the
  # normal case -- it reaches the whole process group and bash runs e2e.sh's EXIT
  # trap, so the hermetic server, its tmux server and the temp tree are all torn
  # down -- but a GUI process tree that ignores TERM would leave `timeout` waiting
  # forever and the lane hung anyway, which is the thing the bound exists to stop.
  local timeout_secs="${CC_E2E_TIMEOUT:-1800}"
  local status=0
  cc_run_in_shell "$CC_CLIENT_SHELL" flutter "timeout -k 30 $timeout_secs $cmd" || status=$?
  if [ "$status" -eq 124 ]; then
    echo "e2e: TIMED OUT after ${timeout_secs}s (raise CC_E2E_TIMEOUT to allow longer)"
  fi
  return "$status"
}

lane_nix_build() {
  if ! command -v nix >/dev/null 2>&1; then
    CC_SKIP_REASON="nix is not installed"
    return "$CC_LANE_SKIPPED"
  fi
  nix build
}

lane_nix_src_filter() {
  if ! command -v nix >/dev/null 2>&1; then
    CC_SKIP_REASON="nix is not installed"
    return "$CC_LANE_SKIPPED"
  fi
  # Mirrors CI's Check Nix Source Filter step. Evaluation only -- no build -- so
  # it costs a second or two and runs before the nix-build lane it protects.
  ./scripts/check-nix-src-filter.sh
}

lane_packaging() {
  # Mirrors CI's Packaging Install job, which mirrors the Homebrew formula's
  # `cargo install --locked --root <prefix> --path <crate>`. A workspace build
  # is happy with a virtual root manifest, so this is the only check that fails
  # when the packaged crate path drifts (brew installs were broken for eight
  # releases before CI gained it). --debug skips the LTO release profile purely
  # for time; the packaged path is what this guards.
  local prefix
  prefix="$(mktemp -d)"
  # shellcheck disable=SC2064  # expand now, while $prefix is still in scope
  trap "rm -rf '$prefix'" RETURN

  cc_run_in_shell "" cargo \
    "cargo install --locked --debug --root '$prefix' --path crates/claude-commander" || return $?

  # Version via `cargo pkgid` rather than `cargo metadata` + jq: jq is not in the
  # flake's dev shell, and pkgid needs no parser. `cargo help pkgid` documents
  # both the `url#version` and `url#name@version` spellings, so handle each; the
  # observed output here is the former
  # (`path+file:///…/crates/claude-commander#0.32.0`).
  local pkgid version
  pkgid="$(cc_capture_in_shell "" cargo "cargo pkgid -p claude-commander")" || return $?
  version="${pkgid##*#}"
  version="${version##*@}"
  if [ -z "$version" ]; then
    cc_error "could not determine the claude-commander version from '$pkgid'"
    return 1
  fi

  # Same assertion as the formula's `test do` block.
  "$prefix/bin/claude-commander" --version | grep -F "$version"
}

lane_shellcheck() {
  # Entry points only. `-x` makes shellcheck follow their `source` directives,
  # so scripts/lib/dev-common.sh is checked in the context that uses it -- naming
  # it here as well would also check it standalone, where every helper and
  # constant its callers use looks unused (SC2034).
  local -a targets=()
  local f
  for f in scripts/*.sh scripts/tests/*.sh client/tool/*.sh docs/tool/*.sh; do
    [ -f "$f" ] && targets+=("$f")
  done
  if [ "${#targets[@]}" -eq 0 ]; then
    CC_SKIP_REASON="no shell scripts found"
    return "$CC_LANE_SKIPPED"
  fi

  if command -v shellcheck >/dev/null 2>&1; then
    shellcheck -x "${targets[@]}"
  # NOTE: this resolves the flake *registry's* nixpkgs, not this repo's
  # flake.lock pin, so the shellcheck version can drift from a dev-shell one.
  # Acceptable for a lint, but it means a clean local run does not guarantee the
  # same result on another machine.
  elif command -v nix >/dev/null 2>&1; then
    nix run nixpkgs#shellcheck -- -x "${targets[@]}"
  else
    CC_SKIP_REASON="no shellcheck on PATH and no nix to supply it"
    return "$CC_LANE_SKIPPED"
  fi
}

lane_selftest() { scripts/tests/run.sh; }

# run_lane <lane> -- dispatch by name, so the tier lists stay data.
run_lane() {
  case "$1" in
    fmt) cc_lane fmt lane_fmt ;;
    clippy) cc_lane clippy lane_clippy ;;
    build) cc_lane build lane_build ;;
    test) cc_lane test lane_test ;;
    pub-get) cc_lane pub-get lane_pub_get ;;
    dart-format) cc_lane dart-format lane_dart_format ;;
    analyze) cc_lane analyze lane_analyze ;;
    flutter-test) cc_lane flutter-test lane_flutter_test ;;
    cdylib) cc_lane cdylib lane_cdylib ;;
    goldens) cc_lane goldens lane_goldens ;;
    e2e) cc_lane e2e lane_e2e ;;
    nix-src-filter) cc_lane nix-src-filter lane_nix_src_filter ;;
    nix-build) cc_lane nix-build lane_nix_build ;;
    packaging) cc_lane packaging lane_packaging ;;
    shellcheck) cc_lane shellcheck lane_shellcheck ;;
    selftest) cc_lane selftest lane_selftest ;;
    *) cc_die "$CC_EXIT_USAGE" "unknown lane '$1'" ;;
  esac
}

# ---------------------------------------------------------------------------
# Single-crate mode
# ---------------------------------------------------------------------------

# The most-run command in this repo's history, by a wide margin. Exits with
# cargo's own status: this is a focused test run, not a lane sweep, so the lane
# exit-code table does not apply.
run_single_crate() {
  local crate
  crate="$(cc_crate_for_alias "$1")" || exit "$CC_EXIT_USAGE"
  shift
  cc_info "cargo test -p $crate $*"
  cc_run_in_shell "" cargo \
    "cargo test -p $(cc_quote_args "$crate") $(cc_quote_args "$@")"
}

# ---------------------------------------------------------------------------
# Arguments
# ---------------------------------------------------------------------------

declare -A SELECTED=()
select_tier() {
  local lane
  for lane in $(cc_lanes_for_tier "$1"); do SELECTED["$lane"]=1; done
}

TIER_SET=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    -h | --help)
      usage
      exit 0
      ;;
    --list)
      list_lanes
      exit 0
      ;;
    --fast)
      select_tier fast
      TIER_SET=1
      shift
      ;;
    --rust)
      select_tier rust
      TIER_SET=1
      shift
      ;;
    --client)
      select_tier rust
      select_tier client
      TIER_SET=1
      shift
      ;;
    --all)
      select_tier all
      TIER_SET=1
      shift
      ;;
    --nix)
      select_tier rust
      SELECTED[nix-src-filter]=1
      SELECTED[nix-build]=1
      SELECTED[packaging]=1
      TIER_SET=1
      shift
      ;;
    --goldens)
      # Not joined to the rust lanes: this is the focused image loop, and making
      # it wait on a full cargo build would defeat that.
      select_tier goldens
      TIER_SET=1
      shift
      ;;
    --update)
      GOLDENS_UPDATE=1
      shift
      ;;
    --e2e)
      select_tier rust
      # The e2e drives the real Flutter app, so it needs resolved packages too.
      SELECTED[pub-get]=1
      SELECTED[e2e]=1
      FORCE_E2E=1
      TIER_SET=1
      shift
      ;;
    -p | --package)
      [ "$#" -ge 2 ] || cc_die "$CC_EXIT_USAGE" "$1 needs a crate name (try: core)"
      # Single-crate mode replaces the lane sweep rather than joining it, so say
      # so instead of silently discarding a tier the caller asked for.
      [ "$TIER_SET" -eq 1 ] && cc_warn "-p runs one crate's tests only; ignoring the tier flags"
      shift
      run_single_crate "$@"
      exit $?
      ;;
    -*) cc_die "$CC_EXIT_USAGE" "unknown option '$1' (see --help)" ;;
    *) cc_die "$CC_EXIT_USAGE" "unexpected argument '$1' (see --help)" ;;
  esac
done

[ "$TIER_SET" -eq 1 ] || select_tier rust

# --update only means anything to the goldens lane, and silently ignoring it would
# leave the caller believing they had regenerated the images.
if [ "$GOLDENS_UPDATE" -eq 1 ] && [ -z "${SELECTED[goldens]:-}" ]; then
  cc_die "$CC_EXIT_USAGE" "--update only applies to --goldens"
fi

# Emit in the canonical run order so flag combinations cannot reorder lanes or run
# one twice. Note this is cc_lane_run_order, not the `all` tier: a lane that lives
# outside every tier (goldens) must still be runnable when selected directly.
lanes=()
for lane in $(cc_lane_run_order); do
  [ -n "${SELECTED[$lane]:-}" ] && lanes+=("$lane")
done

cc_info "running ${#lanes[@]} lane(s): ${lanes[*]}"
for lane in "${lanes[@]}"; do
  run_lane "$lane"
done

cc_print_summary
