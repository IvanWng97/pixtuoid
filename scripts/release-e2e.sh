#!/usr/bin/env bash
# THE end-to-end entry point. `just release-e2e X.Y.Z` walks the pre-tag phase;
# --post walks the post-publish one. The openclaw and replay recipes are `--only`
# views of the same steps, so the release machinery is exercised mid-cycle.
#
# Three verdicts, not two. An auto body exiting 2 means "could not run here"
# (a missing external prerequisite) and records BLOCKED; any other non-zero is a
# real assertion failure. Conflating the two is what makes a checklist either
# unfinishable or fail-open.
#
# Targets macOS's bash 3.2: "${arr[@]:-}" on an EMPTY array expands to one empty
# string there, so forwarded args are always length-guarded before expansion.
set -uo pipefail

LIB="$(cd "$(dirname "${BASH_SOURCE[0]}")/lib" && pwd)"
REPO="$(cd "$LIB/../.." && pwd)"
export LIB REPO
# shellcheck source=/dev/null
. "$LIB/checklist.sh"

PHASE=pre
ONLY=""
FROM=""
LIST=0
VERSION=""
TIER_ARGS=()
while [ $# -gt 0 ]; do
    case "$1" in
    --post) PHASE=post ;;
    --list) LIST=1 ;;
    --only)
        ONLY="${2:?--only needs a step id}"
        shift
        ;;
    --from)
        FROM="${2:?--from needs a step id}"
        shift
        ;;
    --)
        shift
        TIER_ARGS=("$@")
        break
        ;;
    -*)
        echo "unknown flag: $1" >&2
        exit 2
        ;;
    *) VERSION="$1" ;;
    esac
    shift
done
# Cargo.toml is the authority; typing the version is an OPTIONAL assertion, not a
# second copy of a value the tree already owns.
TREE_VERSION="$(grep -m1 '^version' "$REPO/Cargo.toml" | cut -d'"' -f2)"
if [ -z "$VERSION" ]; then
    VERSION="$TREE_VERSION"
elif [ "$VERSION" != "$TREE_VERSION" ]; then
    printf 'you asked for %s but Cargo.toml says %s — bump first, or drop the argument\n' \
        "$VERSION" "$TREE_VERSION" >&2
    exit 2
fi
export VERSION

step_field() {
    local want="$1" n="$2" s
    for s in "${STEPS[@]}"; do
        if [ "${s%%|*}" = "$want" ]; then
            printf '%s\n' "$s" | cut -d'|' -f"$n"
            return 0
        fi
    done
    return 1
}

call_body() {
    if [ "${#TIER_ARGS[@]}" -gt 0 ]; then
        "$1" "${TIER_ARGS[@]}"
    else
        "$1"
    fi
}

record() { printf '%s|%s|%s|%s\n' "$1" "$2" "$3" "${4:-}" >>"$STATE"; }
# `cut -d'|' -f1` rather than a `^id|` grep: `|` is literal to grep BRE but the
# id would still prefix-match a longer sibling id.
last_row() {
    local id="$1" row="" line
    while IFS= read -r line; do
        [ "$(printf '%s' "$line" | cut -d'|' -f1)" = "$id" ] && row="$line"
    done <"$STATE"
    printf '%s' "$row"
}
field_of() { printf '%s' "$1" | cut -d'|' -f"$2"; }

run_step() {
    local id="$1" kind="$2" title="$3" rc ans reason
    printf '\n\033[1m▸ %s\033[0m  %s\n' "$id" "$title"
    if [ "$kind" = auto ]; then
        : >"$BLOCK_REASON"
        (call_body "auto_$id")
        rc=$?
        if [ "$rc" -eq 0 ]; then
            record "$id" pass "$HEAD_SHA"
            printf '  \033[32m✓ pass\033[0m\n'
            return 0
        fi
        if [ "$rc" -eq 2 ]; then
            reason="$(tr -d '\n' <"$BLOCK_REASON")"
            [ -n "$reason" ] || reason="prerequisite missing on this host"
            record "$id" blocked "$HEAD_SHA" "$reason"
            printf '  \033[33m⊘ blocked\033[0m — %s\n' "$reason"
            return 0
        fi
        record "$id" fail "$HEAD_SHA"
        printf '  \033[31m✗ fail (exit %d)\033[0m\n' "$rc"
        return 1
    fi
    # A manual step under a non-TTY must never auto-pass: an unattended run would
    # otherwise report a verdict nobody gave.
    if [ ! -t 0 ]; then
        echo "  manual step needs a TTY — refusing to auto-pass" >&2
        return 1
    fi
    (call_body "manual_$id")
    while :; do
        printf '  [y] pass  [n] fail  [b] blocked  [q] quit > '
        if ! read -r ans; then
            echo
            echo "  stdin closed — treating as quit; state kept at $STATE"
            exit 130
        fi
        case "$ans" in
        y)
            record "$id" pass "$HEAD_SHA"
            return 0
            ;;
        n)
            record "$id" fail "$HEAD_SHA"
            return 1
            ;;
        b)
            printf '  reason (required): '
            read -r reason || reason=""
            if [ -z "$reason" ]; then
                echo "  blocked needs a reason"
                continue
            fi
            record "$id" blocked "$HEAD_SHA" "$reason"
            return 0
            ;;
        q)
            echo "  stopped — state kept at $STATE"
            exit 130
            ;;
        esac
    done
}

