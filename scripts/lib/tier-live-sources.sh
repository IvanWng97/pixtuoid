#!/usr/bin/env bash
# LIVE multi-source e2e — launches each supported agent CLI non-interactively and
# asserts ITS badge appears in one headless pixtuoid's scene. This is the only
# tier that proves a real CLI's real output reaches a real sprite; corpus_check
# proves decode over stored bytes, and the fixtures prove the wire contract.
#
# ⚠ BILLED and NOT hermetic: every CLI here makes one real model turn on YOUR
# account for that provider. Run deliberately. Zero repo side effects — each CLI
# runs in a throwaway workspace with an isolated socket and config.
#
# Build first:  just build --release
# Run:          just live-sources [source-id ...]   (default: every present CLI)
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
. "$here/e2e-common.sh"

REPO="$(e2e_repo_root)"
PIX="$REPO/target/release/pixtuoid"
HOOK="$REPO/target/release/pixtuoid-hook"
ROSTER_BIN="$REPO/target/release/examples/corpus_check"
e2e_require_bin "$PIX" "$HOOK" "$ROSTER_BIN"

# The one place per-CLI knowledge lives, and it is unavoidable: a headless
# invocation is as source-specific as the decoder. A source absent here is
# reported as uncovered, never silently skipped.
invocation_for() {
    case "$1" in
    claude-code) echo "claude|-p" ;;
    codex) echo "codex|exec" ;;
    reasonix) echo "reasonix|-p" ;;
    codewhale) echo "codewhale|exec" ;;
    opencode) echo "opencode|run" ;;
    hermes) echo "hermes|-z" ;;
    copilot) echo "copilot|-p" ;;
    *) return 1 ;;
    esac
}

# One turn, three things verified: the session registers, a TOOL call drives the
# per-source tool-detail decode (where the decoders differ most), and the slot
# returns. Asking for a file READ rather than a shell command keeps it inside
# every CLI's default permissions, so no turn is spent on a approval prompt.
PROMPT='Read the file NOTE.txt in the current directory and reply with only its contents.'
TURN_TIMEOUT=180

SB="$(e2e_sandbox)"
SOCK="$SB/pixtuoid.sock"
OUT="$SB/pixtuoid.log"
PROJ="$SB/projects"
CFG="$SB/config"
WS="$SB/workspace"
mkdir -p "$PROJ" "$CFG/pixtuoid" "$WS"
printf 'pong\n' >"$WS/NOTE.txt"
# A git repo, because several CLIs refuse to act outside one (codex: "Not inside
# a trusted directory"). Cheaper than a per-CLI trust flag, and it is what a real
# user's workspace looks like anyway.
git init -q "$WS" 2>/dev/null
git -C "$WS" add -A 2>/dev/null
git -C "$WS" -c user.email=e2e@pixtuoid -c user.name=e2e commit -qm init 2>/dev/null
PIXPID=""
CLIPID=""

cleanup() {
    [ -n "$CLIPID" ] && kill "$CLIPID" 2>/dev/null
    [ -n "$PIXPID" ] && kill "$PIXPID" 2>/dev/null
    rm -rf "$SB"
}
trap cleanup EXIT

# Every source connected: the reducer's connection gate drops events for a
# DISconnected source, so an unset [sources] key reads as "the CLI never ran".
{
    echo '[sources]'
    "$ROSTER_BIN" --roster | cut -f1 | sed 's/$/ = true/'
} >"$CFG/pixtuoid/config.toml"

# claude-code watches a projects ROOT; the others resolve their own homes, so the
# real host dirs stay in play for them and only CC's tree is isolated.
XDG_CONFIG_HOME="$CFG" PIXTUOID_SOCKET="$SOCK" \
    "$PIX" run --headless --projects-root "$PROJ" --log-level error >"$OUT" 2>&1 &
PIXPID=$!
for _ in $(seq 1 50); do
    [ -S "$SOCK" ] && break
    sleep 0.1
done
[ -S "$SOCK" ] || {
    echo "FAIL: HookRouter never bound $SOCK" >&2
    exit 1
}
echo "headless pixtuoid up (pid $PIXPID), socket $SOCK"

wanted=("$@")
if [ "${#wanted[@]}" -eq 0 ]; then
    while IFS=$'\t' read -r id _ _; do
        wanted+=("$id")
    done < <("$ROSTER_BIN" --roster)
fi

present_ids="$("$PIX" sources --json | jq -r '.[] | select(.cli_present) | .id' | paste -sd' ' -)"

