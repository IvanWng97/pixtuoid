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

# A throwaway git repo at $1, for the CLIs that refuse to act outside one.
#
# The scrub is the whole point. A git hook exports GIT_DIR and GIT_INDEX_FILE
# into every child, and those OUTRANK `git -C <dir>` — so run from a hook, a
# bare `git -C "$WS" add -A` stages the REAL repo while printing nothing (#893,
# which landed twice). `just live-sources` is manual today, so nothing reaches
# this from a hook; the scrub is what keeps that true for the next caller.
#
# The list comes from `git rev-parse --local-env-vars`, git's own answer, not a
# GIT_* prefix match — that would also strip GIT_SSH_COMMAND and the
# GIT_COMMITTER_* identity, neither of which relocates a repo.
e2e_init_repo() {
    local ws="$1" scrub
    scrub=$(git rev-parse --local-env-vars | sed 's/^/-u /' | tr '\n' ' ')
    # An empty list is indistinguishable from "nothing to scrub", so the git calls
    # below would silently inherit the caller's GIT_DIR — the bug this guards.
    if [ -z "$scrub" ]; then
        printf 'e2e_init_repo: git rev-parse --local-env-vars gave nothing\n' >&2
        return 1
    fi
    # shellcheck disable=SC2086  # $scrub is a flag LIST and must word-split
    env $scrub git init -q "$ws"
    # shellcheck disable=SC2086
    env $scrub git -C "$ws" add -A
    # shellcheck disable=SC2086
    env $scrub git -C "$ws" -c user.email=e2e@pixtuoid -c user.name=e2e commit -qm init
}
