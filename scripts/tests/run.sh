#!/usr/bin/env bash
#
# Self-tests for the pure helpers in scripts/lib/dev-common.sh.
#
# Plain bash assertions, no framework and no new dependencies. Only the pure
# helpers are covered: the ones that map names to names (crate aliases, lane
# exit codes, tier -> lane lists) and the ones that read a value out of a file
# (the Android applicationId, the APK path). The effectful parts of verify.sh /
# dev-run.sh -- entering a nix shell, booting an emulator, running cargo -- are
# not tested here; they need the real toolchain and a device.
#
# Fixtures are created under `mktemp -d`; nothing outside it is read or written.
#
#   scripts/tests/run.sh          run all assertions
#   scripts/tests/run.sh -v       also print each passing assertion
#
# Exit: 0 all passed, 1 one or more failed.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source-path=SCRIPTDIR
# shellcheck source=../lib/dev-common.sh
source "$REPO_ROOT/scripts/lib/dev-common.sh"

VERBOSE=0
[ "${1:-}" = "-v" ] && VERBOSE=1

pass_count=0
fail_count=0

# assert_eq <expected> <actual> <description>
assert_eq() {
  local expected="$1" actual="$2" desc="$3"
  if [ "$expected" = "$actual" ]; then
    pass_count=$((pass_count + 1))
    [ "$VERBOSE" -eq 1 ] && printf '  ok   %s\n' "$desc"
    return 0
  fi
  fail_count=$((fail_count + 1))
  printf '  FAIL %s\n' "$desc"
  printf '         expected: %s\n' "$expected"
  printf '         actual:   %s\n' "$actual"
  return 0
}

# assert_fails <description> <command...> -- asserts a non-zero exit status.
assert_fails() {
  local desc="$1"
  shift
  if "$@" >/dev/null 2>&1; then
    fail_count=$((fail_count + 1))
    printf '  FAIL %s\n' "$desc"
    printf '         expected non-zero exit, got 0\n'
  else
    pass_count=$((pass_count + 1))
    [ "$VERBOSE" -eq 1 ] && printf '  ok   %s\n' "$desc"
  fi
  return 0
}

echo "== cc_crate_for_alias =="
assert_eq "claude-commander-core" "$(cc_crate_for_alias core)" "core -> claude-commander-core"
assert_eq "claude-commander-tui" "$(cc_crate_for_alias tui)" "tui -> claude-commander-tui"
assert_eq "claude-commander-viewmodel" "$(cc_crate_for_alias viewmodel)" "viewmodel -> claude-commander-viewmodel"
assert_eq "claude-commander-viewmodel" "$(cc_crate_for_alias vm)" "vm -> claude-commander-viewmodel"
assert_eq "claude-commander" "$(cc_crate_for_alias cli)" "cli -> claude-commander"
assert_eq "claude-commander" "$(cc_crate_for_alias commander)" "commander -> claude-commander"
assert_eq "claude-commander-server" "$(cc_crate_for_alias server)" "server -> claude-commander-server"
assert_eq "claude-commander-protocol" "$(cc_crate_for_alias protocol)" "protocol -> claude-commander-protocol"
assert_eq "claude-commander-remote" "$(cc_crate_for_alias remote)" "remote -> claude-commander-remote"
assert_eq "claude-commander-client" "$(cc_crate_for_alias client)" "client -> claude-commander-client"
assert_eq "claude-commander-test-support" "$(cc_crate_for_alias test-support)" \
  "test-support -> claude-commander-test-support"
# A fully-spelled crate name must pass through untouched, so the shorthand is
# additive rather than the only accepted spelling.
assert_eq "claude-commander-core" "$(cc_crate_for_alias claude-commander-core)" \
  "full crate name passes through"
assert_fails "unknown alias is rejected" cc_crate_for_alias notacrate

