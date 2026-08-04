#!/usr/bin/env bash
# OpenClaw + Claude-Code-backend COMBINED live-e2e: the gateway DAEMON renders as
# the wandering lobster mascot while its bundled `claude-cli` backend coding
# session renders as a full-fidelity `cc·` desk sprite, in one headless scene:
#
#   agents=[… cc·<workspace>@N …] daemons=[openclaw@<port>:busy]
#
# ⚠ REAL side effects — NOT hermetic, NOT a CI test. It starts YOUR gateway (the
# iMessage channel connects and could auto-reply to an inbound text during the
# ~30s window) and makes ONE real model turn on your Anthropic auth. Requires
# `openclaw` (with the pixtuoid plugin + a claude-cli backend agent) and `claude`.
#
# Build first:  just build --release
# Run:          scripts/openclaw-cc-backend-e2e.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PIX="$REPO/target/release/pixtuoid"
PROJECTS="$HOME/.claude/projects"
CFGDIR="$(mktemp -d)"
# The socket lives inside a PRIVATE 0700 dir, never as a fixed name in the shared
# temp dir: a fixed name lets concurrent runs clobber each other's socket, and on
# a shared /tmp it is pre-plantable by another user — nothing downstream polices
# it, since `ensure_owned_socket_dir` leaves an explicit PIXTUOID_SOCKET alone.
SOCKDIR="$(mktemp -d)"
SOCK="$SOCKDIR/pixtuoid.sock"
PIXLOG="$(mktemp)"
GWLOG="$(mktemp)"
AGENTLOG="$(mktemp)"
PIXPID=""
GWPID=""

for bin in openclaw claude; do
    command -v "$bin" >/dev/null 2>&1 || {
        echo "missing '$bin' on PATH — this live test needs a real OpenClaw + Claude Code install" >&2
        exit 2
    }
done
[ -x "$PIX" ] || {
    echo "missing $PIX — run: just build --release" >&2
    exit 2
}
[ -d "$PROJECTS" ] || {
    echo "no $PROJECTS — has Claude Code ever run on this machine?" >&2
    exit 2
}
# The user's config need not resolve the default, so the conflict guard and the
# cleanup reap must resolve the port the same way — pinning the default means the
# guard passes while a gateway IS running and the reap leaks the one we started.
# Env overrides are NOT mirrored: that re-implements `parseGatewayPortEnvValue`.
PORT="$(openclaw config get gateway.port 2>/dev/null | tr -d '" ' | tail -1)"
case "$PORT" in
'' | *[!0-9]*)
    PORT="$(sed -n 's/^const DEFAULT_GATEWAY_PORT = \([0-9][0-9]*\);.*/\1/p' \
        "$REPO/crates/pixtuoid/src/install/openclaw_plugin.js")"
    ;;
esac
[ -n "$PORT" ] || {
    echo "could not resolve the gateway port (openclaw config, nor openclaw_plugin.js)" >&2
    exit 2
}

# Don't fight an already-running gateway: its plugin uses ITS env's socket, so we
# could not isolate. Bail rather than --force-kill the user's gateway.
if lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "a gateway is already listening on :$PORT — stop it first (this test starts its own)" >&2
    exit 2
fi

# Reaps by job pid AND by listening port: a version that daemonises — or a kill
# that failed — would leak the port. The port reap is scoped to OUR resolved port,
# never `pkill -f 'openclaw gateway run'`, which would kill a gateway this script
# never started.
cleanup() {
    [ -n "$GWPID" ] && kill "$GWPID" 2>/dev/null
    local port_pids
    port_pids="$(lsof -ti tcp:"$PORT" -sTCP:LISTEN 2>/dev/null)"
    # shellcheck disable=SC2086  # word-split is intended — one kill per listener pid
    [ -n "$port_pids" ] && kill $port_pids 2>/dev/null
    sleep 1
    port_pids="$(lsof -ti tcp:"$PORT" -sTCP:LISTEN 2>/dev/null)"
    # shellcheck disable=SC2086  # word-split is intended — one kill per listener pid
    [ -n "$port_pids" ] && kill -9 $port_pids 2>/dev/null
    [ -n "$PIXPID" ] && kill "$PIXPID" 2>/dev/null
    rm -f "$PIXLOG" "$GWLOG" "$AGENTLOG"
    rm -rf "$CFGDIR" "$SOCKDIR"
}
trap cleanup EXIT

# The backend's `cc·` label is the openclaw agent WORKSPACE's cwd basename, since
# the claude-cli backend runs there. Naming it directly, rather than diffing a
# baseline, stays correct when OTHER live cc· sessions linger in the scene.
WS_PATH="$(openclaw config get agents.defaults.workspace 2>/dev/null | tr -d '"' | tail -1)"
WS_LABEL="cc·$(basename "${WS_PATH:-workspace}")"

