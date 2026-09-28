#!/usr/bin/env bash
#
# Browser e2e for the web UI: builds a hermetic commander world, starts the
# server (which serves the page itself), and runs the Playwright suite
# (web/e2e/*.spec.ts) against it headless.
#
# Usage: web/e2e/run.sh [--no-build] [playwright test args...]
#   e.g. web/e2e/run.sh --grep review
#
#   --no-build   use the existing target/debug binary instead of building
#
# Exit status: Playwright's (0 all passed, 1 failures or bad Playwright args),
# 3 missing toolchain / version mismatch, 4 fixture setup failed.
#
# Hermetic by construction — it reuses docs/tool/fixture.sh, whose
# cc_fixture_env puts config/state/worktrees under a temp tree, points
# TMUX_TMPDIR there AND unsets $TMUX/$TMUX_PANE (without which tmux resolves the
# developer's real server and the cleanup's kill-server would nuke it), stubs gh
# and exports DO_NOT_TRACK=1. The trap tears the whole tree down on any exit.
#
# Toolchain: re-enters `nix develop .#web` unless already inside it (probed by
# PLAYWRIGHT_BROWSERS_PATH, which only that shell sets). An ambient node is not
# enough: the browsers must be nixpkgs' pinned build, since Playwright's own
# download can't run on NixOS and would differ from what CI runs anyway.
# CC_WEB_SHELL overrides the shell ref. The server is built with the ambient
# cargo, else the default dev shell.
#
# The page under test is whatever crates/claude-commander-server/webui/ holds:
# a debug build of the server reads it from disk, so rebuild it first
# (`npm run build`) when testing a change to web/src. Beware --no-build after
# `cargo test -p claude-commander-server`: that rebuilds target/debug's server
# binary with the crate's dev-only `debug-embed` feature, which bakes the page
# in as it was then — the suite would silently test a stale page.
#
# The suite itself only knows the page's origin and token (CC_WEB_BASE_URL,
# CC_WEB_TOKEN); where the page comes from is decided here, in serve_page.
set -euo pipefail

WEB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=SCRIPTDIR/../../scripts/lib/dev-common.sh
source "$WEB_DIR/../scripts/lib/dev-common.sh"

BUILD=1
if [ "${1:-}" = "--no-build" ]; then
  BUILD=0
  shift
fi
case "${1:-}" in
-h | --help)
  cc_usage_from_header "${BASH_SOURCE[0]}"
  exit 0
  ;;
esac

if [ "$BUILD" = 1 ]; then
  cc_info "building claude-commander-server…"
  cc_run_in_shell "" cargo \
    "cd '$CC_REPO_ROOT' && cargo build -q -p claude-commander-server"
fi

if [ -z "${PLAYWRIGHT_BROWSERS_PATH:-}" ]; then
  command -v nix >/dev/null 2>&1 || cc_die "$CC_EXIT_TOOLCHAIN" \
    "no PLAYWRIGHT_BROWSERS_PATH and no nix: enter 'nix develop .#web' first"
  exec nix develop "${CC_WEB_SHELL:-$CC_REPO_ROOT#web}" -c "${BASH_SOURCE[0]}" --no-build "$@"
fi

cd "$WEB_DIR"

# A Playwright client drives only the browser revisions it shipped with, so the
# npm pin must equal the nixpkgs driver that built PLAYWRIGHT_BROWSERS_PATH.
pinned="$(node -p 'require("./package.json").devDependencies["@playwright/test"]')"
if [ -n "${CC_PLAYWRIGHT_DRIVER_VERSION:-}" ] && [ "$pinned" != "$CC_PLAYWRIGHT_DRIVER_VERSION" ]; then
  cc_die "$CC_EXIT_TOOLCHAIN" "@playwright/test is pinned to $pinned but nixpkgs' playwright-driver is $CC_PLAYWRIGHT_DRIVER_VERSION — bump them together"
fi

if [ ! -f node_modules/.package-lock.json ] || [ package-lock.json -nt node_modules/.package-lock.json ]; then
  cc_info "npm ci…"
  npm ci --no-audit --no-fund --loglevel=error
fi

# --- fixture -----------------------------------------------------------------

free_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
# fixture.sh reads CC_PORT when sourced.
# shellcheck disable=SC2034
CC_PORT="$(free_port)"
# shellcheck source=SCRIPTDIR/../../docs/tool/fixture.sh
source "$CC_REPO_ROOT/docs/tool/fixture.sh"