echo "== cc_exit_code_for_lane =="
assert_eq "10" "$(cc_exit_code_for_lane fmt)" "fmt -> 10"
assert_eq "11" "$(cc_exit_code_for_lane clippy)" "clippy -> 11"
assert_eq "12" "$(cc_exit_code_for_lane build)" "build -> 12"
assert_eq "13" "$(cc_exit_code_for_lane test)" "test -> 13"
assert_eq "20" "$(cc_exit_code_for_lane pub-get)" "pub-get -> 20"
assert_eq "21" "$(cc_exit_code_for_lane dart-format)" "dart-format -> 21"
assert_eq "22" "$(cc_exit_code_for_lane analyze)" "analyze -> 22"
assert_eq "23" "$(cc_exit_code_for_lane flutter-test)" "flutter-test -> 23"
assert_eq "24" "$(cc_exit_code_for_lane cdylib)" "cdylib -> 24"
assert_eq "25" "$(cc_exit_code_for_lane e2e)" "e2e -> 25"
assert_eq "26" "$(cc_exit_code_for_lane goldens)" "goldens -> 26"
assert_eq "30" "$(cc_exit_code_for_lane nix-build)" "nix-build -> 30"
assert_eq "31" "$(cc_exit_code_for_lane packaging)" "packaging -> 31"
assert_eq "32" "$(cc_exit_code_for_lane shellcheck)" "shellcheck -> 32"
assert_eq "33" "$(cc_exit_code_for_lane selftest)" "selftest -> 33"
assert_eq "34" "$(cc_exit_code_for_lane nix-src-filter)" "nix-src-filter -> 34"
assert_fails "unknown lane is rejected" cc_exit_code_for_lane notalane

# Every lane must own a distinct code: a duplicate would make the exit status
# ambiguous, which is the whole point of the table.
echo "== exit codes are unique =="
all_codes=""
lane_count=0
# Every lane in any tier, not just `all`: the goldens lane deliberately sits
# outside `all` (flutter-test already covers it), and a lane that no tier sweep
# reaches would otherwise be free to duplicate someone else's code.
every_lane=$(cc_lane_run_order | tr ' ' '\n' | grep -v '^$')
for lane in $every_lane; do
  all_codes+="$(cc_exit_code_for_lane "$lane")"$'\n'
  lane_count=$((lane_count + 1))
done
uniq_count=$(printf '%s' "$all_codes" | sort -u | grep -c .)
# Derived from the tier list rather than a hand-kept lane list, so adding a lane
# without giving it a distinct code fails here instead of passing quietly.
assert_eq "$lane_count" "$uniq_count" "every lane in every tier has a distinct exit code"
# Counted from the `all` tier alone, not the widened union above: this pins what
# CI's mirror covers, which is a different question from code uniqueness.
all_tier_count=$(cc_lanes_for_tier all | tr ' ' '\n' | grep -c .)
assert_eq "15" "$all_tier_count" "the all tier covers 15 lanes"
assert_eq "16" "$lane_count" "the run order covers 16 lanes (all + goldens)"

# The runner walks cc_lane_run_order, so a lane missing from it can never execute
# however it is selected -- which is exactly how the goldens lane was first
# silently skipped. Assert containment rather than trusting the two lists to agree.
missing_from_run_order=""
for lane in $(cc_lanes_for_tier all) $(cc_lanes_for_tier goldens); do
  case " $(cc_lane_run_order) " in
    *" $lane "*) : ;;
    *) missing_from_run_order+="$lane " ;;
  esac
done
assert_eq "" "$missing_from_run_order" \
  "every lane a tier can select appears in cc_lane_run_order"

# No lane may collide with the reserved argument/precondition codes, or with the
# shell's own conventional statuses.
echo "== reserved codes are not reused by lanes =="
for reserved in 0 1 2 3 77; do
  if printf '%s' "$all_codes" | grep -qx "$reserved"; then
    fail_count=$((fail_count + 1))
    printf '  FAIL a lane claims reserved exit code %s\n' "$reserved"
  else
    pass_count=$((pass_count + 1))
    [ "$VERBOSE" -eq 1 ] && printf '  ok   reserved code %s unused by lanes\n' "$reserved"
  fi
done

echo "== cc_lanes_for_tier =="
assert_eq "fmt clippy" "$(cc_lanes_for_tier fast)" "fast tier is fmt + clippy only"
assert_eq "fmt clippy build test" "$(cc_lanes_for_tier rust)" "rust tier is the four cargo lanes"
assert_eq "pub-get dart-format analyze flutter-test cdylib" "$(cc_lanes_for_tier client)" \
  "client tier resolves packages before checking them"

