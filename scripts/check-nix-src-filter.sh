#!/usr/bin/env bash
# Assert that flake.nix's `src` filter admits only what the claude-commander
# binary is actually built from.
#
# `src` is the sole source input to the package derivation: Nix hashes the
# filtered tree and that hash lands in the .drv, so *any* admitted file forces a
# full (fat-LTO) rebuild when it changes — whether or not cargo ever opens it.
# Docs and the Flutter client under client/ are not part of this binary (root
# Cargo.toml `exclude`s client/, and no workspace crate path-depends on it), so
# they must not be admitted. The client is built and tested by CI's separate
# `client` job, which works from the checked-out worktree and never consults
# `src`.
#
# Both directions are checked, because a filter that admitted nothing at all
# would pass the first half trivially:
#   - probes outside the build inputs must NOT change the src hash
#   - a probe inside crates/ MUST change it
#
# Usage: ./scripts/check-nix-src-filter.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

# Nix flakes ignore untracked files in a git repo, so a probe is invisible
# unless it is at least intent-to-add (`git add -N`). That also means this
# script must put the index back exactly as it found it.
EXCLUDED_PROBES=(
    "docs/_filter_probe.md"
    "client/rust/src/_filter_probe.rs"
    "README_filter_probe.md"
    # A brand-new top-level directory: the filter must prune it whole. Keeping
    # the directory but dropping its files still changes the hash, because an
    # empty directory is part of the tree.
    "_filter_probe_dir/probe.rs"
)
INCLUDED_PROBE="crates/claude-commander-core/src/_filter_probe.md"

# Only ever clean up probes this run actually created. The trap must not be able
# to delete a pre-existing file — that is the very thing the guard below refuses
# to touch.
CREATED=()

add_probe() {
    mkdir -p "$(dirname "$1")"
    echo "transient probe written by scripts/check-nix-src-filter.sh" > "$1"
    CREATED+=("$1")
    git add -N "$1"
}

remove_probe() {
    git reset -q -- "$1" 2>/dev/null || true
    rm -f "$1"
    # Plain `rmdir` (no GNU-only flags): removes a directory the probe created,
    # and fails harmlessly on a pre-existing, still-populated one.
    rmdir "$(dirname "$1")" 2>/dev/null || true
}

drop_probe() {
    remove_probe "$1"
    local remaining=()
    local created
    for created in ${CREATED[@]+"${CREATED[@]}"}; do
        [ "${created}" = "$1" ] || remaining+=("${created}")
    done
    CREATED=(${remaining[@]+"${remaining[@]}"})
}

# shellcheck disable=SC2329  # invoked indirectly, by `trap cleanup EXIT` below
cleanup() {
    local probe
    for probe in ${CREATED[@]+"${CREATED[@]}"}; do
        remove_probe "${probe}"
    done
}

for probe in "${EXCLUDED_PROBES[@]}" "${INCLUDED_PROBE}"; do
    if [ -e "${probe}" ]; then
        echo "error: probe path ${probe} already exists; refusing to clobber it" >&2
        exit 1
    fi
done

# Installed only once nothing can be clobbered, so an early exit above can never
# delete a real file.
trap cleanup EXIT

src_hash() {
    nix eval --raw .#default.src
}

status=0
baseline="$(src_hash)"
echo "==> baseline src: ${baseline}"

# All excluded probes in one evaluation: each `nix eval` re-evaluates the flake
# (the probe changes the tree, so the eval cache misses), and this step runs on
# every Nix Build. The per-probe loop below is only paid on failure, where
# knowing which path leaked is worth the extra evaluations.
for probe in "${EXCLUDED_PROBES[@]}"; do add_probe "${probe}"; done
batch="$(src_hash)"
for probe in "${EXCLUDED_PROBES[@]}"; do drop_probe "${probe}"; done

if [ "${batch}" = "${baseline}" ]; then
    echo "ok   ${#EXCLUDED_PROBES[@]} paths outside the build inputs left the src hash alone"
else
    status=1
    echo "FAIL a path outside the build inputs changed the src hash" >&2
    echo "     Admitting it forces a needless full rebuild. Narrowing down:" >&2
    for probe in "${EXCLUDED_PROBES[@]}"; do
        add_probe "${probe}"
        actual="$(src_hash)"
        drop_probe "${probe}"
        if [ "${actual}" = "${baseline}" ]; then
            echo "     ok      ${probe}" >&2
        else
            echo "     LEAKED  ${probe}" >&2
        fi
    done
    echo "     Narrow the filter in flake.nix." >&2
fi

add_probe "${INCLUDED_PROBE}"
included="$(src_hash)"
drop_probe "${INCLUDED_PROBE}"
if [ "${included}" != "${baseline}" ]; then
    echo "ok   ${INCLUDED_PROBE} is inside the build inputs"
else
    status=1
    echo "FAIL ${INCLUDED_PROBE} did NOT change the src hash" >&2
    echo "     crates/**/*.md must stay admitted — core's commander_prime.md is" >&2
    echo "     include_str!'d into the binary. A filter that admits nothing would" >&2
    echo "     otherwise pass the checks above vacuously." >&2
fi

exit "${status}"
