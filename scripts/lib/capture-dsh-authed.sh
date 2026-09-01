#!/usr/bin/env bash
# Drive an AUTHED dsh headless turn for `just capture-fixture` (dsh source).
#
# Unlike capture-dsh-headless.sh (the free, no-credential boot capture), this
# run completes a real model turn — tool rounds, approvals, usage — so it is
# BILLED. DEEPSEEK_API_KEY must be exported by the caller; it is never echoed.
# DSH_HOME stays the fixed generic dir so every captured path ships unredacted.
set -uo pipefail

if [[ -z ${DEEPSEEK_API_KEY:-} ]]; then
    echo "capture-dsh-authed: export DEEPSEEK_API_KEY first (a real, billed turn)" >&2
    exit 1
fi

repo="$(cd "$(dirname "$0")/../.." && pwd)"
export DSH_HOME=/tmp/pixtuoid-capture/dsh-home
rm -rf "$DSH_HOME"
mkdir -p "$DSH_HOME/pixtuoid"

# A missing shim would otherwise yield a silently EMPTY (still billed) capture:
# the plugin spawns it fire-and-forget with errors swallowed.
shim="$repo/target/release/pixtuoid-hook"
if [[ ! -x $shim ]]; then
    echo "capture-dsh-authed: missing $shim — run 'just build --release' first" >&2
    exit 1
fi

plugin="$DSH_HOME/pixtuoid/pixtuoid-dsh.mjs"
# Fail-loud render: a missing template or a renamed placeholder would
# otherwise yield a plugin whose spawn fails silently — a billed run
# recording zero payloads.
if ! python3 - "$repo" "$plugin" "$shim" <<'PY'
import json, pathlib, sys
repo = pathlib.Path(sys.argv[1])
template = (repo / "crates/pixtuoid/src/install/dsh_plugin.mjs").read_text()
marker = '"{{HOOK_PATH_JSON}}"'
if marker not in template:
    sys.exit("dsh_plugin.mjs lost the HOOK_PATH placeholder")
rendered = template.replace(marker, json.dumps(sys.argv[3]))
pathlib.Path(sys.argv[2]).write_text(rendered)
PY
then
    echo "capture driver: plugin render failed" >&2
    exit 1
fi

cat >"$DSH_HOME/cordis.patch.yml" <<EOF
- insert:
    - id: pixtuoid
      name: $plugin
EOF

# Bounded: a real turn with tool rounds outlives the free boot run's 45s.
dsh --profile headless "$1" &
pid=$!
for _ in $(seq 1 180); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 1
done
kill -TERM "$pid" 2>/dev/null
wait "$pid" 2>/dev/null
exit 0
