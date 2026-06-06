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

# Spaces close-ups — three consistently-sized 800×620 crops from the day render
# (reuses the already-rendered day.png; no second full render needed).
# crop=W:H:X:Y in ffmpeg notation. X:Y chosen so no crop catches the legend HUD
# (top-left, y<110) or the status bar (bottom, y>1408), and frame edges land in
# walkway gaps rather than slicing desks/agents.
echo "crop space: cubicles"
ffmpeg -loglevel error -y -i "$out/day.png" -vf "crop=800:620:680:430" "$out/space_cubicles.png"
echo "crop space: meeting"
ffmpeg -loglevel error -y -i "$out/day.png" -vf "crop=800:620:0:415" "$out/space_meeting.png"
echo "crop space: pantry"
ffmpeg -loglevel error -y -i "$out/day.png" -vf "crop=800:620:0:775" "$out/space_pantry.png"

# Weather shots — one per weather at a fixed afternoon hour, so the only thing
# that changes between chips is the sky/window (the showcase's weather channel in
# showcase.json swaps between these via its variant-set chips). --weather forces
# the variant, bypassing the 10-min clock cycle. Driven by weather.json (the same
# manifest the showcase + astro guard read), so a new weather renders here
# automatically — no hardcoded list to drift.
weather_hour=15
weather_manifest="$site/src/weather.json"
wids="$(node -e "require('$weather_manifest').forEach(function (w) { console.log(w.id); })")" ||
  {
    echo "failed to read weather ids from $weather_manifest" >&2
    exit 1
  }
while IFS= read -r w; do
  [ -n "$w" ] || continue
  echo "render weather: $w"
  "$bin" --cols "$cols" --rows "$rows" --weather "$w" --now-hour "$weather_hour" "$out/weather_$w.png"
done <<<"$wids"

# Animated hero clip — a 20s loop of the office mid-work (agents typing + wandering).
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
"$bin" --cols 208 --rows 88 --gif --gif-duration 20 --gif-fps 12 --now-hour "$hero_hour" "$tmp/hero.gif"
# gif → mp4 + webm + poster, re-encoded from frames so it's a true loop at the
# given fps (the GIF's own frame delays otherwise confuse ffmpeg into a fast clip).
encode_clip() { # encode_clip <gif> <id> <fps> — writes to $out (caller scope)
  local gif="$1" id="$2" fps="$3"
  mkdir -p "$tmp/frames-$id"
  ffmpeg -loglevel error -y -i "$gif" "$tmp/frames-$id/f%04d.png"
  ffmpeg -loglevel error -y -framerate "$fps" -i "$tmp/frames-$id/f%04d.png" -movflags +faststart \
    -pix_fmt yuv420p -vf "scale=trunc(iw/2)*2:trunc(ih/2)*2" "$out/$id.mp4"
  ffmpeg -loglevel error -y -framerate "$fps" -i "$tmp/frames-$id/f%04d.png" \
    -c:v libvpx-vp9 -b:v 0 -crf 36 -row-mt 1 \
    -pix_fmt yuv420p -vf "scale=trunc(iw/2)*2:trunc(ih/2)*2" "$out/$id.webm"
  ffmpeg -loglevel error -y -i "$gif" -vframes 1 "$out/$id-poster.png"
}

encode_clip "$tmp/hero.gif" hero 12

# Multi-floor clip — the real TuiRenderer slide (22 agents overflow a full
# 16-desk floor onto floor 2).
"$bin" --cols 208 --rows 88 --gif --gif-duration 10 --gif-fps 15 --now-hour "$hero_hour" \
  --max-desks 16 --agents 22 --navigate-at 3:1 --navigate-at 7:0 "$tmp/multi-floor.gif"
encode_clip "$tmp/multi-floor.gif" multi-floor 15

# Pets clip — the real TuiRenderer pet (cat roams the floor, naps near idle agents).
"$bin" --cols 208 --rows 88 --gif --gif-duration 12 --gif-fps 15 --now-hour 14 \
  --pets cat "$tmp/pets.gif"
encode_clip "$tmp/pets.gif" pets 15

echo "✓ demos regenerated → $out"
