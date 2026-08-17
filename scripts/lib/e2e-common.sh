#!/usr/bin/env bash
# Sourced by every scripts/lib/tier-*.sh. Holds what the tiers duplicated —
# tier-replay keeps its own binary guard, whose message carries the PIXTUOID_BIN
# override. Cleanup bodies and the expect_line pollers stay per-tier by design;
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

# 0700 by construction, and the tiers put their SOCKET inside it: a fixed or
# `mktemp -u` socket name in shared temp lets concurrent runs clobber each other
# and is pre-plantable by another user, which nothing downstream polices —
# `ensure_owned_socket_dir` leaves an explicit PIXTUOID_SOCKET path alone.
e2e_sandbox() {
    mktemp -d
}