assert_eq "pub-get goldens" "$(cc_lanes_for_tier goldens)" \
  "goldens tier resolves packages, then rasterises only test/goldens"
assert_eq "fmt clippy build test pub-get dart-format analyze flutter-test cdylib shellcheck selftest nix-src-filter e2e nix-build packaging" \
  "$(cc_lanes_for_tier all)" "all tier is every lane, cheap-to-slow"

# pub-get is not a check but a precondition: `dart format` reads each file's
# language version from the nearest package_config.json, so a stale one makes
# dart-format rewrite vendored cargokit files. It must therefore precede every
# lane that runs a Dart or Flutter tool.
for tier in client all; do
  lanes_str="$(cc_lanes_for_tier "$tier")"
  case " $lanes_str " in
    *" pub-get "*)
      pub_pos=0 first_dart_pos=0 pos=0
      for lane in $lanes_str; do
        pos=$((pos + 1))
        [ "$lane" = "pub-get" ] && pub_pos=$pos
        case "$lane" in
          dart-format | analyze | flutter-test | e2e)
            [ "$first_dart_pos" -eq 0 ] && first_dart_pos=$pos
            ;;
        esac
      done
      if [ "$pub_pos" -gt 0 ] && [ "$pub_pos" -lt "$first_dart_pos" ]; then
        pass_count=$((pass_count + 1))
        [ "$VERBOSE" -eq 1 ] && printf '  ok   %s tier runs pub-get before any dart tool\n' "$tier"
      else
        fail_count=$((fail_count + 1))
        printf '  FAIL %s tier must run pub-get before any dart tool\n' "$tier"
      fi
      ;;
  esac
done
assert_fails "unknown tier is rejected" cc_lanes_for_tier notatier

# The rust lanes must lead the `all` list: a fmt or clippy failure is seconds
# away, and finding it after a multi-minute `nix build` wastes the run.
echo "== all tier is ordered cheapest-first =="
all_lanes="$(cc_lanes_for_tier all)"
assert_eq "fmt" "${all_lanes%% *}" "all tier starts with fmt"
case "$all_lanes" in
  *"nix-build packaging"*) nix_late=yes ;;
  *) nix_late=no ;;
esac
assert_eq "yes" "$nix_late" "the slow nix lanes sit late in the all tier"

# The `.#client` dev shell's shellHook echoes "entered claude-commander client
# dev shell …" (and, on first use, AVD-creation notices) to STDOUT, ahead of the
# command's own output -- verified by `nix develop .#client -c bash -c 'echo X'`
# printing the hook line first. Anything capturing a value through that shell has
# to drop the preamble, which is what all the hand-written
# `grep -vE "entered claude|Creating Android"` filters in this repo's history
# were doing.
echo "== cc_strip_shell_preamble =="
assert_eq "emulator-5554" \
  "$(cc_strip_shell_preamble "entered claude-commander client dev shell (flutter + rust + android ndk)
$CC_OUTPUT_SENTINEL
emulator-5554")" \
  "the shellHook preamble is dropped"
assert_eq "emulator-5554" \
  "$(cc_strip_shell_preamble "$CC_OUTPUT_SENTINEL
emulator-5554")" \
  "no preamble at all still yields the payload"
assert_eq "" "$(cc_strip_shell_preamble "Creating Android emulator AVD 'cctest'...
AVD 'cctest' created.
$CC_OUTPUT_SENTINEL")" \
  "an empty payload stays empty"
assert_eq "line one
line two" \
  "$(cc_strip_shell_preamble "noise
$CC_OUTPUT_SENTINEL
line one
line two")" \
  "a multi-line payload is preserved"
# Absent sentinel means the wrapper was bypassed; pass the text through rather
# than silently returning nothing.
assert_eq "raw text" "$(cc_strip_shell_preamble "raw text")" \
  "text with no sentinel passes through unchanged"
# Only a whole line counts as the marker, so a payload mentioning it in passing
# does not truncate the real output.
assert_eq "mentions $CC_OUTPUT_SENTINEL inline" \
  "$(cc_strip_shell_preamble "$CC_OUTPUT_SENTINEL
mentions $CC_OUTPUT_SENTINEL inline")" \
  "the marker must be its own line"

echo "== cc_android_package_id =="
fixture_dir="$(mktemp -d)"
# shellcheck disable=SC2064  # expand $fixture_dir now, while it is still set
trap "rm -rf '$fixture_dir'" EXIT

mkdir -p "$fixture_dir/android/app"
cat >"$fixture_dir/android/app/build.gradle.kts" <<'GRADLE'
plugins {
    id("com.android.application")
}

android {
    namespace = "com.claudecommander.claude_commander_client"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.claudecommander.claude_commander_client"
        minSdk = 24
    }
}
GRADLE
assert_eq "com.claudecommander.claude_commander_client" \
  "$(cc_android_package_id "$fixture_dir/android/app/build.gradle.kts")" \
  "applicationId is read out of build.gradle.kts"