LOG_DIR="$WEB_DIR/test-results/fixture-logs"

PHASE="setup"

# shellcheck disable=SC2329  # invoked by the EXIT trap
cleanup() {
  local status=$?
  if [ "$PHASE" = setup ] && [ "$status" -ne 0 ]; then
    cc_error "fixture setup failed (status $status)"
    [ -f "${CC_WORK:-}/server.log" ] && tail -n 40 "$CC_WORK/server.log" >&2
    status=4
  fi
  if [ -n "${CC_WORK:-}" ]; then
    mkdir -p "$LOG_DIR"
    cp "$CC_WORK"/*.log "$LOG_DIR/" 2>/dev/null || true
  fi
  # cc_fixture_cleanup runs a bare `tmux kill-server`; refuse unless it provably
  # targets the throwaway server.
  if [ -z "${TMUX:-}" ] && [ -n "${CC_WORK:-}" ] && [ "${TMUX_TMPDIR:-}" = "$CC_WORK/tmux" ]; then
    cc_fixture_cleanup
  elif [ -z "${CC_SERVER_PID:-}" ] && [ -n "${CC_WORK:-}" ]; then
    # cc_fixture_env failed before isolating tmux, so no server was started and
    # nothing can be using the tree: remove it without touching tmux at all.
    rm -rf "$CC_WORK"
  else
    cc_warn "tmux isolation not provable; leaving ${CC_WORK:-?} and its tmux server alone"
    [ -n "${CC_SERVER_PID:-}" ] && kill "$CC_SERVER_PID" 2>/dev/null || true
  fi
  exit "$status"
}

# Seeded world (titles must match e2e/support.ts):
#   project alpha: "Echo pane" (interactive stand-in), "Review me" (committed
#   diff vs main), "Lifecycle" (for kill/restart)
#   unregistered: repos/beta (added by path), repos/scan/{gamma,delta} (scanned)
seed() {
  local alpha id
  alpha="$(cc_make_repo alpha)"
  id="$(cc_new_session "Echo pane" "$alpha" echo "")"
  id="$(cc_new_session "Review me" "$alpha" idle "review this")"
  cc_dirty_worktree "$id" 3
  cc_new_session "Lifecycle" "$alpha" idle "kill and restart me" >/dev/null
  CC_WEB_FIXTURE_ADD_REPO="$(cc_make_repo beta)"
  cc_make_repo scan/gamma >/dev/null
  cc_make_repo scan/delta >/dev/null
  CC_WEB_FIXTURE_SCAN_DIR="$CC_WORK/repos/scan"
  export CC_WEB_FIXTURE_ADD_REPO CC_WEB_FIXTURE_SCAN_DIR
}

# Export the page's origin. claude-commander-server serves the page itself, so
# that is the server's own URL. Probe the page rather than trusting /health:
# a server that is up but serves no page fails setup (4), not every test.
serve_page() {
  CC_WEB_BASE_URL="$CC_BASE_URL"
  # Captured first, not piped: `grep -q` exits at the match, and under
  # pipefail a curl still writing the rest of the page then fails on SIGPIPE.
  local page
  if page="$(curl -fsS "$CC_WEB_BASE_URL/")" && grep -q "<title>Claude Commander</title>" <<<"$page"; then
    return 0
  fi
  cc_error "the page is not served at $CC_WEB_BASE_URL/"
  return 1
}

[ -x "$CC_REPO_ROOT/target/debug/claude-commander-server" ] ||
  cc_die "$CC_EXIT_TOOLCHAIN" "target/debug/claude-commander-server missing (drop --no-build)"

rm -rf "$LOG_DIR"
# The trap goes in before cc_fixture_env, which mktemps the tree first: a
# failure anywhere after that must still reach cleanup, or the tree leaks.
trap cleanup EXIT
trap 'exit 130' INT TERM
cc_fixture_env
cc_write_config
# Setup steps run bare (not `step || …`, which would disable errexit inside
# them): any failure exits through the trap, which maps it to status 4.
cc_start_server
cc_info "seeding the fixture…"
seed
serve_page
PHASE="test"

export CC_WEB_BASE_URL CC_WEB_TOKEN="$CC_TOKEN"
cc_info "page at $CC_WEB_BASE_URL"

status=0
npx playwright test "$@" || status=$?
exit "$status"