if [ "$LIST" -eq 1 ]; then
    for s in "${STEPS[@]}"; do
        IFS='|' read -r id phase kind title <<<"$s"
        [ "$phase" = "$PHASE" ] || continue
        printf '%-20s %-7s %s\n' "$id" "$kind" "$title"
    done
    exit 0
fi

BLOCK_REASON="$(mktemp)"
export E2E_BLOCK_REASON="$BLOCK_REASON"
trap 'rm -f "$BLOCK_REASON"' EXIT

if [ -n "$ONLY" ]; then
    if ! kind="$(step_field "$ONLY" 3)"; then
        echo "unknown step: $ONLY" >&2
        exit 2
    fi
    STATE=/dev/null
    HEAD_SHA="$(git -C "$REPO" rev-parse HEAD 2>/dev/null || echo unknown)"
    run_step "$ONLY" "$kind" "$(step_field "$ONLY" 4)"
    exit $?
fi

printf 'version %s (from Cargo.toml)\n' "$VERSION"
if [ -n "$FROM" ] && ! step_field "$FROM" 1 >/dev/null; then
    echo "--from: unknown step id '$FROM'" >&2
    exit 2
fi

HEAD_SHA="$(git -C "$REPO" rev-parse HEAD)"
STATEDIR="${XDG_STATE_HOME:-$HOME/.local/state}/pixtuoid/release-e2e"
mkdir -p "$STATEDIR" || {
    echo "cannot create $STATEDIR" >&2
    exit 2
}
STATE="$STATEDIR/$VERSION-$PHASE"
: >>"$STATE"
if [ -n "$FROM" ]; then
    grep -v "^$FROM|" "$STATE" >"$STATE.tmp" && mv "$STATE.tmp" "$STATE"
fi

FAILED=0
for s in "${STEPS[@]}"; do
    IFS='|' read -r id phase kind title <<<"$s"
    [ "$phase" = "$PHASE" ] || continue
    row="$(last_row "$id")"
    if [ "$(field_of "$row" 2)" = pass ]; then
        # A pass certifies the tree it ran against, so it does not survive an edit.
        if [ "$(field_of "$row" 3)" = "$HEAD_SHA" ]; then
            printf '\033[90m· %s (already pass)\033[0m\n' "$id"
            continue
        fi
        printf '\033[90m· %s (pass was at another commit — re-running)\033[0m\n' "$id"
    fi
    if ! run_step "$id" "$kind" "$title"; then
        FAILED=1
        break
    fi
done

printf '\n\033[1m── %s · phase %s ──\033[0m\n' "$VERSION" "$PHASE"
blocked=0
for s in "${STEPS[@]}"; do
    IFS='|' read -r id phase kind title <<<"$s"
    [ "$phase" = "$PHASE" ] || continue
    row="$(last_row "$id")"
    [ "$(field_of "$row" 2)" = blocked ] || continue
    blocked=$((blocked + 1))
    printf '\033[33mBLOCKED\033[0m %s — %s\n' "$id" "$(field_of "$row" 4)"
done
if [ "$FAILED" -ne 0 ]; then
    suffix=""
    [ "$PHASE" = post ] && suffix=" --post"
    printf '\033[31mstopped at a failing step — resume: just release-e2e %s%s\033[0m\n' "$VERSION" "$suffix"
elif [ "$blocked" -gt 0 ]; then
    printf '\033[33mno failures, but %d step(s) BLOCKED — that coverage is NOT verified\033[0m\n' "$blocked"
else
    printf '\033[32mevery step verified\033[0m\n'
fi
exit "$FAILED"
