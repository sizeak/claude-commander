#!/usr/bin/env bash
#
# Shared helpers for scripts/verify.sh and scripts/dev-run.sh.
#
# Sourced, never executed. The sourcing script owns `set -euo pipefail`; this
# file deliberately does not set shell options so that sourcing it can never
# change the caller's error handling out from under it.
#
# Three groups of things live here:
#
#   * Pure name/value helpers (cc_crate_for_alias, cc_exit_code_for_lane,
#     cc_lanes_for_tier, cc_android_package_id, cc_apk_path,
#     cc_flutter_build_args). These are what scripts/tests/run.sh asserts on.
#   * Toolchain entry (cc_run_in_shell), which reproduces the rule already used
#     by client/tool/dart-format.sh: use the tool on PATH if it is there,
#     otherwise re-enter the nix dev shell that provides it.
#   * The lane runner (cc_lane, cc_print_summary), which gives verify.sh its
#     run-everything / distinct-exit-code behaviour.

# ---------------------------------------------------------------------------
# Paths and reserved exit codes
# ---------------------------------------------------------------------------

# Resolved from this file's own location so both scripts work from any cwd.
CC_REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# The caller's directory, captured at source time -- i.e. before either script
# cds to the repo root. Relative paths a user passes on the command line have to
# resolve against this, not against $PWD after the cd.
CC_INVOCATION_DIR="$PWD"
# Under /target/, which .gitignore already excludes.
CC_LOG_DIR="$CC_REPO_ROOT/target/verify-logs"

# Reserved statuses, kept clear of every lane code so a caller can always tell
# "you invoked me wrong" and "your machine is missing a tool" apart from "a
# check failed".
readonly CC_EXIT_USAGE=2
readonly CC_EXIT_TOOLCHAIN=3

# Internal sentinel a lane returns to report itself skipped rather than failed.
# Not an exit code: it never leaves the script.
readonly CC_LANE_SKIPPED=77

# Marks where a command's own output starts, so a captured value can be told
# apart from the dev shell's shellHook chatter. See cc_capture_in_shell.
readonly CC_OUTPUT_SENTINEL="__cc_output_begins__"

# The Android AVD created by flake.nix's `client` shellHook.
CC_AVD="${CC_AVD:-cctest}"
# Dev shell providing flutter/dart. `.#clientCi` is the slim one CI uses and
# that client/tool/dart-format.sh re-enters; `.#client` additionally carries the
# Android SDK/NDK and is what dev-run.sh's android target needs.
#
# `.#clientCi` is Linux-only — it pulls xvfb-run, gtk3, mesa and libGL
# unconditionally, so it does not evaluate on darwin at all. A Mac therefore
# takes `.#clientApple`, which carries the same Flutter/Dart toolchain; the same
# substitution client/tool/dart-format.sh makes. Without it every client lane
# fails on a Mac with no flutter on PATH, and `CC_FORCE_NIX=1` — which skips the
# PATH shortcut, and which CLAUDE.md makes mandatory for golden changes — fails
# unconditionally.
if [ -z "${CC_CLIENT_SHELL:-}" ]; then
  case "$(uname -s)" in
    Darwin) CC_CLIENT_SHELL=".#clientApple" ;;
    *) CC_CLIENT_SHELL=".#clientCi" ;;
  esac
fi
CC_ANDROID_SHELL="${CC_ANDROID_SHELL:-.#client}"

# ---------------------------------------------------------------------------
# Output
# ---------------------------------------------------------------------------

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  CC_C_RESET=$'\033[0m'
  CC_C_BOLD=$'\033[1m'
  CC_C_DIM=$'\033[2m'
  CC_C_RED=$'\033[31m'
  CC_C_GREEN=$'\033[32m'
  CC_C_YELLOW=$'\033[33m'
else
  CC_C_RESET='' CC_C_BOLD='' CC_C_DIM='' CC_C_RED='' CC_C_GREEN='' CC_C_YELLOW=''
fi

cc_info() { printf '%s==>%s %s\n' "$CC_C_BOLD" "$CC_C_RESET" "$*"; }
cc_warn() { printf '%swarn:%s %s\n' "$CC_C_YELLOW" "$CC_C_RESET" "$*" >&2; }
cc_error() { printf '%serror:%s %s\n' "$CC_C_RED" "$CC_C_RESET" "$*" >&2; }

# cc_die <exit_code> <message...>
cc_die() {
  local code="$1"
  shift
  cc_error "$*"
  exit "$code"
}

