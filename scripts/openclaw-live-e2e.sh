#!/usr/bin/env bash
# OpenClaw daemon live-e2e — drives the REAL `pixtuoid-hook` shim with crafted
# OpenClaw gateway envelopes on an ISOLATED socket, and asserts the wandering
# "Molty" mascot's presence transitions via the headless
# `daemons=[openclaw:<state>]` summary line:
#
#   idle (gateway_start) -> busy (before_agent_run) -> idle (agent_end) -> down (gateway_stop)
#   #317 degraded: busy -> degraded (agent_end success:false) -> busy -> idle (heal)
#   #318 mid-attach: a NON-gateway_start event carrying _pid arms the abrupt-down
#                    exit watch (PidSeen adoption) -> killing that pid -> down
#
# Zero real gateway, zero model calls, zero side effects — it exercises the full
# in-process daemon path end to end: the shim -> HookRouter (the shared-socket
# owner) -> the registry-driven daemon demux in handle_conn -> daemon::apply_presence
# (source-tagged) -> SceneState.daemons -> the headless summary. The #318 step
# needs an ExitWatch backend (macOS kqueue / Linux pidfd) — present on every dev
# platform; on a backend-less platform that one step would time out.
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
CFGDIR="$(mktemp -d)"
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
    rm -rf "$PROJ" "$CFGDIR"
}
trap cleanup EXIT
rm -f "$SOCK"

# Self-contained: an ISOLATED config (via XDG_CONFIG_HOME) that marks OpenClaw
# connected — the reducer's presence connection-gate drops every delta for a
# DISconnected source, so a clean dev box with no prior [sources] entry would
# otherwise time out. Don't touch the dev's real ~/.config/pixtuoid.
mkdir -p "$CFGDIR/pixtuoid"
printf '[sources]\nopenclaw = true\n' >"$CFGDIR/pixtuoid/config.toml"

# Headless pixtuoid on an isolated socket (won't collide with a running instance
# or a real gateway); empty projects root keeps agents=[].
XDG_CONFIG_HOME="$CFGDIR" PIXTUOID_SOCKET="$SOCK" "$PIX" run --headless --projects-root "$PROJ" >"$OUT" 2>&1 &
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

# ---- #317 degraded (model-backend failing) + self-heal ----
echo "[5] gateway_start    -> idle (fresh lifecycle)"
send '{"type":"gateway_start"}'
expect idle idle-fresh

echo "[6] before_agent_run -> busy"
send '{"type":"before_agent_run","runId":"r2"}'
expect busy busy-2

echo "[7] agent_end success:false -> degraded (#317)"
send '{"type":"agent_end","runId":"r2","success":false}'
expect degraded degraded

echo "[8] before_agent_run -> busy (re-attempt clears degraded)"
send '{"type":"before_agent_run","runId":"r3"}'
expect busy busy-retry

echo "[9] agent_end success:true -> idle (heals)"
send '{"type":"agent_end","runId":"r3","success":true}'
expect idle idle-healed

# ---- #318 mid-attach pid adoption + instant abrupt-down ----
# The daemon is up with current_pid=None (no gateway_start carried a _pid). A
# NON-gateway_start event carrying a REAL live _pid must adopt it (PidSeen), so
# killing that pid takes the daemon down via the PresenceExitWatch — the proof
# the mid-attach pid binding works (a non-adopted pid's death would be a no-op,
# leaving the daemon idle and failing step [11]).
sleep 600 &
SPID=$!
echo "[10] session_start carrying _pid=$SPID -> idle (PidSeen adopts the live pid)"
send "{\"type\":\"session_start\",\"sessionId\":\"mid1\",\"_pid\":$SPID}"
expect idle idle-midattach

echo "[11] kill $SPID -> down (instant abrupt-down off the adopted pid, #318)"
kill "$SPID" 2>/dev/null
expect down down-abrupt

echo "--- Molty timeline (headless) ---"
grep 'daemons=' "$OUT" | sed 's/^/  /'
if [ "$FAILED" = 0 ]; then
    echo "openclaw-live-e2e: PASS"
else
    echo "openclaw-live-e2e: FAIL" >&2
fi
exit "$FAILED"
