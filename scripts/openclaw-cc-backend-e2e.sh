#!/usr/bin/env bash
# OpenClaw + Claude-Code-backend COMBINED live-e2e — the REAL end-to-end proof of
# the OpenClaw daemon design premise (`source/openclaw.rs` module doc): the
# gateway DAEMON renders as the wandering lobster mascot (presence), WHILE its
# bundled `claude-cli` backend coding session renders as a full-fidelity `cc·`
# desk sprite. One headless scene, two sources, two sprites:
#
#   agents=[… cc·<workspace>@N …] daemons=[openclaw@<port>:busy]
#
# Flow:
#   1. headless pixtuoid binds an ISOLATED socket + watches ~/.claude/projects
#   2. `openclaw gateway run` (PIXTUOID_SOCKET pointed at that socket so its
#      pixtuoid plugin reaches THIS instance) -> gateway_start -> the lobster idle
#   3. `openclaw agent --message …` routes ONE turn to the claude-cli backend ->
#      before_agent_run -> the lobster busy AND a real `claude` writes ~/.claude/projects
#      -> a NEW `cc·` sprite (label = the openclaw workspace cwd basename)
#   4. assert a backend cc· label (absent from the pre-gateway baseline) AND
#      the gateway instance busy were both observed; then tear the gateway down
#
# The daemon rows are keyed `openclaw@<gatewayPort>` (the mascot's instance
# identity), so every assertion below matches the `openclaw@` PREFIX rather than a
# literal port — this script starts the gateway from the user's own config, whose
# resolved port need not be the default.
#
# ⚠ REAL side effects — UNLIKE the synthetic shim-driven `openclaw-live-e2e.sh`,
# this is NOT hermetic and NOT a CI test. It starts YOUR gateway (the iMessage
# channel connects and could auto-reply to an inbound text during the ~30s
# window) and makes ONE real model turn on your Anthropic auth (the agent reply
# is NOT delivered to any channel — `--deliver` is omitted). Requires `openclaw`
# (with the pixtuoid plugin installed + a claude-cli backend agent) and `claude`.
#
# Build first:  just build --release
# Run:          scripts/openclaw-cc-backend-e2e.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PIX="$REPO/target/release/pixtuoid"
PROJECTS="$HOME/.claude/projects"
CFGDIR="$(mktemp -d)"
# The socket lives inside a PRIVATE 0700 dir, never as a fixed name in the shared
# temp dir: a fixed name makes two concurrent runs bind/`rm` each other's socket,
# and on a shared /tmp it is pre-plantable by another user (nothing downstream
# polices it — `ensure_owned_socket_dir` in hook/unix.rs deliberately leaves an
# explicit PIXTUOID_SOCKET path alone). Same shape as replay-fixture.sh and the
# sibling openclaw-multi-gateway-e2e.sh.
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
# The port THIS run's gateway will bind. The assertions below deliberately match
# the `openclaw@` prefix because the user's config need not resolve the default —
# so the conflict guard and the cleanup reap have to resolve it the same way
# instead of pinning the default, or on a non-default box the guard passes while a
# gateway IS running and the reap leaks the one we started. Falls back to the ONE
# in-repo copy of OpenClaw's default (the plugin template's DEFAULT_GATEWAY_PORT,
# the same literal check_upstream_drift.py compares against upstream) rather than
# a second hardcoded number. Env overrides are NOT mirrored — that would mean
# re-implementing upstream's `parseGatewayPortEnvValue` in shell.
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

cleanup() {
    [ -n "$GWPID" ] && kill "$GWPID" 2>/dev/null
    # `openclaw gateway run` execs/forks a child node that holds the port — killing
    # the CLI wrapper alone LEAKS it. Kill whatever actually LISTENS on OUR port
    # (TERM, then KILL), so the user's machine isn't left with a stray gateway.
    # Reaping by port, not by `pkill -f 'openclaw gateway run'`: that pattern kills
    # EVERY gateway on the box, including one on another port that the guard above
    # never checked and this script never started. Same job-pid + port-listener
    # pair the sibling openclaw-multi-gateway-e2e.sh uses.
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

# The backend's `cc·` label is the openclaw agent WORKSPACE's cwd basename (the
# claude-cli backend runs there → its transcript keys on that cwd). Naming the
# backend directly is robust to OTHER live cc· sessions (yours) lingering in the
# scene — a baseline label-diff would miss it whenever a prior backend run is
# still within the watcher's first-sight window.
WS_PATH="$(openclaw config get agents.defaults.workspace 2>/dev/null | tr -d '"' | tail -1)"
WS_LABEL="cc·$(basename "${WS_PATH:-workspace}")"

# Isolated config: openclaw + claude-code both connected (the presence/agent
# connection-gates drop deltas for a disconnected source). Don't touch the dev's
# real ~/.config/pixtuoid.
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

# Watch for BOTH the backend cc· sprite (the workspace label) AND the gateway
# instance busy — ideally in the SAME line (the literal both-sources coexistence
# the demo proves). The two-wildcard globs below would also match a `:busy` that
# belongs to a DIFFERENT daemon row (`openclaw@N:idle, otherd@1:busy`); harmless
# because openclaw is the only daemon source, and an agent row's `:busy` can't
# reach them either since `agents=` always precedes `daemons=` on the line.
saw_backend=0
saw_busy=0
saw_both=0
for _ in $(seq 1 480); do
    line="$(grep 'agents=' "$PIXLOG" | tail -1)"
    case "$line" in *"$WS_LABEL"*) saw_backend=1 ;; esac
    case "$line" in *"openclaw@"*":busy"*) saw_busy=1 ;; esac
    case "$line" in *"$WS_LABEL"*"openclaw@"*":busy"*) saw_both=1 ;; esac
    [ "$saw_both" = 1 ] && break
    # Turn done + both seen (possibly across frames) is enough — the backend can
    # first-sight a beat after before_agent_run, so don't require same-line.
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