# ---------------------------------------------------------------------------
# Pure helpers
# ---------------------------------------------------------------------------

# cc_crate_for_alias <alias> -- shorthand to workspace crate name.
#
# `cargo test -p claude-commander-core <filter>` was the single most-run command
# in this repo's history, so the shorthand exists to make it cheap to type. A
# fully-spelled crate name passes through unchanged, so the shorthand is
# additive and never the only accepted spelling.
cc_crate_for_alias() {
  local alias="${1:-}"
  case "$alias" in
    core) printf 'claude-commander-core\n' ;;
    cli | commander) printf 'claude-commander\n' ;;
    server) printf 'claude-commander-server\n' ;;
    protocol) printf 'claude-commander-protocol\n' ;;
    remote) printf 'claude-commander-remote\n' ;;
    client) printf 'claude-commander-client\n' ;;
    test-support) printf 'claude-commander-test-support\n' ;;
    claude-commander | claude-commander-*) printf '%s\n' "$alias" ;;
    *)
      cc_error "unknown crate '$alias' (try: core cli server protocol remote client test-support)"
      return 1
      ;;
  esac
}

# cc_exit_code_for_lane <lane> -- the lane's reserved process exit code.
#
# One code per lane so a wrapper can distinguish "clippy is dirty" from "the
# tests broke" without parsing logs. Keep these stable; they are documented in
# CLAUDE.md and anything scripting verify.sh depends on them.
cc_exit_code_for_lane() {
  case "${1:-}" in
    # Rust lanes: 10-19
    fmt) printf '10\n' ;;
    clippy) printf '11\n' ;;
    build) printf '12\n' ;;
    test) printf '13\n' ;;
    # Client lanes: 20-29
    pub-get) printf '20\n' ;;
    dart-format) printf '21\n' ;;
    analyze) printf '22\n' ;;
    flutter-test) printf '23\n' ;;
    cdylib) printf '24\n' ;;
    e2e) printf '25\n' ;;
    goldens) printf '26\n' ;;
    # Whole-repo lanes: 30-39
    nix-build) printf '30\n' ;;
    packaging) printf '31\n' ;;
    shellcheck) printf '32\n' ;;
    selftest) printf '33\n' ;;
    nix-src-filter) printf '34\n' ;;
    *)
      cc_error "unknown lane '${1:-}'"
      return 1
      ;;
  esac
}

# cc_lane_description <lane> -- one line for --list and the summary table.
cc_lane_description() {
  case "${1:-}" in
    fmt) printf 'cargo fmt --all -- --check\n' ;;
    clippy) printf 'cargo clippy --workspace --all-targets -- -D warnings\n' ;;
    build) printf 'cargo build --workspace --all-targets\n' ;;
    test) printf 'cargo test --workspace\n' ;;
    pub-get) printf 'flutter pub get (client lanes need resolved packages)\n' ;;
    dart-format) printf 'client/tool/dart-format.sh --check\n' ;;
    analyze) printf 'flutter analyze lib test integration_test\n' ;;
    flutter-test) printf 'flutter test (client widget + golden tests)\n' ;;
    cdylib) printf 'cargo test in client/rust\n' ;;
    e2e) printf 'client/tool/e2e.sh (hermetic server + Linux app)\n' ;;
    goldens) printf 'flutter test test/goldens (reference images only)\n' ;;
    nix-build) printf 'nix build\n' ;;
    nix-src-filter) printf 'scripts/check-nix-src-filter.sh (flake src filter guard)\n' ;;
    packaging) printf 'cargo install --path crates/claude-commander (Homebrew path)\n' ;;
    shellcheck) printf 'shellcheck -x over scripts/, client/tool/, docs/tool/\n' ;;
    selftest) printf 'scripts/tests/run.sh\n' ;;
    *)
      cc_error "unknown lane '${1:-}'"
      return 1
      ;;
  esac
}

# cc_lanes_for_tier <tier> -- space-separated lane list for a tier.
#
# Ordered cheapest-first within and across families, so a one-second fmt or
# clippy failure surfaces before a multi-minute `nix build` has been paid for.
# cc_lane_run_order -- every lane that exists, cheapest first.
#
# Distinct from `cc_lanes_for_tier all` on purpose: that tier is the CI mirror,
# and a lane can exist outside it (goldens, a focused subset of flutter-test).
# The runner iterates THIS list so flag combinations cannot reorder lanes or run
# one twice -- and so a lane outside every tier still gets a chance to run, which
# is how the goldens lane was silently skipped when the runner walked the `all`
# tier instead.
cc_lane_run_order() {
  printf 'fmt clippy build test pub-get dart-format analyze flutter-test goldens cdylib shellcheck selftest nix-src-filter e2e nix-build packaging\n'
}

