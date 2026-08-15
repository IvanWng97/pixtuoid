#!/usr/bin/env bash
# Record a conformance fixture from the bytes a real agent CLI actually sent.
#
# A composed fixture pins whatever its author believed the wire looked like, and
# the decoder then agrees with it: kimi's per-call id decoded as None for the
# whole source's life because fixture and decoder shared one wrong field name.
# So these are recorded, never composed.
#
# ⚠ BILLED — runs one real model turn on that provider's account.
#
#   just capture-fixture kimi tool-run kimi -p '{prompt}'
#   just capture-fixture cursor tool-run cursor-agent -p --trust '{prompt}'
#   just capture-fixture codewhale tool-run "$SHELL"   # then drive the TUI yourself and exit
#
# The CLI invocation is yours to pass, not a table this script keeps: a copy of
# ten CLIs' flags would drift silently, and a drifted row captures the wrong
# thing while still looking like evidence.
#
# It records at the SHIM'S OUTPUT, by pointing `PIXTUOID_SOCKET` at a listener of
# our own — the one seam that does not care how the payload reached the shim.
# Recording its INPUT cannot cover every source: codewhale is env-mode (identity
# in `DEEPSEEK_*` vars, and the shim never touches stdin when `--event` is
# present), so a stdin tee captures a file of empty payloads. This side is also
# what production's reducer actually receives, stamps included, and it needs no
# edit to the CLI's installed config — the hook it already has is the one that
# runs.
set -euo pipefail

[ $# -ge 3 ] || {
    echo "usage: capture-fixture <source-id> <scenario> <cmd...>   ('{prompt}' expands)" >&2
    exit 2
}
id=$1 scenario=$2
shift 2

# One prompt for every source, so captures stay comparable: reading the SAME file
# twice around a list forces both shapes the composed fixtures got wrong — tools
# that interleave, and a tool id that repeats.
PROMPT='Read NOTE.txt, then list this directory, then read NOTE.txt again.'
orig_cmd="$*"
cmd=()
for a in "$@"; do cmd+=("${a//\{prompt\}/$PROMPT}"); done

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PIX="$REPO/target/release/pixtuoid"
[ -x "$PIX" ] || {
    echo "missing $PIX — run: just build --release" >&2
    exit 2
}
for b in jq python3; do
    command -v "$b" >/dev/null || {
        echo "$b is required — run: just setup-tools" >&2
        exit 2
    }
done

# The CLI's OWN installed hook is what runs, so a disconnected source would spend
# the turn and capture nothing.
connected="$("$PIX" sources --json | jq -r --arg i "$id" '.[] | select(.id == $i) | .connected')"
case "$connected" in
true) ;;
false)
    echo "'$id' is not connected — run: pixtuoid connect $id" >&2
    exit 2
    ;;
*)
    echo "no source '$id' — see: pixtuoid sources" >&2
    exit 2
    ;;
esac

dest="$REPO/crates/pixtuoid-core/tests/sources/fixtures/$id/$scenario"
out="$dest/hook-payloads.jsonl"
# A re-record of an existing scenario is drift EVIDENCE, so it lands beside the
# original to be diffed rather than overwriting a committed, redacted capture.
if [ -e "$out" ]; then out="$out.new"; fi

# A FIXED generic path, because every payload embeds its own cwd and
# transcript_path: capturing somewhere already generic is what lets the bytes
# ship unedited.
SB=/tmp/pixtuoid-capture
WS="$SB/proj"
RAW="$SB/captured.jsonl"
SOCK="$SB/capture.sock"
rm -rf "$SB"
mkdir -p "$WS"
: >"$RAW"

listener=""
cleanup() {
    [ -z "$listener" ] || kill "$listener" 2>/dev/null || true
    rm -rf "${SB:?}"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

printf 'pong\n' >"$WS/NOTE.txt"
git init -q "$WS"
git -C "$WS" add -A
git -C "$WS" -c user.email=fixture@pixtuoid -c user.name=fixture commit -qm init

# One connection per event (`transport::send_line` connects, writes one line,
# closes), so a whole payload arrives per accept and cannot interleave with
# another — no locking, and accept order IS wire order.
python3 - "$SOCK" "$RAW" <<'PY' &
import socket
import sys

sock, out = sys.argv[1], sys.argv[2]
srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
srv.bind(sock)
srv.listen(64)
with open(out, "ab", buffering=0) as f:
    while True:
        conn, _ = srv.accept()
        buf = b""
        while chunk := conn.recv(65536):
            buf += chunk
        conn.close()
        if buf:
            f.write(buf if buf.endswith(b"\n") else buf + b"\n")
PY
listener=$!
until [ -S "$SOCK" ]; do sleep 0.1; done

count() { grep -c . "$RAW" || true; }

echo "capturing $id/$scenario — one real model turn"
rc=0
(cd "$WS" && env PIXTUOID_SOCKET="$SOCK" "${cmd[@]}") || rc=$?

# A hook can still be in flight when the CLI's own process exits, so wait for a
# quiet period measured from the LAST PAYLOAD rather than from that exit.
SETTLE_S=0.5
QUIET_ROUNDS=4
MAX_ROUNDS=40
prev=-1
quiet=0
round=0
while [ "$quiet" -lt "$QUIET_ROUNDS" ] && [ "$round" -lt "$MAX_ROUNDS" ]; do
    n="$(count)"
    if [ "$n" = "$prev" ]; then quiet=$((quiet + 1)); else quiet=0; fi
    prev="$n"
    round=$((round + 1))
    sleep "$SETTLE_S"
done

if [ ! -s "$RAW" ]; then
    echo "captured nothing — the CLI fired no hook at all; check the invocation ran a real turn" >&2
    exit 1
fi
# jq validates every payload and flattens it to the one line JSONL wants. The
# shim already stamped `_pixtuoid_source`, so nothing here edits the bytes.
jq -c . "$RAW" >"$SB/harvest.jsonl"
n="$(grep -c . "$SB/harvest.jsonl")"
mkdir -p "$dest"
cp "$SB/harvest.jsonl" "$out"

# The provenance the conformance gate requires. `--version` is a guess about a
# CLI whose flags this script deliberately does not model, so a failure records
# `unknown` rather than blocking a capture that already cost a turn.
ver="$("${cmd[0]}" --version </dev/null 2>/dev/null | head -1 || true)"
[ -n "$ver" ] || ver=unknown
prov="$dest/provenance.json"
case "$out" in *.new) prov="$prov.new" ;; esac
jq -n --arg cli "$(basename "${cmd[0]}")" --arg version "$ver" \
    --arg captured "$(date +%Y-%m-%d)" --arg command "$orig_cmd" \
    '{origin:"recorded", cli:$cli, version:$version, captured:$captured, command:$command}' >"$prov"

echo "wrote $out ($n payloads, CLI exit $rc) + $prov"

# PII is not always a key you can drop — kimi's arrived as the owner column
# inside a captured `ls -la`.
pii="$HOME"
if [ -n "${USER:-}" ]; then pii="$pii|$USER"; fi
if grep -qE "$pii" "$out"; then
    echo "WARNING: the capture embeds your home path or username — redact before committing" >&2
fi
# A non-zero CLI means the turn was cut short, so the capture is a PARTIAL wire.
if [ "$rc" -ne 0 ]; then
    echo "WARNING: the CLI exited $rc — this capture may be truncated" >&2
fi
if [ "$out" = "$dest/hook-payloads.jsonl" ]; then
    echo "next: just test conformance   then   cargo insta review"
else
    echo "next: diff $dest/hook-payloads.jsonl $out"
fi
