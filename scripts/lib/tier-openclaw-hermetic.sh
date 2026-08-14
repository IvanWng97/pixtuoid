#!/usr/bin/env bash
# OpenClaw daemon live-e2e — drives the REAL `pixtuoid-hook` shim with crafted
# gateway envelopes on an ISOLATED socket, asserting the mascot's presence
# transitions via the headless `daemons=[openclaw@<port>:<state>]` summary line.
# The #318 step needs an ExitWatch backend (macOS kqueue / Linux pidfd); on a
# backend-less platform that one step times out.
#
# Build first:  just build --release
# Run:          just openclaw-e2e
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
. "$here/e2e-common.sh"

REPO="$(e2e_repo_root)"
PIX="$REPO/target/release/pixtuoid"
HOOK="$REPO/target/release/pixtuoid-hook"
e2e_require_bin "$PIX" "$HOOK"

SB="$(e2e_sandbox)"
OUT="$SB/pixtuoid.log"
PROJ="$SB/projects"
CFGDIR="$SB/config"
SOCK="$SB/pixtuoid.sock"
mkdir -p "$PROJ" "$CFGDIR"
PIXPID=""

cleanup() {
    [ -n "$PIXPID" ] && kill "$PIXPID" 2>/dev/null
    # The pid-exit steps' background `sleep`s — set later, hence the `set -u`
    # guard; a Ctrl-C before the kill would leak a 10-min sleep.
    for p in "${SPID:-}" "${APID:-}" "${BPID:-}"; do
        [ -n "$p" ] && kill "$p" 2>/dev/null
    done
    rm -rf "$SB"
}
trap cleanup EXIT

# An ISOLATED config (via XDG_CONFIG_HOME, so the dev's real ~/.config/pixtuoid is
# untouched) marking OpenClaw connected — the reducer's presence connection-gate
# drops every delta for a DISconnected source, so a clean dev box with no prior
# [sources] entry would time out.
mkdir -p "$CFGDIR/pixtuoid"
printf '[sources]\nopenclaw = true\n' >"$CFGDIR/pixtuoid/config.toml"

# An empty projects root keeps agents=[].
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

# Two ports, because the identity under test is the PORT.
PORT_A=18789
PORT_B=19789

# Every envelope must carry a gatewayPort. A port-LESS one is not rejected — it
# falls back to the single legacy instance (the stale-plugin compatibility arm),
# which would make the multi-gateway steps below vacuous.
send() { printf '%s\n' "$1" | PIXTUOID_SOCKET="$SOCK" "$HOOK" --source openclaw; }
send_a() { send "$(printf '%s' "$1" | sed "s/}\$/,\"gatewayPort\":$PORT_A}/")"; }

FAILED=0
# Match the LAST `daemons=` line, not any line, so the idle -> busy -> idle round
# trip is distinguishable (a plain grep-anywhere can't).
# NOT hoisted into `e2e-common.sh`, and adjudicated three times (two review
# lenses + the online bot) — do not re-raise without new evidence. The bodies look
# alike; the retry bounds await different EVENT CLASSES. This tier's 40x0.2s bounds
# an in-process shim -> HookRouter -> reducer -> summary hop; multi-gateway's
# 120x0.3s bounds N real `openclaw gateway run` node cold boots; cc-backend's
# 120x0.25s for ONE real gateway predates that work. So "a real gateway appears" is
# an established ~30s class and "a hermetic transition lands" an 8s one, with no
# single correct shared value — a shared helper would take the timing as parameters
# and hide ~12 lines behind a 4-argument interface. The drift that actually bit (a
# `daemons=` format change rotting a script unseen) is caught by the recipes, not by
# sharing this. In-FILE duplication WAS collapsed: `expect` delegates here.
expect_line() {
    local want="$1" label="$2" last
    for _ in $(seq 1 40); do
        last="$(grep 'daemons=' "$OUT" | tail -1)"
        case "$last" in
        *"$want"*)
            echo "  PASS $label  ($last)"
            return 0
            ;;
        esac
        sleep 0.2
    done
    echo "  FAIL $label — wanted '$want', last: $(grep 'daemons=' "$OUT" | tail -1)" >&2
    FAILED=1
}

expect() { expect_line "daemons=[openclaw@$PORT_A:$1]" "$2"; }

