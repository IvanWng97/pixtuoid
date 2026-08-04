#!/usr/bin/env bash
# replay-fixture.sh — replay a captured Codex rollout fixture into a headless
# pixtuoid run and print the cx· agent's state progression, without a live CLI.
# All FOUR host couplings are isolated: sessions root, projects root, config
# (XDG_CONFIG_HOME) and the hook socket.
#
# Usage:  scripts/replay-fixture.sh <rollout.jsonl> [delay_secs]
#   e.g.  scripts/replay-fixture.sh \
#           crates/pixtuoid-core/tests/sources/fixtures/codex/permission-flow/rollout-*.jsonl
#   PIXTUOID_BIN overrides the binary (default: this tree's target/release build).
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixture="${1:?usage: replay-fixture.sh <rollout.jsonl> [delay_secs]}"
delay="${2:-3}"
# The WORKING TREE's binary, never a bare `pixtuoid` off PATH: a maintainer
# normally has a RELEASED pixtuoid installed, so a PATH default would replay the
# last release and print PASS with none of the change under test in it.
bin="${PIXTUOID_BIN:-$repo/target/release/pixtuoid}"

[ -f "$fixture" ] || {
    echo "no such fixture: $fixture" >&2
    exit 1
}
command -v "$bin" >/dev/null 2>&1 || {
    echo "binary not found: $bin — run: just build --release (or set PIXTUOID_BIN)" >&2
    exit 2
}

root="$(mktemp -d)"
proj="$(mktemp -d)"
cfgdir="$(mktemp -d)"
# The socket lives inside a PRIVATE 0700 dir, not as a bare `mktemp -u` name in
# the shared temp dir, which would leave a pre-plant/symlink race between name
# generation and pixtuoid's bind(). Nothing downstream closes it —
# `ensure_owned_socket_dir` does not police an explicit PIXTUOID_SOCKET path.
sockdir="$(mktemp -d)"
sock="$sockdir/hook.sock"
out="$(mktemp)"
hpid=""
cleanup() {
    if [ -n "$hpid" ]; then
        kill "$hpid" 2>/dev/null || true
        wait "$hpid" 2>/dev/null || true
    fi
    rm -rf "$root" "$proj" "$cfgdir" "$sockdir" "$out"
    return 0
}
trap cleanup EXIT

# An ISOLATED config marking Codex connected. `resolve_connected` treats a missing
# [sources] key as DISCONNECTED and the driver drops a disconnected source's events
# ahead of the reducer, so without this a box that never connected Codex replays
# into zero agents — silently, since the drop sits above the gate's own log line.
mkdir -p "$cfgdir/pixtuoid"
printf '[sources]\ncodex = true\n' >"$cfgdir/pixtuoid/config.toml"

mkdir -p "$root/replay"
# The filename's trailing UUID is the Codex session key (codex_id_from_path); any
# canonical UUID works for a replay.
file="$root/replay/rollout-2026-01-01T00-00-00-0a0a0a0a-0b0b-0c0c-0d0d-0e0e0e0e0e0e.jsonl"

# The isolated socket matters for the ASSERTION, not just for hygiene: on the
# default socket a live CC session's hook traffic lands in this run's scene and
# satisfies the success grep, so a totally broken Codex path would report PASS.
XDG_CONFIG_HOME="$cfgdir" PIXTUOID_SOCKET="$sock" \
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
# Success requires a STATE TRANSITION, not merely a registered sprite: junk content
# still registers `agents=[cx@0:idle]`, so a bare "a non-empty scene appeared"
# predicate reports PASS for a decoder that has stopped decoding entirely. Exits
# non-zero so a replay that produced nothing fails its caller.
if ! grep -qE 'agents=\[cx·[^]]*:(active|waiting)' "$out"; then
    echo "FAIL: the fixture never drove a cx· agent into active/waiting — is '$bin'" >&2
    echo "  the codex-aware build, and is the fixture a codex rollout?" >&2
    exit 1
fi
echo "PASS: the fixture drove a cx· agent through a real state transition."
