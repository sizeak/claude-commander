#!/usr/bin/env bash
#
# Launch a claude-commander target for development. Prefer it over typing the
# underlying commands: the android chain in particular is six steps
# (boot -> wait for boot_completed -> build -> install -> clear logcat ->
# launch) and every step has its own failure mode and exit code.
#
#   dev-run.sh tui [--debug] [-- ARGS…]       the terminal UI
#   dev-run.sh server [--port N] [--token T] [--isolated] [-- ARGS…]
#   dev-run.sh linux [--log FILE]             the Flutter Linux desktop app
#   dev-run.sh android [--device SERIAL] [--release] [--no-launch] [--window]
#   dev-run.sh emulator start|stop|status     headless AVD lifecycle
#
# tui / server / linux run in the foreground and exit with the app's own status.
# android reports where it failed: 40 emulator boot, 41 APK build, 42 install,
# 43 launch. 2 means bad arguments, 3 a missing toolchain nix could not supply.
#
# Toolchains resolve themselves: a target uses the tool already on PATH, else it
# re-enters the nix dev shell that provides it. Android needs `.#client` (SDK +
# NDK + emulator); override with CC_ANDROID_SHELL, the AVD with CC_AVD.
set -euo pipefail

# SCRIPTDIR, not the invocation dir: dev-run.sh is run from anywhere.
# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib/dev-common.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib/dev-common.sh"

cd "$CC_REPO_ROOT"

readonly EXIT_EMULATOR=40
readonly EXIT_APK_BUILD=41
readonly EXIT_INSTALL=42
readonly EXIT_LAUNCH=43

readonly GRADLE_KTS="client/android/app/build.gradle.kts"

usage() { cc_usage_from_header "${BASH_SOURCE[0]}"; }

# ---------------------------------------------------------------------------
# tui
# ---------------------------------------------------------------------------

target_tui() {
  local -a extra=()
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --debug)
        # Logs to /tmp/claude-commander.log, per CLAUDE.md.
        extra+=(--debug)
        shift
        ;;
      --)
        shift
        extra+=("$@")
        break
        ;;
      *) cc_die "$CC_EXIT_USAGE" "tui: unexpected argument '$1'" ;;
    esac
  done

  local args=""
  [ "${#extra[@]}" -gt 0 ] && args=" -- $(cc_quote_args "${extra[@]}")"
  cc_info "cargo run -p claude-commander$args"
  cc_run_in_shell "" cargo "cargo run -p claude-commander$args"
}

# ---------------------------------------------------------------------------
# server
# ---------------------------------------------------------------------------

target_server() {
  local port=8787 token="" isolated=0
  local -a extra=()
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --port)
        [ "$#" -ge 2 ] || cc_die "$CC_EXIT_USAGE" "server: --port needs a value"
        port="$2"
        shift 2
        ;;
      --token)
        [ "$#" -ge 2 ] || cc_die "$CC_EXIT_USAGE" "server: --token needs a value"
        token="$2"
        shift 2
        ;;
      --isolated)
        isolated=1
        shift
        ;;
      --)
        shift
        extra+=("$@")
        break
        ;;
      *) cc_die "$CC_EXIT_USAGE" "server: unexpected argument '$1'" ;;
    esac
  done

  if [ "$isolated" -eq 1 ]; then
    # Same isolation client/tool/e2e.sh uses, for the same reasons: the server's
    # config.toml, state.json and its default worktrees dir all hang off XDG, so
    # redirecting those into a temp tree keeps a dev run away from the real ones.
    local tmp
    tmp="$(mktemp -d)"
    export XDG_CONFIG_HOME="$tmp/cfg"
    export XDG_DATA_HOME="$tmp/data"
    export TMUX_TMPDIR="$tmp/tmux"
    # CRITICAL: tmux clients resolve their socket from $TMUX in PREFERENCE to
    # $TMUX_TMPDIR. Run from inside a tmux session without this unset and the
    # server's sessions land on the developer's real tmux server -- and any
    # cleanup kill-server there takes out every session they have open.
    unset TMUX TMUX_PANE
    # Opt-out-by-default telemetry with a baked credential: without this an
    # isolated dev run still POSTs a session_start to production.
    export DO_NOT_TRACK=1
    mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$TMUX_TMPDIR"
    # Kill the throwaway tmux server BEFORE dropping the temp tree, exactly as
    # client/tool/e2e.sh:61-68 does. tmux daemonizes out of our process group, so
    # every session the isolated server created outlives it; deleting the socket
    # dir first would orphan that server -- and any agent or shell running inside
    # its sessions -- alive, detached and unreachable. Safe to call because
    # TMUX_TMPDIR is still exported and TMUX is unset above, so it can only ever
    # reach the throwaway server in $tmp, never the developer's real one.
    # shellcheck disable=SC2064  # expand $tmp now, while it is still set
    trap "tmux kill-server 2>/dev/null || true; rm -rf '$tmp'" EXIT
    cc_info "isolated state under $tmp (tmux server + tree removed on exit)"
  fi

  local args
  args="--bind 127.0.0.1 --port $(cc_quote_args "$port")"
  # No token flag means the server generates one and logs it once -- which is
  # what you usually want locally, so do not invent a default here.
  [ -n "$token" ] && args="$args --token $(cc_quote_args "$token")"
  [ "${#extra[@]}" -gt 0 ] && args="$args $(cc_quote_args "${extra[@]}")"

  cc_info "cargo run -p claude-commander-server -- $args"
  cc_run_in_shell "" cargo "cargo run -p claude-commander-server -- $args"
}