echo "[1] gateway_start    -> idle"
send_a '{"type":"gateway_start"}'
expect idle idle

echo "[2] before_agent_run -> busy"
send_a '{"type":"before_agent_run","runId":"r1"}'
expect busy busy

echo "[3] agent_end        -> idle"
send_a '{"type":"agent_end","runId":"r1"}'
expect idle idle-again

echo "[4] gateway_stop     -> down"
send_a '{"type":"gateway_stop"}'
expect down down

echo "[5] gateway_start    -> idle (fresh lifecycle)"
send_a '{"type":"gateway_start"}'
expect idle idle-fresh

echo "[6] before_agent_run -> busy"
send_a '{"type":"before_agent_run","runId":"r2"}'
expect busy busy-2

echo "[7] agent_end success:false -> degraded (#317)"
send_a '{"type":"agent_end","runId":"r2","success":false}'
expect degraded degraded

echo "[8] before_agent_run -> busy (re-attempt clears degraded)"
send_a '{"type":"before_agent_run","runId":"r3"}'
expect busy busy-retry

echo "[9] agent_end success:true -> idle (heals)"
send_a '{"type":"agent_end","runId":"r3","success":true}'
expect idle idle-healed

# `PidSeen` adoption is None-ONLY, and the shim stamps a resolved _pid onto every
# event lacking one — so [5]'s gateway_start already armed current_pid, and a bare
# session_start here would adopt nothing. gateway_stop is used purely as the
# deterministic route back to current_pid=None; the mechanism under test is the
# None-only adoption, not the shutdown.
echo "[10] gateway_stop -> down (clears current_pid so the rung can re-arm)"
send_a '{"type":"gateway_stop"}'
expect down down-before-reattach

sleep 600 &
SPID=$!
echo "[11] session_start carrying _pid=$SPID (reconnect, no gateway_start) -> idle"
# The explicit _pid is KEPT by the shim (an inbound value wins over the walk). The
# `idle` here does NOT prove adoption — it comes from the SessionStarted resurrect;
# only [12] proves it, so keep both steps.
send "{\"type\":\"session_start\",\"sessionId\":\"mid1\",\"gatewayPort\":$PORT_A,\"_pid\":$SPID}"
expect idle idle-midattach

echo "[12] kill $SPID -> down (instant abrupt-down off the RE-adopted pid, #318)"
# THE assertion that gates #318: reds unless PidSeen actually adopted $SPID at [11].
kill "$SPID" 2>/dev/null
expect down down-abrupt

# Against a source-keyed roster every assertion below fails: the second
# gateway_start would CLEAR the first's runs and rebind its pid, and either
# gateway's exit would down "the" mascot.
echo "[13] two gateways start -> two independent mascots"
sleep 600 &
APID=$!
sleep 600 &
BPID=$!
send "{\"type\":\"gateway_start\",\"gatewayPort\":$PORT_A,\"_pid\":$APID}"
send "{\"type\":\"gateway_start\",\"gatewayPort\":$PORT_B,\"_pid\":$BPID}"
expect_line "daemons=[openclaw@$PORT_A:idle, openclaw@$PORT_B:idle]" two-gateways

echo "[14] a run on A only -> A busy, B still idle"
send "{\"type\":\"before_agent_run\",\"runId\":\"multi-a\",\"gatewayPort\":$PORT_A,\"_pid\":$APID}"
expect_line "daemons=[openclaw@$PORT_A:busy, openclaw@$PORT_B:idle]" busy-is-instance-local

echo "[15] kill A ($APID) -> only A goes down"
kill "$APID" 2>/dev/null
expect_line "daemons=[openclaw@$PORT_A:down, openclaw@$PORT_B:idle]" exit-is-instance-local

echo "[16] kill B ($BPID) -> B goes down too (its OWN receipt)"
# Assert only B's row: a full-line match would also assert A's row has not yet been
# swept, coupling this step to the down-removal TTL instead of B's own exit receipt.
kill "$BPID" 2>/dev/null
expect_line "openclaw@$PORT_B:down" both-down

echo "--- the lobster timeline (headless) ---"
grep 'daemons=' "$OUT" | sed 's/^/  /'
if [ "$FAILED" = 0 ]; then
    echo "openclaw-hermetic: PASS"
else
    echo "openclaw-hermetic: FAIL" >&2
fi
trap - EXIT
cleanup
exit "$FAILED"