# Both sources must be connected — the presence/agent connection-gates drop
# deltas for a disconnected source. Isolated so the dev's real config is untouched.
mkdir -p "$CFGDIR/pixtuoid"
printf '[sources]\nopenclaw = true\nclaude-code = true\n' >"$CFGDIR/pixtuoid/config.toml"

echo "[1] headless pixtuoid -> isolated socket, watching $PROJECTS"
PIXTUOID_SOCKET="$SOCK" XDG_CONFIG_HOME="$CFGDIR" \
    "$PIX" run --headless --projects-root "$PROJECTS" >"$PIXLOG" 2>&1 &
PIXPID=$!
for _ in $(seq 1 50); do
    [ -S "$SOCK" ] && break
    sleep 0.1
done
[ -S "$SOCK" ] || {
    echo "FAIL: HookRouter never bound $SOCK" >&2
    exit 1
}
sleep 1.5
echo "    watching for the backend label: $WS_LABEL  (openclaw workspace)"

echo "[2] openclaw gateway run (plugin -> $SOCK)"
PIXTUOID_SOCKET="$SOCK" openclaw gateway run --bind loopback >"$GWLOG" 2>&1 &
GWPID=$!
lobster_up=0
for _ in $(seq 1 120); do
    case "$(grep 'daemons=' "$PIXLOG" | tail -1)" in
    *"openclaw@"*)
        lobster_up=1
        break
        ;;
    esac
    sleep 0.25
done
[ "$lobster_up" = 1 ] || {
    echo "FAIL: the lobster never appeared (gateway plugin didn't reach $SOCK)" >&2
    echo "--- gateway log tail ---" >&2
    tail -6 "$GWLOG" >&2
    exit 1
}
echo "    the lobster up: $(grep 'daemons=' "$PIXLOG" | tail -1 | grep -oE 'daemons=\[[^]]*\]')"

echo "[3] openclaw agent --message (routes to the claude-cli backend)"
(
    openclaw agent --message "Reply with exactly one word: pong" \
        --session-key agent:main:pixtuoid-cc-e2e --timeout 120 >"$AGENTLOG" 2>&1
    echo "AGENT_TURN_EXIT=$?" >>"$AGENTLOG"
) &

# The two-wildcard globs below would also match a `:busy` belonging to a DIFFERENT
# daemon row; harmless while openclaw is the only daemon source, and an agent row's
# `:busy` can't reach them since `agents=` always precedes `daemons=` on the line.
saw_backend=0
saw_busy=0
saw_both=0
for _ in $(seq 1 480); do
    line="$(grep 'agents=' "$PIXLOG" | tail -1)"
    case "$line" in *"$WS_LABEL"*) saw_backend=1 ;; esac
    case "$line" in *"openclaw@"*":busy"*) saw_busy=1 ;; esac
    case "$line" in *"$WS_LABEL"*"openclaw@"*":busy"*) saw_both=1 ;; esac
    [ "$saw_both" = 1 ] && break
    # Both seen across frames is enough: the backend can first-sight a beat after
    # before_agent_run, so don't require same-line.
    grep -q AGENT_TURN_EXIT "$AGENTLOG" 2>/dev/null && [ "$saw_backend" = 1 ] && [ "$saw_busy" = 1 ] && break
    sleep 0.25
done

echo "--- combined timeline (backend cc· + the gateway daemon) ---"
grep -F "$WS_LABEL" "$PIXLOG" | grep -E 'daemons=\[openclaw@[0-9]+:(busy|idle)\]' |
    sed 's/:active([^)]*)//g' | tail -4 | sed 's/^/  /'
echo "--- backend agent reply ---"
sed 's/^/  /' "$AGENTLOG" | tail -4

FAILED=0
if [ "$saw_backend" = 1 ]; then
    echo "PASS  backend session rendered as a cc· sprite: $WS_LABEL"
else
    echo "FAIL  backend cc· sprite ($WS_LABEL) never appeared (did the claude-cli turn run?)" >&2
    FAILED=1
fi
if [ "$saw_busy" = 1 ]; then
    echo "PASS  the lobster went busy during the backend run (openclaw@<port>:busy)"
else
    echo "FAIL  never observed openclaw@<port>:busy" >&2
    FAILED=1
fi
[ "$saw_both" = 1 ] && echo "PASS  both rendered in ONE frame ($WS_LABEL + openclaw@<port>:busy)"

if [ "$FAILED" = 0 ]; then
    echo "openclaw-cc-backend-e2e: PASS — the lobster (gateway) + cc· (claude-cli backend) coexist live"
else
    echo "openclaw-cc-backend-e2e: FAIL" >&2
fi
trap - EXIT
cleanup
exit "$FAILED"
