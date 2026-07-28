#!/usr/bin/env bash
# OpenClaw MULTI-GATEWAY live-e2e — N REAL `openclaw gateway run` processes, each
# in its own isolated state dir on its own port, all feeding ONE headless pixtuoid:
#
#   daemons=[openclaw@18901:idle, openclaw@18902:idle, …]   (one row per gateway)
#   kill one  ->  only ITS row goes down
#
# WHY a third openclaw e2e script. The sibling `openclaw-live-e2e.sh` is hermetic
# (crafted envelopes through the real shim) and `openclaw-cc-backend-e2e.sh` is the
# expensive one (a real gateway AND a real model turn). This sits between them: a
# REAL gateway — its real plugin loader, its real hook firing, its real pid — with
# ZERO model calls and zero account footprint, which is the only way to check the
# things the hermetic script cannot:
#   * the config keys `pixtuoid connect openclaw` writes are the keys OpenClaw
#     actually reads (asserted via `openclaw plugins list` reporting us `enabled`),
#   * the plugin resolves its own bound port per gateway (the mascot's identity),
#   * N mascots coexist without the roster/painter collapsing any pair. All daemon
#     presences render on ONE floor (the ground floor — a mascot has no desk), so
#     N gateways share one small visit-spot set; that crowding is exactly where a
#     4-gateway render was found drawing only 3 lobsters.
#
# It is NOT a CI test (it needs a real `openclaw` on PATH) and it is NOT hermetic in
# the "no external binary" sense — but it IS side-effect-free: every gateway runs
# from a throwaway `OPENCLAW_HOME`, so the dev's own ~/.openclaw is never read or
# written, no channel connects, and nothing is billed.
#
# Build first:  just build --release
# Run:          scripts/openclaw-multi-gateway-e2e.sh [port ...]   (default: 4 ports)
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PIX="$REPO/target/release/pixtuoid"
# Four by default: two proves the keying, four proves there is no arity assumption
# AND exercises the one-floor crowding the offset logic exists for.
PORTS=("$@")
[ "${#PORTS[@]}" -gt 0 ] || PORTS=(18901 18902 18903 18904)

[ -x "$PIX" ] || {
    echo "missing $PIX — run: just build --release" >&2
    exit 2
}
command -v openclaw >/dev/null 2>&1 || {
    echo "missing 'openclaw' on PATH — this live test drives the REAL gateway" >&2
    exit 2
}
for port in "${PORTS[@]}"; do
    if lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
        echo "port $port is already in use — pass free ports, or stop that listener" >&2
        exit 2
    fi
done

MG="$(mktemp -d)"
SOCK="$MG/pixtuoid.sock"
OUT="$MG/pixtuoid.log"
GW_PIDS=()
PIXPID=""

cleanup() {
    for p in "${GW_PIDS[@]:-}"; do
        [ -n "$p" ] && kill "$p" 2>/dev/null
    done
    sleep 1
    # Belt-and-braces port reap. `$!` IS the listener on openclaw 2026.7.1 (verified:
    # the job pid holds the port, comm `openclaw-gateway`), so the kills above are
    # normally enough — this catches a version that daemonises instead, or a gateway
    # that outlived a failed kill, so a crashed run can't leave a test port bound.
    for port in "${PORTS[@]}"; do
        left="$(lsof -ti tcp:"$port" -sTCP:LISTEN 2>/dev/null)"
        # shellcheck disable=SC2086  # word-split intended — one kill per listener pid
        [ -n "$left" ] && kill -9 $left 2>/dev/null
    done
    [ -n "$PIXPID" ] && kill "$PIXPID" 2>/dev/null
    rm -rf "$MG"
}
trap cleanup EXIT

# Isolated pixtuoid config marking OpenClaw connected — the presence connection-gate
# drops every delta for a DISconnected source, so a clean box would else time out.
mkdir -p "$MG/proj" "$MG/cfg/pixtuoid"
printf '[sources]\nopenclaw = true\n' >"$MG/cfg/pixtuoid/config.toml"

