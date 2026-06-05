#!/usr/bin/env bash
# Regenerate the website's demo art from the pixtuoid binary — the single source
# of truth for site/public/demos/*. Re-run after changing the office's look OR
# adding a theme (also add the theme to site/src/themes.json and it renders here).
#
# Requires: ffmpeg + node. Builds the release `snapshot` example if it's missing.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
site="$root/site"
out="$site/public/demos"
bin="$root/target/release/examples/snapshot"
manifest="$site/src/themes.json"

cols=200
rows=90
hero_hour=17

mkdir -p "$out"
# Always (re)build so a stale binary can't render outdated art — cargo no-ops when
# nothing changed (gating on `[ -x "$bin" ]` silently reused a stale binary).
(cd "$root" && cargo build --release --example snapshot)

# Per-theme switcher shots — driven by the manifest (add a theme there → renders here).
# Materialize the ids first with an explicit error check: a `< <(node …)` process
# substitution would swallow node's exit code even under pipefail, silently
# rendering zero themes. An unknown --theme then aborts loudly via set -e.
ids="$(node -e "require('$manifest').forEach(function (t) { console.log(t.id); })")" ||
  {
    echo "failed to read theme ids from $manifest" >&2
    exit 1
  }
while IFS= read -r id; do
  [ -n "$id" ] || continue
  echo "render theme: $id"
  "$bin" --cols "$cols" --rows "$rows" --theme "$id" --now-hour 20 "$out/theme_$id.png"
done <<<"$ids"

# Day / night (normal theme) for the day-night feature.
"$bin" --cols "$cols" --rows "$rows" --now-hour 13 "$out/day.png"
"$bin" --cols "$cols" --rows "$rows" --now-hour 22 "$out/night.png"

# Animated hero → mp4 + poster: a 20s loop of the office mid-work — multiple
# agents typing and wandering (to the pantry, the meeting room, the couch).
# Re-encode from frames so it's a true 20s/12fps loop (the GIF's own frame
# delays otherwise confuse ffmpeg into a fast clip).
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
"$bin" --cols 208 --rows 88 --gif --gif-duration 20 --gif-fps 12 --now-hour "$hero_hour" "$tmp/hero.gif"
mkdir -p "$tmp/frames"
# -loglevel error: quiet on success, but let a real ffmpeg failure surface its
# reason on stderr (set -e aborts the script either way).
ffmpeg -loglevel error -y -i "$tmp/hero.gif" "$tmp/frames/f%03d.png"
ffmpeg -loglevel error -y -framerate 12 -i "$tmp/frames/f%03d.png" -movflags +faststart -pix_fmt yuv420p \
  -vf "scale=trunc(iw/2)*2:trunc(ih/2)*2" "$out/hero.mp4"
ffmpeg -loglevel error -y -i "$tmp/hero.gif" -vframes 1 "$out/hero-poster.png"

echo "✓ demos regenerated → $out"
