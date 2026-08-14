#!/usr/bin/env bash
# Record a conformance fixture from the bytes a real agent CLI actually sent.
#
# A composed fixture pins whatever its author believed the wire looked like, and
# the decoder then agrees with it: kimi's per-call id decoded as None for the
# whole source's life because fixture and decoder shared one wrong field name.
# So these are recorded, never composed.
#
# ⚠ BILLED — runs one real model turn on that provider's account, and repoints
#   that source's installed hook at the recorder until it exits.
#
#   just capture-fixture kimi tool-run kimi -p '{prompt}'
#   just capture-fixture cursor tool-run cursor-agent -p --trust '{prompt}'
#   just capture-fixture kimi permission-flow "$SHELL"   # then drive the TUI yourself and exit
#
# The CLI invocation is yours to pass, not a table this script keeps: a copy of
# ten CLIs' flags would drift silently, and a drifted row captures the wrong
# thing while still looking like evidence.
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
HOOK="$REPO/target/release/pixtuoid-hook"
PIX="$REPO/target/release/pixtuoid"
for b in "$HOOK" "$PIX"; do
    [ -x "$b" ] || {
        echo "missing $b — run: just build --release" >&2
        exit 2
    }
done
command -v jq >/dev/null || {
    echo "jq is required — run: just setup-tools" >&2
    exit 2
}

was_connected="$("$PIX" sources --json | jq -r --arg i "$id" '.[] | select(.id == $i) | .connected')"
[ -n "$was_connected" ] || {
    echo "no source '$id' — see: pixtuoid sources" >&2
    exit 2
}

dest="$REPO/crates/pixtuoid-core/tests/sources/fixtures/$id/$scenario"
out="$dest/hook-payloads.jsonl"
# A re-record of an existing scenario is drift EVIDENCE, so it lands beside the
# original to be diffed rather than overwriting a committed, redacted capture.
if [ -e "$out" ]; then out="$out.new"; fi

# A FIXED generic path, because every payload embeds its own cwd and
# transcript_path: capturing somewhere already generic is what lets the bytes
# ship unedited. It is world-writable and predictable and the CLI is about to
# execute the shim we put there, so the `rm -rf` under `set -e` is load-bearing.
SB=/tmp/pixtuoid-capture
RAWD="$SB/raw"
WS="$SB/proj"
rm -rf "$SB"
mkdir -p "$SB/bin" "$RAWD" "$WS"
# Put the source's hook back the way a normal install writes it. Not a config
# edit of our own: `connect` is the install authority, and the pre-state comes
# from `sources --json`.
restore() {
    if [ "$was_connected" = true ]; then
        "$PIX" connect "$id" >/dev/null || echo "RESTORE FAILED — run: pixtuoid connect $id" >&2
    else
        "$PIX" disconnect "$id" >/dev/null || echo "RESTORE FAILED — run: pixtuoid disconnect $id" >&2
    fi
}
# The executable always goes; a BILLED capture survives a failed run. The signal
# traps exit rather than cleaning up, so cleanup runs once, from EXIT.
ok=""
cleanup() {
    rm -rf "${SB:?}/bin"
    restore
    if [ -n "$ok" ]; then rm -rf "${SB:?}"; else echo "raw capture kept: $RAWD" >&2; fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

printf 'pong\n' >"$WS/NOTE.txt"
git init -q "$WS"
git -C "$WS" add -A
git -C "$WS" -c user.email=fixture@pixtuoid -c user.name=fixture commit -qm init

# The recording shim: one dir per invocation, claimed with an atomic mkdir.
# Hooks for interleaved tools run concurrently, and a shared append can interleave
# a payload large enough to split across writes.
cat >"$SB/bin/pixtuoid-hook" <<SHIM
#!/usr/bin/env bash
n=0
while ! mkdir "$RAWD/\$n" 2>/dev/null; do n=\$((n + 1)); done
tee "$RAWD/\$n/payload.json" | "$HOOK" "\$@"
SHIM
chmod +x "$SB/bin/pixtuoid-hook"
# Every source but Claude embeds the hook's ABSOLUTE path in the CLI's own config
# (`BinaryStrategy::EmbedAbsolute`), so a shim on PATH is never consulted. The
# hook is repointed at the recorder through `PIXTUOID_HOOK`, the override the
# installer embeds, and this run owns the CLI's real config until `restore`.
PIXTUOID_HOOK="$SB/bin/pixtuoid-hook" "$PIX" connect "$id" >/dev/null

# Claimed slots, counted the same way the harvest walks them, so the two cannot
# disagree about where the capture ends.
count() {
    local n=0
    while [ -d "$RAWD/$n" ]; do n=$((n + 1)); done
    echo "$n"
}

echo "capturing $id/$scenario — one real model turn; $id's hook points at the recorder until this exits"
rc=0
(cd "$WS" && "${cmd[@]}") || rc=$?

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

# Production's shim stamps `_pixtuoid_source` downstream of the tee above, so the
# recorder stamps it here rather than leaving it to a hand edit. jq also validates
# each payload and flattens it to the one line JSONL wants.
n=0
while [ -f "$RAWD/$n/payload.json" ]; do
    jq -c --arg s "$id" '. + {_pixtuoid_source: $s}' "$RAWD/$n/payload.json"
    n=$((n + 1))
done >"$SB/harvest.jsonl"

if [ ! -s "$SB/harvest.jsonl" ]; then
    echo "captured nothing — the CLI fired no hook at all; check the invocation ran a real turn" >&2
    exit 1
fi
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

ok=1
echo "wrote $out ($n payloads, CLI exit $rc) + $prov"

claimed="$(count)"
if [ "$n" -ne "$claimed" ]; then
    echo "WARNING: $claimed hook invocations claimed a slot but only $n wrote a payload" >&2
fi

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