# One throwaway state dir per gateway. `gateway.mode` is REQUIRED — `gateway run`
# refuses a config without it as possibly-clobbered — and `bind: loopback` keeps
# each gateway local-only.
for port in "${PORTS[@]}"; do
    home="$MG/home$port"
    mkdir -p "$home/.openclaw"
    printf '{ "gateway": { "mode": "local", "bind": "loopback", "port": %d } }\n' "$port" \
        >"$home/.openclaw/openclaw.json"
    OPENCLAW_HOME="$home" XDG_CONFIG_HOME="$MG/cfg" "$PIX" connect openclaw --json >/dev/null || {
        echo "FAIL: connect openclaw failed for port $port" >&2
        exit 1
    }
    # THE install cross-check: OpenClaw's own loader must report the plugin enabled.
    # A config merge that writes keys upstream does not read passes every unit test
    # and still renders no lobster.
    OPENCLAW_HOME="$home" openclaw plugins list 2>/dev/null | grep -q 'pixtuoid.*enabled' || {
        echo "FAIL: openclaw does not report the pixtuoid plugin enabled (port $port)" >&2
        exit 1
    }
    echo "  port $port: plugin installed, and OpenClaw itself reports it enabled"
done

XDG_CONFIG_HOME="$MG/cfg" PIXTUOID_SOCKET="$SOCK" \
    "$PIX" run --headless --projects-root "$MG/proj" >"$OUT" 2>&1 &
PIXPID=$!
for _ in $(seq 1 50); do
    [ -S "$SOCK" ] && break
    sleep 0.1
done
[ -S "$SOCK" ] || {
    echo "FAIL: HookRouter never bound $SOCK" >&2
    exit 1
}
echo "  headless pixtuoid up (pid $PIXPID), HookRouter owns $SOCK"

FAILED=0
# Wait until the LATEST `daemons=` line contains `want` (a substring match, so a
# per-instance assertion does not have to name every sibling).
expect_line() {
    local want="$1" label="$2" last
    for _ in $(seq 1 120); do
        last="$(grep 'daemons=' "$OUT" | tail -1)"
        case "$last" in
        *"$want"*)
            echo "  PASS $label  ($last)"
            return 0
            ;;
        esac
        sleep 0.3
    done
    echo "  FAIL $label — wanted '$want', last: $(grep 'daemons=' "$OUT" | tail -1)" >&2
    FAILED=1
}

echo "[1] start all ${#PORTS[@]} gateways"
for port in "${PORTS[@]}"; do
    OPENCLAW_HOME="$MG/home$port" PIXTUOID_SOCKET="$SOCK" \
        openclaw gateway run --bind loopback >"$MG/gw$port.log" 2>&1 &
    GW_PIDS+=("$!")
done
# The roster is a BTreeMap keyed by the instance STRING, so the summary lists
# gateways in LEXICOGRAPHIC port order — not the order they were passed. Sort here
# or `just openclaw-multi-e2e 18902 18901` fails on ordering alone while the render
# is perfectly correct.
want=""
while read -r port; do
    want="$want${want:+, }openclaw@$port:idle"
done < <(printf '%s\n' "${PORTS[@]}" | LC_ALL=C sort)
expect_line "daemons=[$want]" "${#PORTS[@]} gateways render ${#PORTS[@]} independent mascots"

# Step [1] is a hard PRECONDITION for step [2], not just another assertion.
# `expect_line` matches a substring of the LATEST line, so a gateway that has not
# announced yet satisfies "sibling $port is untouched" on its FIRST-EVER announce —
# which, after a step-[1] timeout, lands AFTER the kill.
if [ "$FAILED" != 0 ]; then
    echo "  SKIP [2] — not every gateway announced; the instance-local assertions would be vacuous" >&2
else
    echo "[2] kill the FIRST gateway (${PORTS[0]}) — only its own mascot walks out"
    # `$!` is the process that holds the port and stamps `_pid` into every envelope, so
    # this exercises the INSTANT abrupt-down rung (ExitWatch on the gateway pid), not a
    # timeout. That the assertion below can pass at all is the proof: `expect_line` gives
    # up after ~36s while the silence path (`PresenceTtl::DEFAULT.presence_ttl_ms`) is
    # FIVE MINUTES, so a regression that lost the pid rung could not sneak through here.
    kill "${GW_PIDS[0]}" 2>/dev/null
    GW_PIDS[0]=""
    expect_line "openclaw@${PORTS[0]}:down" "the killed gateway goes down (instant pid rung)"
    for port in "${PORTS[@]:1}"; do
        expect_line "openclaw@$port:idle" "sibling $port is untouched"
    done
fi

echo "--- the lobster timeline (headless):"
grep 'daemons=' "$OUT" | sed 's/^/    /'
if [ "$FAILED" = 0 ]; then
    echo "openclaw-multi-gateway-e2e: PASS"
else
    echo "openclaw-multi-gateway-e2e: FAIL" >&2
fi
trap - EXIT
cleanup
exit "$FAILED"
