#!/usr/bin/env bash
# One whole OpenClaw gateway lifecycle for `just capture-fixture openclaw <scenario>`.
#
# The DOOR matters more than the flags here. OpenClaw's plugin registers six
# hooks; `openclaw agent` fires only the gateway pair, because the four session
# hooks come from the gateway's get-reply path, which `agent` never enters — a
# capture through that door reads like upstream drift and is not. `openclaw tui`
# is the path that opens a session. `--deliver` stays off, so nothing reaches a
# real chat channel.
#
# `session_start` additionally needs a FRESH session key: a reused one RESUMES
# and stays silent, which is why OC_SESSION defaults to a timestamp.
#
# Note the recorded order: `session_end` lands AFTER `gateway_stop`, because
# shutdown is what closes the session.
set -uo pipefail

PROMPT="${1:?prompt}"
PORT="${OC_PORT:-19099}"
SESSION="${OC_SESSION:-pixcap-$(date +%s)}"
DRIVER="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/tuidrive.py"
# Beside the recorder's own private sandbox, never a fixed shared-temp name: the
# same symlink-followable, concurrently-clobbered class the recorder's socket and
# the driver's transcript were both moved out of.
LOGS="$(dirname "${TUIDRIVE_LOG:-$(mktemp -d)/x}")"

openclaw gateway --port "$PORT" --force --allow-unconfigured >"$LOGS/openclaw-gw.log" 2>&1 &
gw=$!
trap 'kill -TERM "$gw" 2>/dev/null' EXIT

for _ in $(seq 1 60); do
    openclaw gateway health --port "$PORT" >/dev/null 2>&1 && break
    sleep 1
done

python3 "$DRIVER" "$PROMPT" openclaw tui --session "$SESSION" >"$LOGS/openclaw-tui.log" 2>&1
echo "tui rc=$?"

# SIGTERM, not kill: `gateway_stop` is a clean-shutdown hook.
kill -TERM "$gw" 2>/dev/null
wait "$gw" 2>/dev/null
sleep 2
