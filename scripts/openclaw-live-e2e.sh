#!/usr/bin/env bash
# OpenClaw daemon live-e2e — drives the REAL `pixtuoid-hook` shim with crafted
# OpenClaw gateway envelopes on an ISOLATED socket, and asserts the wandering
# "Molty" mascot's presence transitions via the headless
# `daemons=[openclaw:<state>]` summary line:
#
#   idle (gateway_start) -> busy (before_agent_run) -> idle (agent_end) -> down (gateway_stop)
#
# Zero real gateway, zero model calls, zero side effects — it exercises the full
# in-process daemon path end to end: the shim -> HookRouter (the shared-socket
# owner) -> the registry-driven daemon demux in handle_conn -> daemon::apply_presence
# (source-tagged) -> SceneState.daemons -> the headless summary. The abrupt-down
# PresenceExitWatch (PidExited) rung is covered by the unit suite + can be driven
# manually with a `gateway_start` carrying a live `_pid` you then kill.
#
# Build first:  just build --release
# Run:          scripts/openclaw-live-e2e.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PIX="$REPO/target/release/pixtuoid"
HOOK="$REPO/target/release/pixtuoid-hook"
SOCK="${TMPDIR:-/tmp}/pixtuoid-openclaw-e2e.sock"
OUT="$(mktemp)"
PROJ="$(mktemp -d)"
PIXPID=""

for bin in "$PIX" "$HOOK"; do
    [ -x "$bin" ] || {
        echo "missing $bin — run: just build --release" >&2
        exit 2
    }
done

# shellcheck disable=SC2329  # invoked indirectly via `trap cleanup EXIT` below
cleanup() {
    [ -n "$PIXPID" ] && kill "$PIXPID" 2>/dev/null
    rm -f "$SOCK" "$OUT"
    rm -rf "$PROJ"
}
trap cleanup EXIT
rm -f "$SOCK"

# Headless pixtuoid on an isolated socket (won't collide with a running instance
# or a real gateway). OpenClaw is connected via the persisted [sources] flag or
# the install-state migrate-default; an empty projects root keeps agents=[].
PIXTUOID_SOCKET="$SOCK" "$PIX" run --headless --projects-root "$PROJ" >"$OUT" 2>&1 &
PIXPID=$!
for _ in $(seq 1 50); do
    [ -S "$SOCK" ] && break
    sleep 0.1
done
[ -S "$SOCK" ] || {
    echo "FAIL: HookRouter never bound $SOCK" >&2
    exit 1
}
sleep 0.3
echo "pixtuoid headless up (pid $PIXPID), HookRouter owns $SOCK"

send() { printf '%s\n' "$1" | PIXTUOID_SOCKET="$SOCK" "$HOOK" --source openclaw; }

FAILED=0
# Wait until the LATEST `daemons=` line is the wanted state — distinguishes the
# idle -> busy -> idle round trip (a plain grep-anywhere can't).
expect() {
    local want="$1" label="$2" last
    for _ in $(seq 1 40); do
        last="$(grep 'daemons=' "$OUT" | tail -1)"
        case "$last" in
        *"daemons=[openclaw:$want]"*)
            echo "  PASS $label  ($last)"
            return 0
            ;;
        esac
        sleep 0.2
    done
    echo "  FAIL $label — wanted openclaw:$want, last: $(grep 'daemons=' "$OUT" | tail -1)" >&2
    FAILED=1
}

echo "[1] gateway_start    -> idle"
send '{"type":"gateway_start"}'
expect idle idle

echo "[2] before_agent_run -> busy"
send '{"type":"before_agent_run","runId":"r1"}'
expect busy busy

echo "[3] agent_end        -> idle"
send '{"type":"agent_end","runId":"r1"}'
expect idle idle-again

echo "[4] gateway_stop     -> down"
send '{"type":"gateway_stop"}'
expect down down

echo "--- Molty timeline (headless) ---"
grep 'daemons=' "$OUT" | sed 's/^/  /'
if [ "$FAILED" = 0 ]; then
    echo "openclaw-live-e2e: PASS"
else
    echo "openclaw-live-e2e: FAIL" >&2
fi
exit "$FAILED"
