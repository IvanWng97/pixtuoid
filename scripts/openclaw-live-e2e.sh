#!/usr/bin/env bash
# OpenClaw daemon live-e2e — drives the REAL `pixtuoid-hook` shim with crafted
# OpenClaw gateway envelopes on an ISOLATED socket, and asserts the wandering
# lobster mascot's presence transitions via the headless
# `daemons=[openclaw@<port>:<state>]` summary line:
#
#   idle (gateway_start) -> busy (before_agent_run) -> idle (agent_end) -> down (gateway_stop)
#   #317 degraded: busy -> degraded (agent_end success:false) -> busy -> idle (heal)
#   #318 mid-attach: a NON-gateway_start event carrying _pid arms the abrupt-down
#                    exit watch (PidSeen adoption) -> killing that pid -> down
#   MULTI-GATEWAY: two gateways (two `gatewayPort`s, two owned pids) coexist as two
#                  independent mascots, and killing ONE process takes down only its
#                  own instance — the collapse this keying exists to prevent
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
OUT="$(mktemp)"
PROJ="$(mktemp -d)"
CFGDIR="$(mktemp -d)"
# The socket lives inside a PRIVATE 0700 dir, never as a fixed name in the shared
# temp dir: a fixed name makes two concurrent runs bind/`rm` each other's socket,
# and on a shared /tmp it is pre-plantable by another user (nothing downstream
# polices it — `ensure_owned_socket_dir` in hook/unix.rs deliberately leaves an
# explicit PIXTUOID_SOCKET path alone). Same shape as replay-fixture.sh and the
# sibling openclaw-multi-gateway-e2e.sh.
SOCKDIR="$(mktemp -d)"
SOCK="$SOCKDIR/pixtuoid.sock"
PIXPID=""

for bin in "$PIX" "$HOOK"; do
    [ -x "$bin" ] || {
        echo "missing $bin — run: just build --release" >&2
        exit 2
    }
done

cleanup() {
    [ -n "$PIXPID" ] && kill "$PIXPID" 2>/dev/null
    # The background `sleep`s the pid-exit steps own (set later, so guard under
    # `set -u`): a Ctrl-C before the kill would otherwise leak a 10-min sleep.
    for p in "${SPID:-}" "${APID:-}" "${BPID:-}"; do
        [ -n "$p" ] && kill "$p" 2>/dev/null
    done
    rm -f "$OUT"
    rm -rf "$PROJ" "$CFGDIR" "$SOCKDIR"
}
trap cleanup EXIT

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

# The gateway ports the crafted envelopes claim. Two, spaced like OpenClaw's own
# guidance for multiple gateways, because the identity under test is the PORT.
PORT_A=18789
PORT_B=19789

# Every envelope carries the gatewayPort the real plugin stamps. A port-LESS one
# is not rejected — it falls back to the single legacy instance (the stale-plugin
# compatibility arm), which would make the multi-gateway steps below vacuous. Only
# a PRESENT-but-invalid port rejects the whole envelope.
send() { printf '%s\n' "$1" | PIXTUOID_SOCKET="$SOCK" "$HOOK" --source openclaw; }
send_a() { send "$(printf '%s' "$1" | sed "s/}\$/,\"gatewayPort\":$PORT_A}/")"; }

FAILED=0
# Wait until the LATEST `daemons=` line matches a glob — the LAST line, not any
# line, so the idle -> busy -> idle round trip is distinguishable (a plain
# grep-anywhere can't). The 8s bound covers an in-process shim -> HookRouter ->
# reducer -> summary hop; the multi-gateway script's longer bound waits on real
# node gateway boots, a different event class.
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

# The single-gateway shorthand: assert gateway A is in exactly one state. A strict
# specialization of `expect_line`, so the poll + the row format live in one place.
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

# ---- #317 degraded (model-backend failing) + self-heal ----
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

# ---- #318 mid-attach pid adoption + instant abrupt-down ----
# `PidSeen` adoption is None-ONLY (apply_presence): it bootstraps current_pid
# only when the daemon has none, so GatewayUp never gets clobbered. That means
# the mid-attach scenario has to REACH current_pid=None first, and the shim
# stamps _pid=getppid() onto every event that lacks one — so the gateway_start
# at [5] already armed current_pid to the shim's parent. Sending a bare
# session_start here would adopt nothing (its PidSeen hits a Some) and killing
# its pid would be a no-op — the exact false-premise bug that made [11] fail.
#
# The real #318 trigger is a MID-ATTACH — pixtuoid attaches to an
# already-running gateway, never sees gateway_start, and adopts the pid off the
# first plain event into a fresh current_pid=None entry. That literal shape
# can't be reproduced in one process here, since [5]'s gateway_start already
# created the entry with an armed pid. So reach the SAME None state the honest
# way: gateway_stop takes the mascot Down and enter_down clears current_pid,
# then the reconnect's plain _pid event re-adopts via PidSeen — the identical
# adoption path (pinned by the pid_seen_re_adopts_after_an_abrupt_down unit
# test). gateway_stop is a clean shutdown, used here only as the deterministic
# route to current_pid=None; the mechanism under test is the None-only adoption.
echo "[10] gateway_stop -> down (clears current_pid so the rung can re-arm)"
send_a '{"type":"gateway_stop"}'
expect down down-before-reattach

sleep 600 &
SPID=$!
echo "[11] session_start carrying _pid=$SPID (reconnect, no gateway_start) -> idle"
# The explicit _pid is KEPT by the shim (an inbound value wins over getppid), so
# this is the real live pid. The `idle` here does NOT itself prove adoption — it
# comes from the SessionStarted resurrect; the adoption is proved only by [12]
# below (kill the adopted pid, get an instant down). Keep both steps.
send "{\"type\":\"session_start\",\"sessionId\":\"mid1\",\"gatewayPort\":$PORT_A,\"_pid\":$SPID}"
expect idle idle-midattach

echo "[12] kill $SPID -> down (instant abrupt-down off the RE-adopted pid, #318)"
# THE assertion that gates #318: this reds unless PidSeen actually adopted $SPID
# at [11] (verified by mutation — disabling the adoption fails exactly here).
kill "$SPID" 2>/dev/null
expect down down-abrupt

# ---- MULTI-GATEWAY: two ports, two mascots, instance-local death ----
# THE capability this keying exists for. Two crafted gateways, each with its OWN
# owned process, coexist; killing one takes down ONLY its instance. Against a
# source-keyed roster every one of these assertions fails: the second
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
# Assert only B's row: A has been Down since [15], so a full-line match would also
# be asserting that A's row has not yet been swept — coupling this step to the
# down-removal TTL rather than to B's own exit receipt.
kill "$BPID" 2>/dev/null
expect_line "openclaw@$PORT_B:down" both-down

echo "--- the lobster timeline (headless) ---"
grep 'daemons=' "$OUT" | sed 's/^/  /'
if [ "$FAILED" = 0 ]; then
    echo "openclaw-live-e2e: PASS"
else
    echo "openclaw-live-e2e: FAIL" >&2
fi
trap - EXIT
cleanup
exit "$FAILED"
