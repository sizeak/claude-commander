#!/usr/bin/env bash
#
# Capture the TUI's session-list views as SVGs for the README, from the hermetic
# demo fixture in fixture.sh (never the developer's real sessions).
#
# Run from anywhere:  docs/tool/capture-tui.sh [view …]
# Views: stacked (default view mode), board, info. No argument captures all.
#
# Requires: tmux, git, cargo, uv (for the Rich renderer), python3.
set -euo pipefail

# SCRIPTDIR, not the invocation dir: this script is run from anywhere.
# shellcheck source-path=SCRIPTDIR
# shellcheck source=fixture.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/fixture.sh"

COLS="${CC_COLS:-130}"
ROWS="${CC_ROWS:-26}"

# The TUI binary is what this script captures, so the fixture must build it.
# shellcheck disable=SC2034  # read by fixture.sh
CC_NEED_TUI=1

for tool in tmux git cargo uv python3 curl; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "capture-tui: required tool '$tool' not on PATH" >&2
    exit 1
  }
done

trap cc_fixture_cleanup EXIT
cc_fixture_env
cc_write_config
cc_seed_fixture
cc_write_tui_prefs "Fix connection pool leak"

# --- run the TUI on the isolated tmux server --------------------------------
#
# A detached session still allocates a pty of the requested size, so the TUI
# renders a full frame with no client attached.
echo "screenshots: starting the TUI (${COLS}x${ROWS})…"
tmux new-session -d -s cc-capture -x "$COLS" -y "$ROWS" \
  -e COLORTERM=truecolor -e TERM=xterm-256color \
  "$CC_BIN"

# Let the first agent-state poll (3s) and diff computation land before capturing.
sleep 8

capture() {
  local name="$1" title="$2"
  tmux capture-pane -e -p -t cc-capture >"$CC_WORK/$name.ansi"
  uv run --quiet "$CC_TOOL_DIR/ansi-to-svg.py" \
    "$CC_WORK/$name.ansi" "$CC_IMAGES_DIR/$name.svg" \
    --title "$title" --width "$COLS"
  echo "screenshots: wrote docs/images/$name.svg"
}

# Key sequences are sent as the TUI's own keys; each is followed by a settle
# pause so the next capture sees a fully rendered frame.
key() {
  tmux send-keys -t cc-capture "$@"
  sleep 1.5
}

CC_VIEWS=("$@")

# With no arguments every view is captured.
want() {
  [ "${#CC_VIEWS[@]}" -eq 0 ] && return 0
  local view
  for view in "${CC_VIEWS[@]}"; do [ "$view" = "$1" ] && return 0; done
  return 1
}

if want stacked; then
  capture stacked "claude-commander"
fi

if want info; then
  key i
  capture info-modal "claude-commander"
  key Escape
fi

if want board; then
  # `v` cycles project → sections → stacks → board; the fixture starts on stacks.
  key v
  capture board "claude-commander"
fi

echo "screenshots: done."
