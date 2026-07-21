#!/usr/bin/env bash
# replay-fixture.sh — replay a captured Codex rollout fixture into a HERMETIC
# headless pixtuoid run and print the cx· agent's state progression. Lets you
# eyeball a source's lifecycle (e.g. permission -> resume) end-to-end without a
# live CLI.
#
# "Hermetic" means all FOUR host couplings are isolated — sessions root, projects
# root, config (XDG_CONFIG_HOME) and the hook socket. The original wording
# claimed hermetic while only naming ~/.codex, and the two unnamed leaks are
# exactly what silently broke this script; keep the list explicit.
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
cfgdir="$(mktemp -d)"
# The socket lives inside a PRIVATE 0700 dir, not as a bare name in the shared
# temp dir: `mktemp -u` reserves a name without creating anything, leaving a
# pre-plant/symlink race between name generation and pixtuoid's bind(). Nothing
# downstream closes it — `ensure_owned_socket_dir` (hook/unix.rs) deliberately
# does not police an explicit PIXTUOID_SOCKET path. This matches what the sibling
# Rust harness (cli_json.rs::headless_replay) already does with tempfile::tempdir.
sockdir="$(mktemp -d)"
sock="$sockdir/hook.sock"
out="$(mktemp)"
hpid=""
cleanup() {
    if [ -n "$hpid" ]; then
        kill "$hpid" 2>/dev/null || true
        wait "$hpid" 2>/dev/null || true # reap quietly (suppress "Terminated")
    fi
    rm -rf "$root" "$proj" "$cfgdir" "$sockdir" "$out"
    return 0
}
trap cleanup EXIT

# An ISOLATED config marking Codex connected. `resolve_connected` treats a
# missing [sources] key as DISCONNECTED (config/mod.rs) and the driver drops a
# disconnected source's events ahead of the reducer, so without this a box that
# never connected Codex in the Sources panel replays into zero agents — and,
# because the drop sits above the gate's own log line, zero explanation too.
# openclaw-live-e2e.sh isolates its config for exactly this reason.
mkdir -p "$cfgdir/pixtuoid"
printf '[sources]\ncodex = true\n' >"$cfgdir/pixtuoid/config.toml"

mkdir -p "$root/replay"
# The filename's trailing UUID is the Codex session key (codex_id_from_path);
# any canonical UUID works for a replay.
file="$root/replay/rollout-2026-01-01T00-00-00-0a0a0a0a-0b0b-0c0c-0d0d-0e0e0e0e0e0e.jsonl"

# The isolated socket matters for the ASSERTION, not just for hygiene: on the
# default socket a live CC session's hook traffic lands in this run's scene and
# satisfies the `agents=\[[^]]` success grep, so a totally broken Codex path
# would report PASS.
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
# Success requires a STATE TRANSITION, not merely a registered sprite: junk
# content still registers `agents=[cx@0:idle]`, so a bare "a non-empty scene
# appeared" predicate reports PASS for a decoder that has stopped decoding
# entirely. The bundled permission-flow fixture guarantees both active and
# waiting, so requiring either keeps the check honest without over-fitting.
#
# This EXITS non-zero: the script is cited as a verification step, so a replay
# that produced nothing must fail its caller rather than print a note and exit 0.
if ! grep -qE 'agents=\[cx·[^]]*:(active|waiting)' "$out"; then
    echo "FAIL: the fixture never drove a cx· agent into active/waiting — is '$bin'" >&2
    echo "  the codex-aware build, and is the fixture a codex rollout?" >&2
    exit 1
fi
echo "PASS: the fixture drove a cx· agent through a real state transition."
