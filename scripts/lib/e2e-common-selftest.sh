#!/usr/bin/env bash
# Proves the git env scrub, BOTH copies: `e2e_init_repo` and the pre-push hook.
# Each half runs an unscrubbed form first and shows it leaks — without that
# negative control this file would pass against a scrub that does nothing,
# which is the exact failure mode #893 shipped twice.
#
# Everything happens inside one `mktemp -d`; the real repo is never a target.
set -uo pipefail

# This file's fixture setup builds repos, so it must not inherit a relocating
# GIT_DIR — #893, inside the file that pins #893. `.githooks/pre-push` scrubs at
# its entry now, but this suite also runs directly, and `submodule foreach`
# exports GIT_DIR with no hook involved. It has to happen HERE, before anything
# builds a repo; the suite's own GIT_* exports come later and are unaffected.
# shellcheck disable=SC2046  # githooks(5)'s own idiom — the list must word-split
unset $(git rev-parse --local-env-vars)

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

# Both sides of the next check read `head_of_real`, so an absent fixture repo
# would compare empty to empty and pass — the exact state a leaked GIT_DIR
# produces, which is when that check most needs to fire.
check "the fixture repo has a HEAD to compare against" \
    test -n "$head_before"
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

# --- the pre-push hook's half of the same scrub ------------------------------
# Enumerated rather than named, so a hook added later fails HERE instead of
# inheriting the gap silently. pre-commit is exempt while it only runs
# `cargo fmt`; drop it from this list the moment it fans out to anything else.
hooks_exempt_from_scrub="pre-commit"

# What the hook hands `just` at its `exec`, given the GIT_DIR a LINKED
# worktree's push exports. Stubbing `just` asserts the env contract alone — no
# toolchain, no network, no push, which is why CI can run this at all.
hook_hands_just() {
    local hook="$1" s
    s=$(mktemp -d "$root/hookXXXX")
    mkdir -p "$s/bin" "$s/repo"
    # Cleared for the fixture's OWN init: this suite exports GIT_DIR at the top,
    # so a bare `git init` here re-inits THAT repo, leaves $s/repo without a
    # .git, and the hook then dies before `exec` — passing the check vacuously.
    # shellcheck disable=SC2046  # the var list must word-split
    (unset $(git rev-parse --local-env-vars) && git init -q "$s/repo")
    # Reports EVERY local-env-var still set, not just GIT_DIR: a scrub narrowed
    # to GIT_DIR alone leaves GIT_INDEX_FILE relocating the index, which is half
    # of what outranks `git -C <dir>` in #893, and would otherwise pass here.
    # shellcheck disable=SC2016  # the stub's own script text, not expansion
    printf '#!/usr/bin/env bash\nfor v in $(git rev-parse --local-env-vars); do [ -n "${!v:-}" ] && printf "%%s " "$v"; done >"%s/seen"\nexit 0\n' "$s" >"$s/bin/just"
    chmod +x "$s/bin/just"
    (cd "$s/repo" && env PATH="$s/bin:$PATH" GIT_DIR="$s/repo/.git" \
        GIT_INDEX_FILE="$s/repo/.git/index" GIT_WORK_TREE="$s/repo" \
        bash "$hook" origin y </dev/null) >/dev/null 2>&1
    cat "$s/seen" 2>/dev/null
}

# Written out, never derived from a real hook: a control cut with `grep -v`
# orphans the guard's `if` body, so the mutant exits before `just` and the
# control silently reports "no leak" while testing nothing.
printf '#!/usr/bin/env bash\nexec just preflight\n' >"$root/unscrubbed-hook"
check "the harness actually poisons a hook's env (control fires)" \
    test -n "$(hook_hands_just "$root/unscrubbed-hook")"

for hook in "$(e2e_repo_root)"/.githooks/*; do
    name=$(basename "$hook")
    case " $hooks_exempt_from_scrub " in *" $name "*) continue ;; esac
    check "$name leaks no repo-relocating git env to preflight" \
        test -z "$(hook_hands_just "$hook")"
done

if [ "$fails" -eq 0 ]; then
    echo "e2e-common-selftest: OK"
else
    echo "e2e-common-selftest: $fails FAILED" >&2
    exit 1
fi