FAILED=0
covered=0
declare_uncovered=""
for id in "${wanted[@]}"; do
    prefix="$("$ROSTER_BIN" --roster | awk -F'\t' -v i="$id" '$1==i{print $2}')"
    if [ -z "$prefix" ]; then
        echo "  SKIP $id — not a registered source"
        continue
    fi
    if ! spec="$(invocation_for "$id")"; then
        declare_uncovered="$declare_uncovered $id"
        continue
    fi
    case " $present_ids " in
    *" $id "*) ;;
    *)
        echo "  SKIP $id — its CLI is not installed here"
        declare_uncovered="$declare_uncovered $id"
        continue
        ;;
    esac

    # Spend the turn only if the integration could possibly report. A broken
    # install (a hook path left dangling by an uninstall) yields no events no
    # matter how well the CLI runs, so burning a message to discover that is waste.
    health="$("$PIX" sources --json | jq -r --arg i "$id" '.[] | select(.id==$i) | .health // empty')"
    if [ -n "$health" ]; then
        echo "  BLOCKED $id — $health"
        declare_uncovered="$declare_uncovered $id"
        continue
    fi

    bin="${spec%%|*}"
    sub="${spec##*|}"
    echo "[$id] $bin $sub — one real model turn"
    # stdin from /dev/null: a CLI that also accepts a piped prompt otherwise waits
    # on the inherited terminal forever (codex: "Reading additional input...").
    (cd "$WS" && PIXTUOID_SOCKET="$SOCK" "$bin" "$sub" "$PROMPT" </dev/null >"$SB/$id.log" 2>&1) &
    CLIPID=$!

    # Registration alone proves little — junk content registers a sprite too. The
    # tool-driven transition is the assertion; registration is reported so a
    # failure says WHICH half broke.
    seen=0
    drove=0
    for _ in $(seq 1 "$TURN_TIMEOUT"); do
        grep -q "agents=\[[^]]*${prefix}·" "$OUT" 2>/dev/null && seen=1
        grep -qE "agents=\[[^]]*${prefix}·[^]]*:(active|waiting)" "$OUT" 2>/dev/null && drove=1
        [ "$drove" = 1 ] && break
        kill -0 "$CLIPID" 2>/dev/null || {
            # The CLI finished; give the watcher a beat to observe its last write.
            sleep 3
            grep -q "agents=\[[^]]*${prefix}·" "$OUT" 2>/dev/null && seen=1
            grep -qE "agents=\[[^]]*${prefix}·[^]]*:(active|waiting)" "$OUT" 2>/dev/null && drove=1
            break
        }
        sleep 1
    done
    kill "$CLIPID" 2>/dev/null
    wait "$CLIPID" 2>/dev/null
    cli_rc=$?
    CLIPID=""

    # A CLI that failed on ITS OWN account (no credit, no API key, not logged in)
    # says nothing about pixtuoid. Reporting that as a decode failure would be the
    # same lie as calling an unrunnable check a pass.
    if [ "$drove" = 0 ] && [ "$seen" = 0 ] && [ "$cli_rc" -ne 0 ]; then
        echo "  BLOCKED $id — the CLI itself failed (exit $cli_rc), not a pixtuoid result"
        tail -2 "$SB/$id.log" | sed 's/^/      /'
        declare_uncovered="$declare_uncovered $id"
        continue
    fi

    if [ "$drove" = 1 ]; then
        echo "  PASS $id — ${prefix}· registered AND reached a lifecycle state"
        covered=$((covered + 1))
    elif [ "$seen" = 1 ]; then
        echo "  FAIL $id — ${prefix}· registered but never left idle (tool-detail decode?)" >&2
        FAILED=1
    else
        echo "  FAIL $id — no ${prefix}· sprite after its turn" >&2
        echo "  --- $bin output tail ---" >&2
        tail -5 "$SB/$id.log" >&2
        FAILED=1
    fi
done

echo "--- scene timeline ---"
grep 'agents=' "$OUT" | tail -5 | sed 's/^/    /'
echo "live-sources: $covered source(s) rendered from a real CLI turn"
[ -n "$declare_uncovered" ] && echo "live-sources: NOT COVERED —$declare_uncovered"
if [ "$FAILED" = 0 ]; then
    echo "live-sources: PASS"
else
    echo "live-sources: FAIL" >&2
fi
trap - EXIT
cleanup
exit "$FAILED"
