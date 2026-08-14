#!/usr/bin/env bash
# Record a conformance fixture from the bytes a real agent CLI actually sent.
#
# Every hook-only source's fixture in this tree was hand-written, and at least one
# is provably wrong: cursor/tool-run carries no `tool_use_id` and strictly
# sequential tools, while #901 established from a real capture that every
# preToolUse carries an id and that tools INTERLEAVE. A fixture that encodes the
# author's model instead of the wire teaches the next reader the wrong shape, so
# these are recorded, never composed.
#
# ⚠ BILLED — one real model turn on that provider's account.
#
# Run:  just capture-fixture <source-id> <scenario> ["prompt"]
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PIX="$REPO/target/release/pixtuoid"
HOOK="$REPO/target/release/pixtuoid-hook"
ROSTER="$REPO/target/release/examples/corpus_check"
for b in "$PIX" "$HOOK" "$ROSTER"; do
    [ -x "$b" ] || {
        echo "missing $b — run: just build --release --bins --examples" >&2
        exit 2
    }
done

id="${1:?usage: capture-fixture <source-id> <scenario> [prompt]}"
scenario="${2:?usage: capture-fixture <source-id> <scenario> [prompt]}"
PROMPT="${3:-Read NOTE.txt, then list this directory, then read NOTE.txt again.}"

# Per-CLI headless invocation. Every entry was checked against that CLI's own
# --help; an unverified guess here would produce a capture of the wrong thing.
case "$id" in
claude-code) bin=claude sub=-p extra="--allowedTools Read Bash" ;;
codex) bin=codex sub=exec extra="" ;;
antigravity) bin=agy sub=-p extra="" ;;
reasonix) bin=reasonix sub=-p extra="" ;;
codewhale) bin=codewhale sub=exec extra="" ;;
opencode) bin=opencode sub=run extra="" ;;
copilot) bin=copilot sub=-p extra="" ;;
    # --trust, not --yolo: the sandbox holds one file, and the narrower flag is
    # the one cursor-agent's own refusal message offers.
cursor) bin=cursor-agent sub=-p extra="--trust" ;;
hermes) bin=hermes sub=-z extra="" ;;
grok) bin=grok sub=-p extra="" ;;
*)
    echo "no verified headless invocation for '$id' — check its --help and add one" >&2
    exit 2
    ;;
esac
[ -d "$HOME/.local/bin" ] && PATH="$PATH:$HOME/.local/bin"
command -v "$bin" >/dev/null 2>&1 || {
    echo "'$bin' is not on PATH" >&2
    exit 2
}

# A FIXED generic path, not a random temp dir: every payload embeds its cwd and
# transcript path, and redacting a random path after the fact mangles them
# (macOS's /private prefix, dash-encoded project dirs). Capturing somewhere
# already generic means the bytes need no editing — which is the whole point.
SB=/tmp/pixtuoid-capture
WS="$SB/proj"
RAW="$SB/captured.jsonl"
rm -rf "$SB"
mkdir -p "$SB/bin" "$WS"
printf 'pong\n' >"$WS/NOTE.txt"
git init -q "$WS" 2>/dev/null
git -C "$WS" add -A 2>/dev/null
git -C "$WS" -c user.email=fixture@pixtuoid -c user.name=fixture commit -qm init 2>/dev/null
trap 'rm -rf "$SB"' EXIT

# The recording shim: tee stdin to the capture, then hand the SAME bytes to the
# real shim so the session behaves normally while being observed.
cat >"$SB/bin/pixtuoid-hook" <<SHIM
#!/usr/bin/env bash
payload="\$(cat)"
printf '%s\n' "\$payload" >>"$RAW"
printf '%s\n' "\$payload" | "$HOOK" "\$@"
SHIM
chmod +x "$SB/bin/pixtuoid-hook"
PATH="$SB/bin:$PATH"
export PATH

# A relocatable source gets a throwaway home with a hook config this run wrote,
# so the capture does not depend on how the host's install happens to be wired.
home_env="$("$ROSTER" --roster | awk -F'\t' -v i="$id" '$1==i{print $4}')"
env_pair=""
if [ "$home_env" != "-" ] && [ -n "$home_env" ]; then
    mkdir -p "$SB/home"
    if env "$home_env=$SB/home" "$PIX" connect "$id" --json >/dev/null 2>&1; then
        env_pair="$home_env=$SB/home"
    fi
fi

echo "capturing $id/$scenario — one real model turn"
# shellcheck disable=SC2086  # $extra is a flag LIST and must word-split
(cd "$WS" && env ${env_pair:+"$env_pair"} "$bin" "$sub" "$PROMPT" $extra </dev/null >"$SB/cli.log" 2>&1)
rc=$?
sleep 2

if [ ! -s "$RAW" ]; then
    echo "captured nothing — is $id's hook installed and does it invoke a bare 'pixtuoid-hook'?" >&2
    sed 's/^/    /' "$SB/cli.log" | tail -8 >&2
    exit 1
fi

dest="$REPO/crates/pixtuoid-core/tests/sources/fixtures/$id/$scenario"
mkdir -p "$dest"
cp "$RAW" "$dest/hook-payloads.jsonl"
n="$(wc -l <"$dest/hook-payloads.jsonl" | tr -d ' ')"
echo "wrote $dest/hook-payloads.jsonl ($n payloads, CLI exit $rc)"

grep -q "$HOME" "$dest/hook-payloads.jsonl" &&
    echo "WARNING: the capture embeds your home path — inspect before committing" >&2
# A non-zero CLI means the turn was cut short, so the capture is a PARTIAL wire;
# committing it would pin a truncated shape as though it were the whole one.
if [ "$rc" -ne 0 ]; then
    echo "WARNING: the CLI exited $rc — this capture may be truncated:" >&2
    sed 's/^/    /' "$SB/cli.log" | tail -10 >&2
fi
echo "next: just test conformance   then   cargo insta review"