cc_lanes_for_tier() {
  case "${1:-}" in
    fast) printf 'fmt clippy\n' ;;
    rust) printf 'fmt clippy build test\n' ;;
    client) printf 'pub-get dart-format analyze flutter-test cdylib\n' ;;
    # A focused subset of flutter-test, for the golden-image loop. Deliberately
    # NOT in the client or all tiers: `flutter test` already runs the goldens, so
    # including it there would rasterise every reference image twice.
    goldens) printf 'pub-get goldens\n' ;;
    all)
      printf 'fmt clippy build test pub-get dart-format analyze flutter-test cdylib shellcheck selftest nix-src-filter e2e nix-build packaging\n'
      ;;
    *)
      cc_error "unknown tier '${1:-}' (try: fast rust client goldens all)"
      return 1
      ;;
  esac
}

# cc_android_package_id <gradle_kts_file> -- the app's applicationId.
#
# Read at run time rather than baked into the script: this id has already moved
# once (com.example.claude_commander_client ->
# com.claudecommander.claude_commander_client), and a stale literal silently
# launches nothing. `namespace` is declared just above it and looks identical to
# a naive dotted-string grep, so match the `applicationId` assignment itself.
cc_android_package_id() {
  local gradle_file="${1:-}"
  if [ ! -f "$gradle_file" ]; then
    cc_error "no gradle file at '$gradle_file'"
    return 1
  fi
  local id
  id="$(sed -n 's/^[[:space:]]*applicationId[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' \
    "$gradle_file" | head -1)"
  if [ -z "$id" ]; then
    cc_error "no applicationId found in '$gradle_file'"
    return 1
  fi
  printf '%s\n' "$id"
}

# cc_apk_path <debug|release> -- APK path relative to client/.
cc_apk_path() {
  case "${1:-}" in
    debug) printf 'build/app/outputs/flutter-apk/app-debug.apk\n' ;;
    release) printf 'build/app/outputs/flutter-apk/app-release.apk\n' ;;
    *)
      cc_error "unknown build profile '${1:-}' (want: debug or release)"
      return 1
      ;;
  esac
}

# cc_flutter_build_args <debug|release> -- flutter args for that profile.
cc_flutter_build_args() {
  case "${1:-}" in
    debug) printf 'build apk --debug\n' ;;
    release) printf 'build apk --release\n' ;;
    *)
      cc_error "unknown build profile '${1:-}' (want: debug or release)"
      return 1
      ;;
  esac
}

# cc_strip_shell_preamble <text> -- drop everything up to and including the
# sentinel line.
#
# The `.#client` dev shell's shellHook writes to STDOUT, not stderr: it echoes
# "entered claude-commander client dev shell …" on every entry, plus AVD-creation
# notices the first time. Verified directly --
# `nix develop .#client -c bash -c 'echo REAL'` prints the hook line first, then
# REAL. So any value captured through that shell arrives with a preamble glued to
# the front of it, and `$(… adb devices …)` yields the hook line rather than a
# serial. This is what the hand-written `grep -vE "entered claude|Creating
# Android|AVD creation"` filters throughout this repo's history were working
# around; a sentinel is exact where a pattern list is guesswork.
#
# Matching is whole-line, so a payload that merely mentions the sentinel is not
# truncated. Text with no sentinel passes through unchanged: that means the
# wrapper was bypassed, and returning nothing would turn a wiring mistake into a
# silent empty value.
cc_strip_shell_preamble() {
  local text="${1:-}"
  if ! printf '%s\n' "$text" | grep -qxF "$CC_OUTPUT_SENTINEL"; then
    printf '%s\n' "$text"
    return 0
  fi
  printf '%s\n' "$text" |
    awk -v sentinel="$CC_OUTPUT_SENTINEL" 'seen { print } $0 == sentinel { seen = 1 }'
}

# cc_quote_args [arg...] -- shell-quote each argument, space separated.
#
# Both scripts run their commands through `bash -c` (that is how re-entering a
# nix dev shell works), so caller-supplied values cross a shell boundary and
# would otherwise be re-split on whitespace or interpreted as syntax.
cc_quote_args() {
  local out="" arg
  for arg in "$@"; do
    out+="${out:+ }$(printf '%q' "$arg")"
  done
  printf '%s\n' "$out"
}

