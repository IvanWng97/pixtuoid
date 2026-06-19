#!/usr/bin/env bash
# replay-fixture.sh — replay a captured Codex rollout fixture into a HERMETIC
# headless pixtuoid run (via --codex-sessions-root + a temp dir, so ~/.codex is
# untouched) and print the cx· agent's state progression. Lets you eyeball a
# source's lifecycle (e.g. permission -> resume) end-to-end without a live CLI.
#
# Usage:  scripts/replay-fixture.sh <rollout.jsonl> [delay_secs]
#   e.g.  scripts/replay-fixture.sh \
#           crates/pixtuoid-core/tests/sources/fixtures/codex/permission-flow/rollout-*.jsonl
#   PIXTUOID_BIN overrides the binary (default: `pixtuoid` on PATH).
set -euo pipefail

fixture="${1:?usage: replay-fixture.sh <rollout.jsonl> [delay_secs]}"
delay="${2:-3}"
bin="${PIXTUOID_BIN:-pixtuoid}"

[ -f "$fixture" ] || {
    echo "no such fixture: $fixture" >&2
    exit 1
}
command -v "$bin" >/dev/null 2>&1 || {
    echo "binary not found: $bin (set PIXTUOID_BIN)" >&2
    exit 1
}

root="$(mktemp -d)"
proj="$(mktemp -d)"
out="$(mktemp)"
hpid=""
cleanup() {
    if [ -n "$hpid" ]; then
        kill "$hpid" 2>/dev/null || true
        wait "$hpid" 2>/dev/null || true # reap quietly (suppress "Terminated")
    fi
    rm -rf "$root" "$proj" "$out"
    return 0
}
trap cleanup EXIT

mkdir -p "$root/replay"
# The filename's trailing UUID is the Codex session key (codex_id_from_path);
# any canonical UUID works for a replay.
file="$root/replay/rollout-2026-01-01T00-00-00-0a0a0a0a-0b0b-0c0c-0d0d-0e0e0e0e0e0e.jsonl"

"$bin" run --headless --codex-sessions-root "$root" --projects-root "$proj" \
    --log-level error >"$out" 2>&1 &
hpid=$!
sleep 2 # let the watcher bind/seed before the first append

echo "replaying $(basename "$fixture") (1 line / ${delay}s) into a hermetic headless run..." >&2
# `|| [ -n "$line" ]` so a final line without a trailing newline is still processed.
while IFS= read -r line || [ -n "$line" ]; do
    [ -z "$line" ] && continue
    printf '%s\n' "$line" >>"$file"
    sleep "$delay"
done <"$fixture"
sleep 2

echo "=== cx· agent state progression ==="
grep 'agents=' "$out" || true
# Headless always prints `agents=[]` (empty scene), so success = at least one
# NON-empty agents line ever appeared. `agents=\[[^]]` = a char after `[` other than `]`.
if ! grep -qE 'agents=\[[^]]' "$out"; then
    echo "(no cx· agent ever appeared — is '$bin' the codex-aware build, and is the fixture a codex rollout?)"
fi
