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
# ⚠ Verify a capture against an INDEPENDENT observer before trusting it. cursor
#   invokes the pixtuoid hook FOUR times per event while this listener is the
#   endpoint, and once when its own unrelated debug tee is watching the same run —
#   an observer effect this recorder has not explained, so cursor's fixtures are
#   not captured this way. Nothing else shows it (opencode 10 unique of 10 lines;
#   codewhale's repeats are its minimal envelope, not duplicates).
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
# that interleave, and a tool id that repeats. `CAPTURE_PROMPT` overrides it for a
# scenario the shared one cannot reach — a permission gate needs a tool the CLI
# refuses to run unasked.
PROMPT="${CAPTURE_PROMPT:-Read NOTE.txt, then list this directory, then read NOTE.txt again.}"
orig_cmd="$*"
cmd=()
for a in "$@"; do cmd+=("${a//\{prompt\}/$PROMPT}"); done

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PIX="$REPO/target/release/pixtuoid"
ROSTER="$REPO/target/release/examples/corpus_check"
for b in "$PIX" "$ROSTER"; do
    [ -x "$b" ] || {
        echo "missing $b — run: just build --release --examples" >&2
        exit 2
    }
done
for b in jq python3; do
    command -v "$b" >/dev/null || {
        echo "$b is required — run: just setup-tools" >&2
        exit 2
    }
done

# A transcript-bearing source's evidence is the FILE the CLI writes, not a hook —
# omp ships no shell-hook target at all — so the two are captured differently and
# the roster, not a list here, decides which.
root=""
if root="$("$ROSTER" --root "$id" 2>/dev/null)"; then
    kind=transcript
else
    kind=hook
    root=""
fi

# A hook capture rides the CLI's OWN installed hook, so a disconnected source
# would spend the turn and capture nothing.
connected="$("$PIX" sources --json | jq -r --arg i "$id" '.[] | select(.id == $i) | .connected')"
case "$connected:$kind" in
true:* | *:transcript) ;;
false:*)
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
# A transcript keeps the CLI's own filename, so it cannot land beside a committed
# one: the harness requires exactly one transcript per scenario dir.
if [ "$kind" = transcript ] && [ -n "$(find "$dest" -name '*.jsonl' ! -name hook-payloads.jsonl 2>/dev/null)" ]; then
    echo "$dest already holds a transcript — pick a new scenario name" >&2
    exit 2
fi

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

STARTED="$SB/started"
touch "$STARTED"
printf 'pong\n' >"$WS/NOTE.txt"
# A gate is a per-CLI RULE, and it belongs in the sandbox: opencode auto-approves
# a trusted workspace, and CC does not register `PermissionRequest` at all. Copied
# in declaratively rather than run, so a seed cannot become a second capture path.
[ -z "${CAPTURE_SEED:-}" ] || cp -R "$CAPTURE_SEED"/. "$WS"/
git init -q "$WS"
git -C "$WS" add -A
git -C "$WS" -c user.email=fixture@pixtuoid -c user.name=fixture commit -qm init

# One connection per event (`transport::send_line` connects, writes one line,
# closes), so a whole payload arrives per accept and cannot interleave with
# another. THREADED, and that is not an optimization: the shim write-times-out
# under a watchdog, and a CLI whose hook then fails RETRIES it — a serial
# listener made cursor fire the same `tool_use_id` four times, which its own
# unrelated debug tee (one line, same run) proved was the recorder's doing.
python3 - "$SOCK" "$RAW" <<'PY' &
import socketserver
import sys
import threading

sock, out = sys.argv[1], sys.argv[2]
lock = threading.Lock()
f = open(out, "ab", buffering=0)


class Handler(socketserver.BaseRequestHandler):
    def handle(self):
        buf = b""
        while chunk := self.request.recv(65536):
            buf += chunk
        if buf:
            with lock:
                f.write(buf if buf.endswith(b"\n") else buf + b"\n")


class Server(socketserver.ThreadingUnixStreamServer):
    daemon_threads = True
    request_queue_size = 64


Server(sock, Handler).serve_forever()
PY
listener=$!
until [ -S "$SOCK" ]; do sleep 0.1; done

count() { grep -c . "$RAW" || true; }