# cc_format_duration <seconds> -- "42s" / "3m12s".
cc_format_duration() {
  local secs="${1:-0}"
  if [ "$secs" -lt 60 ]; then
    printf '%ds\n' "$secs"
  else
    printf '%dm%02ds\n' "$((secs / 60))" "$((secs % 60))"
  fi
}

# ---------------------------------------------------------------------------
# Toolchain entry
# ---------------------------------------------------------------------------

# cc_run_in_shell <shell_ref> <probe_tool> <script...>
#
# Runs <script> with the toolchain guaranteed present: directly when
# <probe_tool> is already on PATH, otherwise by re-entering the named nix dev
# shell. An empty <shell_ref> means the flake's default shell.
#
# This is the rule client/tool/dart-format.sh already follows, and it matters
# for speed as much as correctness: inside `nix develop .#client` there is
# nothing to re-evaluate, while from a bare terminal the script still works.
cc_run_in_shell() {
  local shell_ref="$1" probe="$2"
  shift 2
  local script="$*"

  # The PATH shortcut, unless CC_FORCE_NIX=1 asks for every command to go through
  # the flake's toolchain the way CI does (without which an ambient cargo/clippy of
  # a different version can disagree with CI in either direction).
  if [ -z "${CC_FORCE_NIX:-}" ] && command -v "$probe" >/dev/null 2>&1; then
    bash -c "$script"
    return $?
  fi

  # Everything else needs nix. One invocation site for both reasons we get here,
  # so the two paths cannot drift apart.
  if ! command -v nix >/dev/null 2>&1; then
    if [ -n "${CC_FORCE_NIX:-}" ]; then
      cc_error "CC_FORCE_NIX is set but nix is not installed."
    else
      cc_error "no '$probe' on PATH and no 'nix' to provide one."
      cc_error "enter the dev shell first: nix develop ${shell_ref:-}"
    fi
    return "$CC_EXIT_TOOLCHAIN"
  fi

  if [ -n "$shell_ref" ]; then
    nix develop "$shell_ref" -c bash -c "$script"
  else
    nix develop -c bash -c "$script"
  fi
}

# cc_capture_in_shell <shell_ref> <probe_tool> <script...>
#
# cc_run_in_shell when you need the command's stdout as a value. Prints the
# sentinel first so the dev shell's shellHook chatter can be stripped off the
# front; see cc_strip_shell_preamble for why that is necessary.
cc_capture_in_shell() {
  local shell_ref="$1" probe="$2"
  shift 2
  local raw status=0
  raw="$(cc_run_in_shell "$shell_ref" "$probe" "printf '%s\n' '$CC_OUTPUT_SENTINEL'; $*")" || status=$?
  [ "$status" -eq 0 ] || return "$status"
  cc_strip_shell_preamble "$raw"
}

# cc_usage_from_header <script_file> -- print the script's header comment block
# as help text.
#
# Starts at line 3 (past the shebang and the blank comment line) and stops at the
# first non-comment line. Keyed on the comment marker rather than a line range so
# that editing a header cannot silently leak code into --help.
cc_usage_from_header() {
  awk 'NR >= 3 { if (!/^#/) exit; sub(/^# ?/, ""); print }' "$1"
}

# cc_have_display -- true when a GUI target can actually open a window.
cc_have_display() {
  [ -n "${DISPLAY:-}" ] || [ -n "${WAYLAND_DISPLAY:-}" ]
}

# ---------------------------------------------------------------------------
# Lane runner
# ---------------------------------------------------------------------------

CC_LANE_NAMES=()
CC_LANE_STATUS=()
CC_LANE_SECS=()
CC_LANE_NOTES=()
# Reason set by a lane function that returns CC_LANE_SKIPPED.
CC_SKIP_REASON=""
# How many log lines to echo when a lane fails. The audit that motivated these
# scripts showed essentially every manual invocation ending in `| tail -N`.
CC_FAIL_TAIL="${CC_FAIL_TAIL:-40}"

