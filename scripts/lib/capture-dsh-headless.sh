#!/usr/bin/env bash
# Drive a no-auth dsh headless run for `just capture-fixture` (dsh source).
#
# The recorder needs REAL plugin wire without a DeepSeek credential: a headless
# run creates the session, emits model/session_start through the plugin, then
# dies on MISSING_CREDENTIAL before any billed call — session_end still fires
# through the disposer. DSH_HOME is a fixed generic temp dir so every captured
# path ships unredacted; the plugin is rendered from the repo template with
# the release shim baked (the shim honors the recorder's PIXTUOID_SOCKET).
set -uo pipefail

repo="$(cd "$(dirname "$0")/../.." && pwd)"
export DSH_HOME=/tmp/pixtuoid-capture/dsh-home
rm -rf "$DSH_HOME"
mkdir -p "$DSH_HOME/pixtuoid"

python3 - "$repo" <<'PY'
import json, pathlib, sys
repo = pathlib.Path(sys.argv[1])
template = (repo / "crates/pixtuoid/src/install/dsh_plugin.mjs").read_text()
shim = repo / "target/release/pixtuoid-hook"
rendered = template.replace('"{{HOOK_PATH_JSON}}"', json.dumps(str(shim)))
pathlib.Path("/tmp/pixtuoid-capture/dsh-home/pixtuoid/pixtuoid-dsh.mjs").write_text(rendered)
PY

cat >"$DSH_HOME/cordis.patch.yml" <<'EOF'
- insert:
    - id: pixtuoid
      name: /tmp/pixtuoid-capture/dsh-home/pixtuoid/pixtuoid-dsh.mjs
EOF

# The credential failure is this run's EXPECTED exit; the lifecycle events all
# precede it. Bounded in case a configured machine starts a real model call.
dsh --profile headless "say hi" &
pid=$!
for _ in $(seq 1 45); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 1
done
kill -TERM "$pid" 2>/dev/null
wait "$pid" 2>/dev/null
exit 0
