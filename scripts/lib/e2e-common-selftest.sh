#!/usr/bin/env bash
# Proves `e2e_init_repo`'s env scrub by running the NAIVE form first and showing
# it corrupts. Without that negative control this file would pass against a
# scrub that does nothing, which is the exact failure mode #893 shipped twice.
#
# Everything happens inside one `mktemp -d`; the real repo is never a target.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
. "$here/e2e-common.sh"

fails=0
# Takes the message, then the command to run — never `$?`, which after a `[ ]`
# reports the condition and is overwritten by the next one (SC2319).
check() {
    local msg="$1"
    shift
    if "$@"; then
        printf '  ok   %s\n' "$msg"
    else
        printf '  FAIL %s\n' "$msg" >&2
        fails=$((fails + 1))
    fi
}

root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT

# The stand-in for the developer's real repo — what a hook's exported GIT_DIR
# points at, and what must stay untouched.
real="$root/real"
mkdir -p "$real"
git init -q "$real"
printf 'keep\n' >"$real/tracked.txt"
git -C "$real" add tracked.txt
git -C "$real" -c user.email=t@t -c user.name=t commit -qm base

# What a git hook exports into every child it spawns. `git -C <dir>` does NOT
# override these. GIT_WORK_TREE is pinned too: without it an unscrubbed `add -A`
# resolves the work tree from the CWD, which makes what it stages depend on
# where the suite was invoked — and the index assertion below then passes or
# fails by accident rather than by the scrub.
export GIT_DIR="$real/.git"
export GIT_INDEX_FILE="$real/.git/index"
export GIT_WORK_TREE="$real"

staged_in_real() {
    git --git-dir="$real/.git" --work-tree="$real" \
        diff --cached --name-only 2>/dev/null | wc -l | tr -d ' '
}

# HEAD, not the index: an unscrubbed `e2e_init_repo` ends in `commit`, which
# lands the stolen files as a REAL COMMIT and leaves `diff --cached` clean
# again. Keyed on the index alone this suite passed against the mutation that
# removes the scrub — the commit is the damage, so the commit is the assertion.
head_of_real() {
    git --git-dir="$real/.git" rev-parse HEAD 2>/dev/null
}

# git's own list must relocate the repo and nothing else: stripping a transport
# or identity var would break a caller that set it deliberately.
scrub_list_is_narrow() {
    local list
    list=$(git rev-parse --local-env-vars)
    ! grep -qx 'GIT_SSH_COMMAND' <<<"$list" &&
        ! grep -qx 'GIT_COMMITTER_NAME' <<<"$list"
}

# Bait INSIDE the real work tree, staged by any unscrubbed `add -A`. Both halves
# below read the same file: the `reset` between them unstages without deleting.
# Without it each half would pass or fail on whether the real repo happened to be
# dirty rather than on the scrub — verified by mutation.
printf 'bait\n' >"$real/BAIT.txt"

# --- negative control: the naive form, which is what this helper replaced -----
naive="$root/naive-ws"
mkdir -p "$naive"
printf 'stray\n' >"$naive/NOTE.txt"
git init -q "$naive" 2>/dev/null
git -C "$naive" add -A 2>/dev/null
naive_staged=$(staged_in_real)
check "the naive form stages into the REAL repo (negative control fires)" \
    test "$naive_staged" -gt 0

# Undo whatever the naive form did, so the scrubbed run starts clean.
git --git-dir="$real/.git" --work-tree="$real" reset -q

# --- the helper under test ---------------------------------------------------
scrubbed="$root/scrubbed-ws"
mkdir -p "$scrubbed"
printf 'stray\n' >"$scrubbed/NOTE.txt"
head_before=$(head_of_real)
e2e_init_repo "$scrubbed" >/dev/null 2>&1

check "e2e_init_repo does not commit to the REAL repo" \
    test "$(head_of_real)" = "$head_before"
check "e2e_init_repo leaves the real repo's index untouched" \
    test "$(staged_in_real)" -eq 0
check "e2e_init_repo created the workspace's OWN repo" \
    test -d "$scrubbed/.git"
workspace_repo_has_a_commit() {
    git --git-dir="$scrubbed/.git" --work-tree="$scrubbed" \
        log --oneline -1 >/dev/null 2>&1
}
check "the workspace repo carries its init commit" workspace_repo_has_a_commit
check "git's own list carries no transport/identity vars to over-strip" \
    scrub_list_is_narrow

if [ "$fails" -eq 0 ]; then
    echo "e2e-common-selftest: OK"
else
    echo "e2e-common-selftest: $fails FAILED" >&2
    exit 1
fi