# cc_lane <lane> <fn> [args...]
#
# Times <fn>, captures its output to target/verify-logs/<lane>.log, records
# PASS/FAIL/SKIP, and on failure prints the tail of the log plus its path.
# Always returns 0: the run continues, and cc_print_summary decides the process
# exit status. A lane function reports "not applicable here" by returning
# $CC_LANE_SKIPPED with $CC_SKIP_REASON set.
cc_lane() {
  local lane="$1"
  shift

  mkdir -p "$CC_LOG_DIR"
  local log="$CC_LOG_DIR/$lane.log"
  local start=$SECONDS
  local status=0

  printf '%s==>%s %-12s %s%s%s\n' \
    "$CC_C_BOLD" "$CC_C_RESET" "$lane" "$CC_C_DIM" "$(cc_lane_description "$lane")" "$CC_C_RESET"

  CC_SKIP_REASON=""
  if "$@" >"$log" 2>&1; then
    status=0
  else
    status=$?
  fi
  local elapsed=$((SECONDS - start))

  CC_LANE_NAMES+=("$lane")
  CC_LANE_SECS+=("$elapsed")

  if [ "$status" -eq 0 ]; then
    CC_LANE_STATUS+=("PASS")
    CC_LANE_NOTES+=("")
  # A reason is required, not optional: 77 is also automake's skip convention, so
  # a lane's underlying tool could exit 77 on its own and would otherwise be
  # reported as a deliberate skip and let the run pass. Only this file's lane
  # functions set CC_SKIP_REASON (reset to empty just above), so its presence is
  # what distinguishes "we chose to skip" from "the tool happened to exit 77".
  elif [ "$status" -eq "$CC_LANE_SKIPPED" ] && [ -n "$CC_SKIP_REASON" ]; then
    CC_LANE_STATUS+=("SKIP")
    CC_LANE_NOTES+=("$CC_SKIP_REASON")
    printf '    %sskipped: %s%s\n' "$CC_C_YELLOW" "$CC_SKIP_REASON" "$CC_C_RESET"
  else
    CC_LANE_STATUS+=("FAIL")
    CC_LANE_NOTES+=("exit $(cc_exit_code_for_lane "$lane")")
    printf '    %slast %s lines of %s:%s\n' \
      "$CC_C_DIM" "$CC_FAIL_TAIL" "${log#"$CC_REPO_ROOT"/}" "$CC_C_RESET"
    sed -e 's/^/    | /' <(tail -n "$CC_FAIL_TAIL" "$log")
  fi
  return 0
}

# cc_print_summary -- print the PASS/FAIL/SKIP table; return the first failing
# lane's reserved exit code (0 when nothing failed).
cc_print_summary() {
  local width=0 i
  for i in "${!CC_LANE_NAMES[@]}"; do
    [ "${#CC_LANE_NAMES[$i]}" -gt "$width" ] && width="${#CC_LANE_NAMES[$i]}"
  done

  local failed=0 first_fail=0 skipped=0
  printf '\n%s-- verify summary %s%s\n' \
    "$CC_C_BOLD" "$(printf '%.0s-' $(seq 1 $((width + 24))))" "$CC_C_RESET"
  for i in "${!CC_LANE_NAMES[@]}"; do
    local lane="${CC_LANE_NAMES[$i]}" st="${CC_LANE_STATUS[$i]}"
    local colour="$CC_C_GREEN"
    case "$st" in
      FAIL) colour="$CC_C_RED" ;;
      SKIP) colour="$CC_C_YELLOW" ;;
    esac
    printf '  %s%-4s%s  %-*s  %6s  %s\n' \
      "$colour" "$st" "$CC_C_RESET" "$width" "$lane" \
      "$(cc_format_duration "${CC_LANE_SECS[$i]}")" "${CC_LANE_NOTES[$i]}"
    if [ "$st" = "FAIL" ]; then
      failed=$((failed + 1))
      [ "$first_fail" -eq 0 ] && first_fail="$(cc_exit_code_for_lane "$lane")"
    fi
    [ "$st" = "SKIP" ] && skipped=$((skipped + 1))
  done
  printf '%s%s%s\n' "$CC_C_BOLD" "$(printf '%.0s-' $(seq 1 $((width + 42))))" "$CC_C_RESET"

  local total="${#CC_LANE_NAMES[@]}"
  if [ "$failed" -eq 0 ]; then
    local msg="$total lanes passed"
    [ "$skipped" -gt 0 ] && msg="$msg ($skipped skipped)"
    printf '%s%s%s\n' "$CC_C_GREEN" "$msg" "$CC_C_RESET"
    return 0
  fi

  printf '%s%s of %s lanes failed -> exiting %s%s\n' \
    "$CC_C_RED" "$failed" "$total" "$first_fail" "$CC_C_RESET"
  printf '%slogs: %s%s\n' "$CC_C_DIM" "${CC_LOG_DIR#"$CC_REPO_ROOT"/}" "$CC_C_RESET"
  return "$first_fail"
}
