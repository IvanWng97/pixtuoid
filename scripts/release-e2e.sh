#!/usr/bin/env bash
# THE end-to-end entry point. `just release-e2e X.Y.Z` walks the whole checklist
# before a tag (add --post after release.yml is green); the openclaw and replay
# recipes are `--only` views of the same steps, so the release path is exercised
# every time a tier is run mid-cycle.
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
export VERSION

phase_of() {
    local id="$1" cur=pre s sid
    for s in "${STEPS[@]}"; do
        sid="${s%%|*}"
        if [ "$sid" = "$id" ]; then
            echo "$cur"
            return 0
        fi
        [ "$sid" = "$PHASE1_LAST" ] && cur=post
    done
    echo unknown
}

call_body() {
    if [ "${#TIER_ARGS[@]}" -gt 0 ]; then
        "$1" "${TIER_ARGS[@]}"
    else
        "$1"
    fi
}

record() { printf '%s|%s|%s\n' "$1" "$2" "${3:-}" >>"$STATE"; }
verdict_of() { grep "^$1|" "$STATE" 2>/dev/null | tail -1 | cut -d'|' -f2; }
reason_of() { grep "^$1|" "$STATE" 2>/dev/null | tail -1 | cut -d'|' -f3; }

run_step() {
    local id="$1" kind="$2" title="$3" rc ans reason
    printf '\n\033[1m▸ %s\033[0m  %s\n' "$id" "$title"
    if [ "$kind" = auto ]; then
        (call_body "auto_$id")
        rc=$?
        if [ "$rc" -eq 0 ]; then
            record "$id" pass
            printf '  \033[32m✓ pass\033[0m\n'
            return 0
        fi
        record "$id" fail
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
        printf '  [y] pass  [n] fail  [s] skip  [q] quit > '
        read -r ans
        case "$ans" in
        y)
            record "$id" pass
            return 0
            ;;
        n)
            record "$id" fail
            return 1
            ;;
        s)
            printf '  reason (required): '
            read -r reason
            if [ -z "$reason" ]; then
                echo "  a skip needs a reason"
                continue
            fi
            record "$id" skip "$reason"
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
        IFS='|' read -r id kind title <<<"$s"
        [ "$(phase_of "$id")" = "$PHASE" ] || continue
        printf '%-20s %-7s %s\n' "$id" "$kind" "$title"
    done
    exit 0
fi

if [ -n "$ONLY" ]; then
    for s in "${STEPS[@]}"; do
        IFS='|' read -r id kind title <<<"$s"
        [ "$id" = "$ONLY" ] || continue
        STATE=/dev/null
        run_step "$id" "$kind" "$title"
        exit $?
    done
    echo "unknown step: $ONLY" >&2
    exit 2
fi

if [ -z "$VERSION" ]; then
    echo "usage: release-e2e.sh <X.Y.Z> [--post] [--from <id>] [--list]" >&2
    exit 2
fi

STATEDIR="${XDG_STATE_HOME:-$HOME/.local/state}/pixtuoid/release-e2e"
mkdir -p "$STATEDIR"
STATE="$STATEDIR/$VERSION-$PHASE"
: >>"$STATE"
if [ -n "$FROM" ]; then
    grep -v "^$FROM|" "$STATE" >"$STATE.tmp" 2>/dev/null
    mv "$STATE.tmp" "$STATE"
fi

FAILED=0
for s in "${STEPS[@]}"; do
    IFS='|' read -r id kind title <<<"$s"
    [ "$(phase_of "$id")" = "$PHASE" ] || continue
    if [ "$(verdict_of "$id")" = pass ]; then
        printf '\033[90m· %s (already pass)\033[0m\n' "$id"
        continue
    fi
    if ! run_step "$id" "$kind" "$title"; then
        FAILED=1
        break
    fi
done

printf '\n\033[1m── %s · phase %s ──\033[0m\n' "$VERSION" "$PHASE"
for s in "${STEPS[@]}"; do
    IFS='|' read -r id kind title <<<"$s"
    [ "$(phase_of "$id")" = "$PHASE" ] || continue
    [ "$(verdict_of "$id")" = skip ] || continue
    printf '\033[33mSKIPPED\033[0m %s — %s\n' "$id" "$(reason_of "$id")"
done
if [ "$FAILED" -eq 0 ]; then
    printf '\033[32mall steps accounted for\033[0m\n'
else
    suffix=""
    [ "$PHASE" = post ] && suffix=" --post"
    printf '\033[31mstopped at a failing step — resume: just release-e2e %s%s\033[0m\n' "$VERSION" "$suffix"
fi
exit "$FAILED"
