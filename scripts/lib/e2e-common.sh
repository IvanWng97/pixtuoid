#!/usr/bin/env bash
# Sourced by every scripts/lib/tier-*.sh. Holds ONLY what all four duplicated
# verbatim. Cleanup bodies and the expect_line pollers stay per-tier by design —
# the WHY for the pollers is at tier-openclaw-hermetic.sh's expect_line.

e2e_repo_root() {
    (cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
}

e2e_require_bin() {
    local bin
    for bin in "$@"; do
        [ -x "$bin" ] || {
            echo "missing $bin — run: just build --release" >&2
            exit 2
        }
    done
}

e2e_sandbox() {
    mktemp -d
}
