#!/bin/bash
# Render an N-frame walk through the real cutaway painter, in the scratch
# render worktree, then put that worktree back exactly as it was.
# Usage: preview.sh PET ANIM FRAMES_DIR FRAME_MS OUT_DIR
set -euo pipefail
R=/Users/navepnow/Desktop/pixtuoid.nosync/.claude/worktrees/petrender
S=/private/tmp/claude-501/-Users-navepnow-Desktop-pixtuoid-nosync/c3372352-26a1-4356-bb82-c52383d069de/scratchpad/critters
pet=$1
anim=$2
frames=$3
ms=$4
out=$5
base=${6:-}
restore() {
	git -C "$R" checkout -q -- crates/pixtuoid-scene
	git -C "$R" clean -qfd -- crates/pixtuoid-scene/sprites
}
trap restore EXIT
/Users/navepnow/Desktop/pixtuoid.nosync/.venv/bin/python3 "$S/blender/install_walk.py" \
	"$R/crates/pixtuoid-scene/sprites/default" "$anim" "$frames" "$ms" ${base:+"$base"}
cat "$S/walk_harness.rs" >>"$R/crates/pixtuoid-scene/src/cutaway/paint.rs"
mkdir -p "$out"
rm -f "$out"/f*.ppm
(cd "$R" && env ${STILL:+SCRATCH_STILL="$anim"} SCRATCH_SCALE="${SCALE:-4}" SCRATCH_PET="$pet" SCRATCH_DIR="$out" SCRATCH_STEP_MS=125 SCRATCH_STEPS="${STEPS:-32}" \
	cargo test -q -p pixtuoid-scene --lib scratch_walk -- --ignored 2>&1 |
	grep -E "^error|panicked" -A8 | head -12) || true
find "$out" -name "f*.ppm" | wc -l
