#!/usr/bin/env bash
# One opencode session from creation to DELETION, for
# `just capture-fixture opencode <scenario>`.
#
# `session.deleted` has exactly one door: a mutation THROUGH a running server.
# The plugin listens on opencode's in-process event bus, and `opencode session
# delete` edits the SQLite store from its own process — verified to fire NOTHING
# even with a plugin-loaded `serve` up, which is why the obvious CLI route keeps
# coming back empty.
#
# Costs no model turn: a session's creation and deletion ARE the scenario.
set -uo pipefail

PORT="${OC_PORT:-4103}"
BASE="http://127.0.0.1:$PORT"

opencode serve --port "$PORT" >/tmp/opencode-serve.log 2>&1 &
srv=$!
trap 'kill -TERM "$srv" 2>/dev/null' EXIT

for _ in $(seq 1 30); do
    curl -sf "$BASE/session" >/dev/null 2>&1 && break
    sleep 1
done

sid=$(curl -s -X POST "$BASE/session" -H 'content-type: application/json' -d '{}' |
    python3 -c "import json,sys; print(json.load(sys.stdin).get('id',''))" 2>/dev/null)
echo "created ${sid:-FAIL}"
sleep 2
[ -n "$sid" ] && curl -s -o /dev/null -w 'delete %{http_code}\n' -X DELETE "$BASE/session/$sid"
sleep 3