# `namespace` is declared before `applicationId` and both are valid-looking
# dotted ids, so a naive grep for the first dotted string returns the wrong one
# when they ever diverge. Pin that applicationId wins.
cat >"$fixture_dir/diverged.gradle.kts" <<'GRADLE'
android {
    namespace = "com.example.old_namespace"
    defaultConfig {
        applicationId = "com.claudecommander.renamed"
    }
}
GRADLE
assert_eq "com.claudecommander.renamed" \
  "$(cc_android_package_id "$fixture_dir/diverged.gradle.kts")" \
  "applicationId wins over an earlier, different namespace"

assert_fails "a gradle file with no applicationId is rejected" \
  cc_android_package_id "$fixture_dir/android/app/does-not-exist.kts"

cat >"$fixture_dir/empty.gradle.kts" <<'GRADLE'
android {
    compileSdk = 36
}
GRADLE
assert_fails "a gradle file that declares no applicationId is rejected" \
  cc_android_package_id "$fixture_dir/empty.gradle.kts"

# Both scripts hand their commands to `bash -c` (that is how a nix dev shell is
# re-entered), so every caller-supplied value crosses a shell boundary. Without
# quoting, `verify.sh -p core "two words"` would arrive as two arguments.
echo "== cc_quote_args =="
assert_eq "" "$(cc_quote_args)" "no arguments quote to nothing"
assert_eq "plain" "$(cc_quote_args plain)" "a plain word is unchanged"
assert_eq "one two" "$(cc_quote_args one two)" "separate words stay separate"
assert_eq 'two\ words' "$(cc_quote_args "two words")" "a value with a space is quoted"
assert_eq "--debug" "$(cc_quote_args --debug)" "a flag is unchanged"
# Round-trip through the same `bash -c` path the scripts use: the quoted form
# must reproduce the original argument vector exactly.
assert_eq "3" \
  "$(bash -c "printf '%s\n' $(cc_quote_args a "b c" d)" | wc -l)" \
  "quoted args round-trip through bash -c as 3 arguments"
assert_eq "b c" \
  "$(bash -c "printf '%s\n' $(cc_quote_args a "b c" d)" | sed -n 2p)" \
  "the spaced argument survives the round trip intact"
# A value containing shell metacharacters must not be interpreted.
assert_eq 'rm -rf /' \
  "$(bash -c "printf '%s\n' $(cc_quote_args 'rm -rf /')")" \
  "shell metacharacters are inert after quoting"

echo "== cc_apk_path =="
assert_eq "build/app/outputs/flutter-apk/app-debug.apk" "$(cc_apk_path debug)" \
  "debug APK path"
assert_eq "build/app/outputs/flutter-apk/app-release.apk" "$(cc_apk_path release)" \
  "release APK path"
assert_fails "unknown build profile is rejected" cc_apk_path profile

echo "== cc_flutter_build_args =="
assert_eq "build apk --debug" "$(cc_flutter_build_args debug)" "debug build args"
assert_eq "build apk --release" "$(cc_flutter_build_args release)" "release build args"

echo "== cc_format_duration =="
assert_eq "0s" "$(cc_format_duration 0)" "zero seconds"
assert_eq "59s" "$(cc_format_duration 59)" "just under a minute"
assert_eq "1m00s" "$(cc_format_duration 60)" "exactly a minute zero-pads"
assert_eq "3m16s" "$(cc_format_duration 196)" "minutes and seconds"
assert_eq "60m01s" "$(cc_format_duration 3601)" "over an hour stays in minutes"