# The capture is launched from inside an agent session, and that agent's own env
# reaches the CLI under test: a nested `claude` inherited CLAUDE_CODE_CHILD_SESSION
# and turned its transcript saving OFF, so the run left nothing to harvest. Scrub
# the whole namespace rather than the one variable that bit.
scrub=()
while IFS='=' read -r k _; do
    case "$k" in CLAUDECODE | CLAUDE_CODE_*) scrub+=(-u "$k") ;; esac
done < <(env)

echo "capturing $id/$scenario — one real model turn"
rc=0
(cd "$WS" && env "${scrub[@]}" PIXTUOID_SOCKET="$SOCK" "${cmd[@]}") || rc=$?

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

mkdir -p "$dest"
if [ "$kind" = transcript ]; then
    # The transcript this run CREATED: birth time, not mtime. The developer's own
    # Claude Code session is appended to continuously and is therefore always the
    # newest by mtime — this harvest picked it up once, 4155 lines of it, and only
    # the PII warning caught it before a commit. A capture is a NEW file.
    # `--list` is the source's OWN path filter, not a guess: grok writes five
    # jsonl siblings per session and the one-line `rewind_points.jsonl` wins on
    # birth time, while the transcript it actually tails is `updates.jsonl`.
    born_after="$(stat -f %B "$STARTED")"
    fresh="$("$ROSTER" --list "$id" "$root" 2>/dev/null |
        while IFS= read -r f; do printf '%s %s\n' "$(stat -f %B "$f")" "$f"; done |
        awk -v t="$born_after" '$1 >= t' | sort -rn | head -1 | cut -d' ' -f2-)"
    if [ -z "$fresh" ]; then
        echo "captured nothing — no new transcript under $root; did the turn run?" >&2
        exit 1
    fi
    # A source whose transcripts all share ONE basename keys its session on the
    # PARENT dir, so flattening the capture would rename the session after the
    # scenario. Asking `--list` whether the name repeats beats knowing which
    # sources those are.
    base="$(basename "$fresh")"
    if [ "$("$ROSTER" --list "$id" "$root" 2>/dev/null | grep -c "/$base\$")" -gt 1 ]; then
        out="$dest/$(basename "$(dirname "$fresh")")/$base"
        mkdir -p "$(dirname "$out")"
    else
        out="$dest/$base"
    fi
    cp "$fresh" "$out"
    n="$(grep -c . "$out")"
    # A source can be BOTH. CC's tool run is in the transcript but its permission
    # gate is a hook event, so keeping only the transcript threw away the very
    # bytes the gate scenario exists for.
    if [ -s "$RAW" ]; then
        hooks_out="$dest/hook-payloads.jsonl"
        [ ! -e "$hooks_out" ] || hooks_out="$hooks_out.new"
        jq -c . "$RAW" >"$hooks_out"
    fi
else
    if [ ! -s "$RAW" ]; then
        echo "captured nothing — the CLI fired no hook at all; check the invocation ran a real turn" >&2
        exit 1
    fi
    # jq validates every payload and flattens it to the one line JSONL wants. The
    # shim already stamped `_pixtuoid_source`, so nothing here edits the bytes.
    jq -c . "$RAW" >"$SB/harvest.jsonl"
    n="$(grep -c . "$SB/harvest.jsonl")"
    cp "$SB/harvest.jsonl" "$out"
fi

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
[ -z "${hooks_out:-}" ] || echo "also wrote $hooks_out ($(grep -c . "$hooks_out") hook payloads)"

# An interactive capture runs a SHELL, so the probe above identifies the shell and
# not the CLI under test — say so rather than let a wrong `cli`/`version` sit in a
# file whose whole job is provenance.
case "$(basename "${cmd[0]}")" in
*sh)
    echo "NOTE: $prov names $(basename "${cmd[0]}") — an interactive capture cannot probe the CLI; fix cli/version/command by hand" >&2
    ;;
esac

# PII is not always a key you can drop — kimi's arrived as the owner column
# inside a captured `ls -la`.
pii="$HOME"
if [ -n "${USER:-}" ]; then pii="$pii|$USER"; fi
if grep -qE "$pii" "$out" "${hooks_out:-$out}"; then
    echo "WARNING: the capture embeds your home path or username — redact before committing" >&2
fi
# A non-zero CLI means the turn was cut short, so the capture is a PARTIAL wire.
if [ "$rc" -ne 0 ]; then
    echo "WARNING: the CLI exited $rc — this capture may be truncated" >&2
fi
case "$out" in
*.new) echo "next: diff ${out%.new} $out" ;;
*) echo "next: just test conformance   then   cargo insta review" ;;
esac