# ---------------------------------------------------------------------------
# linux desktop
# ---------------------------------------------------------------------------

target_linux() {
  local log=""
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --log)
        [ "$#" -ge 2 ] || cc_die "$CC_EXIT_USAGE" "linux: --log needs a path"
        log="$2"
        shift 2
        ;;
      *) cc_die "$CC_EXIT_USAGE" "linux: unexpected argument '$1'" ;;
    esac
  done

  cc_have_display || cc_die "$CC_EXIT_TOOLCHAIN" \
    "linux: no DISPLAY or WAYLAND_DISPLAY -- the desktop app needs a display"

  # The shellHook's release -> debug symlink in client/rust/target means a debug
  # `flutter run` finds the cdylib without a separate cargo build.
  local cmd="cd client && flutter run -d linux"
  if [ -n "$log" ]; then
    # Resolve to an absolute path FIRST: the command cds into client/, so a
    # relative --log would land at client/<path> rather than beside the caller.
    case "$log" in
      /*) ;;
      *) log="$CC_INVOCATION_DIR/$log" ;;
    esac
    mkdir -p "$(dirname "$log")"
    cc_info "flutter run -d linux (log: $log)"
    # pipefail so a flutter failure is not masked by tee's exit status; this
    # target passes the app's own status through to the caller.
    cmd="set -o pipefail; $cmd 2>&1 | tee $(cc_quote_args "$log")"
  else
    cc_info "flutter run -d linux"
  fi
  cc_run_in_shell "$CC_ANDROID_SHELL" flutter "$cmd"
}

# ---------------------------------------------------------------------------
# emulator
# ---------------------------------------------------------------------------

# Where emulator_start records the launcher PID, so stopping can target a process
# by identity instead of by pattern. See emulator_stop for why that matters.
readonly EMULATOR_PIDFILE="$CC_LOG_DIR/emulator.pid"

# Last-resort match, scoped to this AVD rather than every qemu-system process on
# the host -- a developer may well be running unrelated VMs.
#
# ASSUMPTION, unverified here: that the AVD name appears in the emulator's qemu
# argv. The form `pgrep -af 'qemu-system-x86_64-headless.*cctest'` has been used
# successfully against this project's emulator, but no emulator was running when
# this was written, so it is not confirmed for every emulator build. Two
# consequences, both deliberate: emulator_stop reports a miss rather than widening
# the pattern, and the PID recorded at launch is tried first. Note also that
# `pkill -f` matches any process whose *command line* merely contains this text
# (an editor, a grep, a script mentioning the AVD), which is the other reason the
# pidfile leads. $CC_AVD is interpolated as a regex, so an AVD name containing
# regex metacharacters will not match as written.
readonly QEMU_PATTERN="qemu-system.*${CC_AVD}"

# emulator_running -- true when a qemu process for this AVD is up.
emulator_running() {
  pgrep -f "$QEMU_PATTERN" >/dev/null 2>&1
}

emulator_start() {
  local windowed="${1:-0}"
  if emulator_running; then
    cc_info "emulator already running"
    return 0
  fi

  local window_flag="-no-window"
  [ "$windowed" -eq 1 ] && window_flag=""

  local log="$CC_LOG_DIR/emulator.log"
  mkdir -p "$CC_LOG_DIR"
  cc_info "booting AVD '$CC_AVD' (log: ${log#"$CC_REPO_ROOT"/})"

  # -no-snapshot so each boot is clean; swiftshader_indirect because the AVD is
  # usually headless. Backgrounded inside the shell so nix develop returns. The
  # launcher PID is recorded rather than discarded so emulator_stop can target it
  # directly; it is the `emulator` wrapper's PID, which need not be the qemu
  # process, hence the pattern fallback.
  cc_run_in_shell "$CC_ANDROID_SHELL" emulator "
    nohup emulator -avd $(cc_quote_args "$CC_AVD") $window_flag -no-audio -no-snapshot \
      -gpu swiftshader_indirect -no-boot-anim >$(cc_quote_args "$log") 2>&1 &
    echo \$! > $(cc_quote_args "$EMULATOR_PIDFILE")
    echo \"emulator pid \$!\"
  " || return "$EXIT_EMULATOR"

  emulator_wait_for_boot
}

# Wait until Android is actually usable.
#
# Everything is inside ONE bounded loop. An earlier version opened with
# `adb wait-for-device`, which blocks indefinitely with no timeout of its own --
# so if the nohup'd emulator died at boot (no KVM, a broken AVD) no device would
# ever appear, the advertised bound would never start counting, and this would
# hang instead of returning EXIT_EMULATOR. The loop also gives up early if the
# emulator process disappears, rather than waiting out the full timeout.
#
# `adb -e` addresses the emulator specifically: per `adb help`, "-e: use TCP/IP
# device (error if multiple TCP/IP devices available)", and emulators attach over
# TCP/IP while a USB-cabled handset does not. Without it a bare `adb shell` fails
# with "more than one device/emulator" whenever a phone is plugged in, which would
# read as a boot failure. Note the limit implied by that same receipt: "TCP/IP
# device" also covers an `adb connect`ed wireless-debugging handset, so with one of
# those attached `-e` becomes ambiguous again and hits the very error it fixes for
# the USB case. Pass --device explicitly if you work that way.
emulator_wait_for_boot() {
  local timeout="${1:-300}"
  cc_info "waiting for sys.boot_completed (timeout ${timeout}s)"
  # Elapsed from $SECONDS, not by summing the sleeps: each poll also pays for an
  # adb call, and outside the dev shell that means a `nix develop` entry of a
  # second or two, so counting only the sleeps would overrun the advertised budget
  # by a wide margin and under-report how long the boot actually took.
  local start=$SECONDS
  while [ $((SECONDS - start)) -lt "$timeout" ]; do
    if ! emulator_running; then
      cc_error "the emulator process exited during boot; see ${CC_LOG_DIR#"$CC_REPO_ROOT"/}/emulator.log"
      return "$EXIT_EMULATOR"
    fi
    local booted
    booted="$(cc_capture_in_shell "$CC_ANDROID_SHELL" adb \
      "adb -e shell getprop sys.boot_completed 2>/dev/null | tr -d '\r'" 2>/dev/null || true)"
    if [ "$booted" = "1" ]; then
      cc_info "booted after $((SECONDS - start))s"
      return 0
    fi
    sleep 3
  done
  cc_error "emulator did not finish booting within ${timeout}s; see ${CC_LOG_DIR#"$CC_REPO_ROOT"/}/emulator.log"
  return "$EXIT_EMULATOR"
}

emulator_stop() {
  if ! emulator_running; then
    # Drop a pidfile left behind by an emulator that died on its own or was killed
    # externally, so a later stop cannot act on it.
    rm -f "$EMULATOR_PIDFILE"
    cc_info "no emulator running"
    return 0
  fi
  # Ask politely first so the AVD's disk image is closed cleanly, then insist.
  cc_run_in_shell "$CC_ANDROID_SHELL" adb "adb -e emu kill" >/dev/null 2>&1 || true
  local _
  for _ in $(seq 1 20); do
    emulator_running || break
    sleep 1
  done

  # Escalate by identity before resorting to the pattern: `pkill -f` would match
  # any process whose command line happens to contain the AVD name.
  if emulator_running && [ -f "$EMULATOR_PIDFILE" ]; then
    local pid
    pid="$(cat "$EMULATOR_PIDFILE" 2>/dev/null || true)"
    # `kill -0` proves a PID exists, not that it is still OUR process: PIDs get
    # recycled and this pidfile can outlive the emulator it names, so confirm the
    # process still looks like an emulator before SIGKILLing it.
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null &&
      grep -qaiE 'emulator|qemu' "/proc/$pid/cmdline" 2>/dev/null; then
      cc_warn "emulator ignored 'adb emu kill'; killing recorded pid $pid"
      kill -9 "$pid" 2>/dev/null || true
      sleep 1
    elif [ -n "$pid" ]; then
      cc_warn "recorded pid $pid is not an emulator process; ignoring the pidfile"
    fi
  fi

  if emulator_running; then
    cc_warn "still running; sending SIGKILL to processes matching '$QEMU_PATTERN'"
    pkill -9 -f "$QEMU_PATTERN" 2>/dev/null || true
    sleep 1
  fi
  if emulator_running; then
    cc_error "could not stop the AVD '$CC_AVD'. Processes still matching:"
    pgrep -af "$QEMU_PATTERN" | sed 's/^/    | /' || true
    return "$EXIT_EMULATOR"
  fi
  rm -f "$EMULATOR_PIDFILE"
  cc_info "emulator stopped"
}

emulator_status() {
  if emulator_running; then
    cc_info "emulator: running"
    cc_run_in_shell "$CC_ANDROID_SHELL" adb "adb devices" || true
  else
    cc_info "emulator: not running"
  fi
}

target_emulator() {
  case "${1:-status}" in
    start) emulator_start 0 ;;
    stop) emulator_stop ;;
    status) emulator_status ;;
    *) cc_die "$CC_EXIT_USAGE" "emulator: want start, stop or status (got '${1:-}')" ;;
  esac
}

# ---------------------------------------------------------------------------
# android
# ---------------------------------------------------------------------------

# first_adb_device [serial_prefix] -- serial of the first fully-booted device,
# empty if none. With a prefix, only serials starting with it are considered:
# `first_adb_device emulator-` picks the emulator even when a handset is also
# attached, which matters right after we booted one on the caller's behalf.
#
# cc_capture_in_shell, not cc_run_in_shell: the `.#client` shellHook prints to
# stdout, so a plain capture returns "entered claude-commander client dev
# shell …" instead of a serial.
first_adb_device() {
  # The `p == ""` arm is not redundant: `index(s, "")` returns 1 on gawk (checked)
  # but 0 on some one-true-awk descendants, which would make the no-prefix call
  # match nothing at all. Testing for the empty prefix explicitly makes the default
  # path awk-implementation independent.
  local prefix="${1:-}"
  cc_capture_in_shell "$CC_ANDROID_SHELL" adb \
    "adb devices | awk -v p=$(cc_quote_args "$prefix") \
      '\$2 == \"device\" && (p == \"\" || index(\$1, p) == 1) { print \$1; exit }'" 2>/dev/null || true
}

target_android() {
  local device="" profile=debug launch=1 windowed=0
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --device)
        [ "$#" -ge 2 ] || cc_die "$CC_EXIT_USAGE" "android: --device needs a serial"
        device="$2"
        shift 2
        ;;
      --release)
        profile=release
        shift
        ;;
      --no-launch)
        launch=0
        shift
        ;;
      --window)
        windowed=1
        shift
        ;;
      *) cc_die "$CC_EXIT_USAGE" "android: unexpected argument '$1'" ;;
    esac
  done

  # -- 1. a device to deploy to --------------------------------------------
  if [ -z "$device" ]; then
    device="$(first_adb_device)"
  fi
  if [ -z "$device" ]; then
    cc_info "no adb device attached; booting the emulator"
    emulator_start "$windowed" || return $?
    # Prefix-scoped: we booted an emulator, so deploy to that, not to a handset
    # that happened to be plugged in meanwhile.
    device="$(first_adb_device emulator-)"
    [ -n "$device" ] || cc_die "$EXIT_EMULATOR" \
      "emulator booted but no adb device appeared"
  fi
  cc_info "device: $device"

  # -- 2. build ------------------------------------------------------------
  local build_args apk
  build_args="$(cc_flutter_build_args "$profile")" || exit "$CC_EXIT_USAGE"
  apk="$(cc_apk_path "$profile")" || exit "$CC_EXIT_USAGE"

  cc_info "flutter $build_args"
  cc_run_in_shell "$CC_ANDROID_SHELL" flutter "cd client && flutter $build_args" || {
    cc_error "APK build failed"
    return "$EXIT_APK_BUILD"
  }
  [ -f "client/$apk" ] || {
    cc_error "flutter reported success but $apk is missing"
    return "$EXIT_APK_BUILD"
  }

  # -- 3. install ----------------------------------------------------------
  # Per `adb help`: "-r: replace existing application" (so app data survives) and
  # "-g: grant all runtime permissions" (so a fresh install does not stall on a
  # permission dialog).
  cc_info "adb install -r -g $apk"
  cc_run_in_shell "$CC_ANDROID_SHELL" adb \
    "adb -s $(cc_quote_args "$device") install -r -g $(cc_quote_args "client/$apk")" || {
    cc_error "install failed"
    return "$EXIT_INSTALL"
  }

  [ "$launch" -eq 1 ] || {
    cc_info "installed; skipping launch (--no-launch)"
    return 0
  }

  # -- 4. launch -----------------------------------------------------------
  local pkg
  pkg="$(cc_android_package_id "$GRADLE_KTS")" || return "$EXIT_LAUNCH"
  cc_info "launching $pkg"
  local dev_arg
  dev_arg="$(cc_quote_args "$device")"
  cc_run_in_shell "$CC_ANDROID_SHELL" adb "
    adb -s $dev_arg logcat -c
    adb -s $dev_arg shell monkey -p $(cc_quote_args "$pkg") -c android.intent.category.LAUNCHER 1
  " || {
    cc_error "launch failed"
    return "$EXIT_LAUNCH"
  }

  # monkey's status reflects intent *delivery*, not app health: its job is to
  # dispatch the LAUNCHER intent and it exits once it has, without waiting to see
  # whether the process survives. So the health check is the process itself.
  #
  # It has to be, because a log grep alone cannot tell the two apart -- and the
  # grep this replaced (any line matching `AndroidRuntime`, unscoped, over the
  # whole buffer) matched monkey's own `D AndroidRuntime: >>>>>> START ...` lines
  # and so failed every *successful* launch with EXIT_LAUNCH. Verified against a
  # Pixel 8a: a healthy app reports a pid and logs nothing at error level, and
  # `adb shell am force-stop` clears the pid.
  sleep 4
  local pid
  pid="$(cc_capture_in_shell "$CC_ANDROID_SHELL" adb \
    "adb -s $dev_arg shell pidof $(cc_quote_args "$pkg")" || true)"
  # The first pid is the process hosting the launched activity.
  pid="$(cc_first_pid "$pid")"
  if [ -z "$pid" ]; then
    cc_error "the app is not running after launch:"
    cc_capture_in_shell "$CC_ANDROID_SHELL" adb \
      "adb -s $dev_arg logcat -d 2>/dev/null | grep -E 'FATAL EXCEPTION|E AndroidRuntime|E/AndroidRuntime' | tail -20" \
      | sed 's/^/    | /'
    return "$EXIT_LAUNCH"
  fi
  # Error level only, and only from the app's own process. `--pid` is what keeps
  # another app's noise -- and monkey's -- out of the verdict.
  local errors
  errors="$(cc_capture_in_shell "$CC_ANDROID_SHELL" adb \
    "adb -s $dev_arg logcat -d --pid=$pid 2>/dev/null | grep -E 'FATAL EXCEPTION|E AndroidRuntime|E/AndroidRuntime|E flutter|E/flutter' | head -20" || true)"
  if [ -n "$errors" ]; then
    cc_error "the app logged errors on startup (pid $pid):"
    printf '%s\n' "$errors" | sed 's/^/    | /'
    return "$EXIT_LAUNCH"
  fi
  cc_info "running cleanly on $device (pid $pid)"
}

# ---------------------------------------------------------------------------
# Dispatch
# ---------------------------------------------------------------------------

[ "$#" -ge 1 ] || {
  usage
  exit "$CC_EXIT_USAGE"
}

target="$1"
shift
case "$target" in
  -h | --help)
    usage
    exit 0
    ;;
  tui) target_tui "$@" ;;
  server) target_server "$@" ;;
  linux) target_linux "$@" ;;
  android) target_android "$@" ;;
  emulator) target_emulator "$@" ;;
  *)
    cc_error "unknown target '$target'"
    usage
    exit "$CC_EXIT_USAGE"
    ;;
esac