# ---------------------------------------------------------------------------
# The lane runner. This is the headline contract -- no fail-fast, exit with the
# FIRST failing lane's code, SKIP semantics -- and none of it is exercised by the
# name-mapping assertions above. Driven with stub lane functions and a temp log
# dir so no real toolchain runs. A plausible regression this catches: "fixing"
# cc_lane to propagate its lane's status, which under `set -e` would abort the
# sweep at the first failure.
# ---------------------------------------------------------------------------
echo "== cc_lane / cc_print_summary =="

CC_LOG_DIR="$fixture_dir/logs"

reset_lanes() {
  CC_LANE_NAMES=()
  CC_LANE_STATUS=()
  CC_LANE_SECS=()
  CC_LANE_NOTES=()
}

stub_pass() { echo "stub output: passing"; }
stub_fail() {
  echo "stub output: failing"
  return 1
}
stub_skip_with_reason() {
  CC_SKIP_REASON="nothing to do here"
  return "$CC_LANE_SKIPPED"
}
# A lane whose underlying TOOL happens to exit 77 (automake's own skip
# convention) and never set a reason. That must count as a failure, not a
# silently-tolerated skip.
stub_exit_77_no_reason() { return 77; }

reset_lanes
cc_lane fmt stub_pass >/dev/null 2>&1
assert_eq "PASS" "${CC_LANE_STATUS[0]}" "a passing lane records PASS"

reset_lanes
cc_lane clippy stub_fail >/dev/null 2>&1
lane_rc=$?
assert_eq "FAIL" "${CC_LANE_STATUS[0]}" "a failing lane records FAIL"
# The sweep must continue: cc_lane itself always succeeds, or `set -e` in the
# caller would abort at the first failing lane.
assert_eq "0" "$lane_rc" "cc_lane returns 0 even when its lane failed"

reset_lanes
cc_lane test stub_skip_with_reason >/dev/null 2>&1
assert_eq "SKIP" "${CC_LANE_STATUS[0]}" "sentinel + reason records SKIP"
assert_eq "nothing to do here" "${CC_LANE_NOTES[0]}" "the skip reason is kept for the summary"

reset_lanes
cc_lane test stub_exit_77_no_reason >/dev/null 2>&1
assert_eq "FAIL" "${CC_LANE_STATUS[0]}" \
  "exit 77 with no reason set is a FAILURE, not a skip"

# Exit status: the first failing lane's reserved code, not the last.
reset_lanes
cc_lane fmt stub_pass >/dev/null 2>&1
cc_lane clippy stub_fail >/dev/null 2>&1
cc_lane test stub_fail >/dev/null 2>&1
summary_rc=0
cc_print_summary >/dev/null 2>&1 || summary_rc=$?
assert_eq "11" "$summary_rc" "summary exits with the FIRST failing lane's code (clippy=11, not test=13)"

reset_lanes
cc_lane fmt stub_pass >/dev/null 2>&1
cc_lane clippy stub_pass >/dev/null 2>&1
summary_rc=0
cc_print_summary >/dev/null 2>&1 || summary_rc=$?
assert_eq "0" "$summary_rc" "all lanes passing exits 0"

# A skip must not fail the run.
reset_lanes
cc_lane fmt stub_pass >/dev/null 2>&1
cc_lane test stub_skip_with_reason >/dev/null 2>&1
summary_rc=0
cc_print_summary >/dev/null 2>&1 || summary_rc=$?
assert_eq "0" "$summary_rc" "a skipped lane does not fail the run"

# Output is captured to the lane's log rather than lost.
reset_lanes
cc_lane fmt stub_pass >/dev/null 2>&1
assert_eq "stub output: passing" "$(cat "$CC_LOG_DIR/fmt.log")" \
  "the lane's output lands in target/verify-logs/<lane>.log"

# On failure the tail is echoed so the reason is visible without opening the log.
reset_lanes
lane_out="$(cc_lane clippy stub_fail 2>&1)"
case "$lane_out" in
  *"stub output: failing"*) tail_shown=yes ;;
  *) tail_shown=no ;;
esac
assert_eq "yes" "$tail_shown" "a failing lane prints its log tail"

echo
printf '%s passed, %s failed\n' "$pass_count" "$fail_count"
[ "$fail_count" -eq 0 ] || exit 1
