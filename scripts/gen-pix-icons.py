#!/usr/bin/env python3
"""Generate the site's pixel-icon PNGs (site/src/assets/pix-icons/) plus the
root README's pre-scaled variants (docs/images/pix-icons/).

Single color source: the embedded sprite pack's palette
(crates/pixtuoid-scene/sprites/default/pack.toml) — an icon grid may only use
keys defined there, so the icons can never drift off the office's own colors.
An icon is either extracted verbatim from a pack sprite ("sprite") or authored
here as a pixel grid ("grid"). The site's 1x RGBA PNGs are consumed by
PixIcon.astro, which integer-upscales them with image-rendering: pixelated
(upscale-crisp only). GitHub strips that CSS, so the README instead embeds a
SEPARATE, pre-scaled (nearest-neighbor, README_SCALE) variant per icon — the
only way to keep them crisp there — written to docs/images/pix-icons/
(the doc-image convention; the site's src/assets/pix-icons/ is a Vite build
input, not a docs asset).

Usage:
  .venv/bin/python3 scripts/gen-pix-icons.py          # (re)generate (just gen-icons)
  .venv/bin/python3 scripts/gen-pix-icons.py --check  # exit 1 on drift

--check decode-compares pixels (via scripts/compare-screenshots.py, like
gen-media.py's --check) rather than raw PNG bytes — a raw-byte compare is
Pillow-version-fragile (re-encoding the identical pixels can change the
compressed bytes), which would make the gate flaky across machines/CI. It also
diffs each output directory's file listing against ICONS.keys() so a PNG left
behind by a removed manifest entry (an orphan) fails loudly instead of going
unnoticed.
"""

import io
import shutil
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
PACK = ROOT / "crates/pixtuoid-scene/sprites/default"
OUT = ROOT / "site/src/assets/pix-icons"
README_OUT = ROOT / "docs/images/pix-icons"
# Nearest-neighbor upscale factor for the README variants: GitHub's markdown
# renderer strips <img> sizing/CSS, so these must be pre-scaled pixels. 5x turns
# the 10x10 (or 8x12 sprite) grids into ~40-70px images — gen-readme.mjs then
# pins each <img> to THESE dimensions (width/height from the PNG's IHDR) so the
# table column can't collapse them; bump this to resize the README icons.
README_SCALE = 5
COMPARE = ROOT / "scripts/compare-screenshots.py"
DIFF_DIR = ROOT / "target/gen-check-diff"

# Icon manifest. "sprite": extract a whole pack sprite frame verbatim.
# "grid": rows of space-separated pack-palette keys ('.' = transparent).
ICONS = {
    # the office's own walker, straight from the pack (8x12)
    "walk": {"sprite": "walking_0.sprite"},
    "coffee": {
        "grid": [
            ". . K . . K . . . .",
            ". K . . K . . . . .",
            ". . K . . K . . . .",
            ". . . . . . . . . .",
            ". V V V V V V . . .",
            ". V d d d d V V V .",
            ". V d d d d V . V .",
            ". V V V V V V V V .",
            ". . V V V V V V . .",
            ". K K K K K K K K .",
        ]
    },
    "chat": {
        "grid": [
            ". . n n n n n n . .",
            ". n w w w w w w n .",
            "n w w w w w w w w n",
            "n w q w q w q w w n",
            "n w w w w w w w w n",
            ". n w w w w w w n .",
            ". . n n w w n n . .",
            ". . . n w n . . . .",
            ". . . n n . . . . .",
            ". . . . . . . . . .",
        ]
    },
    "palette": {
        "grid": [
            ". . D D D D D D . .",
            ". D D D D D D D D .",
            "D D r r D D b b D D",
            "D D r r D D b b D D",
            "D D D D D D D D D D",
            "D D y y D D . . D D",
            "D D y y D . . . D D",
            ". D D D D . . D D .",
            ". . D D D D D D . .",
            ". . . . . . . . . .",
        ]
    },
    # per-tool monitor glow → a monitor with a light (K) outline so the dark
    # bezel doesn't dissolve into the DARK theme
    "glow": {
        "grid": [
            ". K K K K K K K K .",
            ". K M M M M M M K .",
            ". K M c c c c M K .",
            ". K M c c c c M K .",
            ". K M c c c c M K .",
            ". K M M M M M M K .",
            ". K K K K K K K K .",
            ". . . K K K K . . .",
            ". . K K K K K K . .",
            ". . . . . . . . . .",
        ]
    },
    # hover tooltips → an info "i" badge (the old magnifier read as "search /
    # zoom", not "hover for details")
    "magnify": {
        "grid": [
            ". . K K K K K K . .",
            ". K w w w w w w K .",
            ". K w w b w w w K .",
            ". K w w w w w w K .",
            ". K w b b w w w K .",
            ". K w w b w w w K .",
            ". K w w b w w w K .",
            ". K w b b b w w K .",
            ". K w w w w w w K .",
            ". . K K K K K K . .",
        ]
    },
    # token meter → a stack of sheets on a desk, the top one fanned. Dark (n)
    # side-edges so the near-white sheets separate on the LIGHT theme (they
    # vanished into cream before).
    "tokens": {
        "grid": [
            ". . . n n n . . . .",
            ". . n w w w n . . .",
            ". n w w w w w n . .",
            ". n V V V V V n . .",
            ". n w w w w w n . .",
            ". n V V V V V n . .",
            ". n w w w w w n . .",
            ". n V V V V V n . .",
            ". D D D D D D D D .",
            ". D D D D D D D D .",
        ]
    },
    "note": {
        "grid": [
            ". . y y y y y y y .",
            ". . y y y y y y y .",
            ". . y . . . . . y .",
            ". . y . . . . . y .",
            ". . y . . . . . y .",
            ". . y . . . . . y .",
            "y y y . . . y y y .",
            "y y y . . . y y y .",
            ". y y . . . . y y .",
            ". . . . . . . . . .",
        ]
    },
    "shield": {
        "grid": [
            ". n n n n n n n n .",
            "n B B B B B B B B n",
            "n B B B B B B w B n",
            "n B B B B B w w B n",
            "n B w B B w w B B n",
            "n B w w w w B B B n",
            ". n B w w B B B n .",
            ". n B B B B B B n .",
            ". . n B B B B n . .",
            ". . . n n n n . . .",
        ]
    },
    # stacked floors with up/down arrows — "hop between floors". The orange
    # arrows (visible on both themes) + banded floors read as floor navigation,
    # NOT the filing-cabinet/building the earlier shaft version was mistaken for
    # (and no longer twins the multiagent building).
    "multifloor": {
        "grid": [
            ". . . . o . . . . .",
            ". . . o o o . . . .",
            ". K K K K K K K K .",
            ". K V V V V V V K .",
            ". K K K K K K K K .",
            ". K V V V V V V K .",
            ". K K K K K K K K .",
            ". . . o o o . . . .",
            ". . . . o . . . . .",
            ". . . . . . . . . .",
        ]
    },
    # an office facade (light frame) with 2-wide windows lit in different hues —
    # one per agent — and a bright ground-floor entrance
    "multiagent": {
        "grid": [
            ". K K K K K K K K .",
            ". K c c M y y M K .",
            ". K M M M M M M K .",
            ". K r r M c c M K .",
            ". K M M M M M M K .",
            ". K y y M r r M K .",
            ". K M M M M M M K .",
            ". K M w w w w M K .",
            ". K M w w w w M K .",
            ". K K K K K K K K .",
        ]
    },
    # a top-down floor plan: four rooms (cream), each with a desk (cyan), walled
    # off with doorways. Light (K) walls so the plan holds on the DARK theme (the
    # earlier brown frame merged into the night bg), and the desks say "rooms",
    # not a bare "+" grid.
    "spaces": {
        "grid": [
            "K K K K K K K K K .",
            "K V V V K V V V K .",
            "K V c V K V c V K .",
            "K V V V K V V V K .",
            "K K . K K K . K K .",
            "K V V V K V V V K .",
            "K V c V K V c V K .",
            "K V V V K V V V K .",
            "K K K K K K K K K .",
            ". . . . . . . . . .",
        ]
    },
    # the agent-tree DASHBOARD — a file-tree view: a root row, a trunk, and
    # three coloured agent rows branching off it (the sidebar-tree idiom reads
    # far clearer at ~20px than an org-chart's thin diagonal lines)
    "tree": {
        "grid": [
            ". . . . . . . . . .",
            ". . c c c . . . . .",
            ". . . K . . . . . .",
            ". . . K K l l l . .",
            ". . . K . . . . . .",
            ". . . K K y y y . .",
            ". . . K . . . . . .",
            ". . . K K r r r . .",
            ". . . . . . . . . .",
            ". . . . . . . . . .",
        ]
    },
    # a paw print — four toe beans in an arc + a rounded pad, centred and in a
    # warm brown (D) so it reads clean, not a heavy off-centre lump
    "pets": {
        "grid": [
            ". . D D . . D D . .",
            ". . D D . . D D . .",
            "D D . . . . . . D D",
            "D D . . . . . . D D",
            ". . . D D D D . . .",
            ". . D D D D D D . .",
            ". . D D D D D D . .",
            ". . D D D D D D . .",
            ". . . D D D D . . .",
            ". . . . . . . . . .",
        ]
    },
    # the OpenClaw gateway mascot — the office's own lobster, straight from the
    # pack (14x12), so the icon IS the mascot rendered in the office
    "lobster": {"sprite": "lobster_rest.sprite"},
    # Office VIBES = the ambient atmosphere: day/night + weather + themes (NOT
    # audio — that's the lofi row). A warm sun peeking behind a cloud; the cloud
    # is fully grey-OUTLINED so the white body stays legible on the light theme
    # (a base-only edge still washed out on cream).
    "vibes": {
        "grid": [
            ". t . . . . . . . .",
            ". . t t t . . . . .",
            ". t t t t t . . . .",
            "t . t t t t . . . .",
            ". . t t t K K K . .",
            ". . . K w w w w K .",
            ". . K w w w w w w K",
            ". . K w w w w w w K",
            ". . . K K K K K K .",
            ". . . . . . . . . .",
        ]
    },
    # a floating desktop window — title bar with traffic-light dots, then a light
    # content pane with text lines (NOT a solid cyan screen, which twinned the
    # monitor-glow icon)
    "window": {
        "grid": [
            "K K K K K K K K K K",
            "K M M M M M M M M K",
            "K r . y . l . M M K",
            "K M M M M M M M M K",
            "K w w w w w w w w K",
            "K w M M M M w w w K",
            "K w w w w w w w w K",
            "K w M M M w w w w K",
            "K w w w w w w w w K",
            "K K K K K K K K K K",
        ]
    },
}


def load_palette():
    with open(PACK / "pack.toml", "rb") as f:
        pack = tomllib.load(f)
    pal = {}
    for key, hexval in pack["palette"].items():
        if hexval == "transparent":
            pal[key] = None
        else:
            pal[key] = tuple(int(hexval[i : i + 2], 16) for i in (1, 3, 5))
    return pal


def sprite_rows(name, frame=0):
    rows, in_frame = [], False
    for line in (PACK / name).read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("@frame"):
            in_frame = int(line.split()[1]) == frame
            continue
        if in_frame:
            rows.append(line.split())
    if not rows:
        sys.exit(f"gen-pix-icons: no @frame {frame} rows in {name}")
    return rows


def render(icon_name, rows, pal):
    h, w = len(rows), len(rows[0])
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    px = img.load()
    assert px is not None
    for y, row in enumerate(rows):
        if len(row) != w:
            sys.exit(f"gen-pix-icons: {icon_name} row {y} is ragged ({len(row)} != {w})")
        for x, key in enumerate(row):
            if key not in pal:
                sys.exit(f"gen-pix-icons: {icon_name} uses unknown palette key {key!r}")
            rgb = pal[key]
            if rgb is not None:
                px[x, y] = (*rgb, 255)
    return img


# (label, dir) pairs — label disambiguates the two outputs, since both
# directories happen to share the basename "pix-icons".
OUTPUTS = [("site", OUT), ("readme", README_OUT)]


def emit(name, img, label, out_dir, check, work, stale):
    out = out_dir / f"{name}.png"
    tag = f"{label}/{name}"
    if check:
        if not out.exists():
            stale.append(f"{tag} (missing)")
            return
        cand = work / f"{label}-{name}.png"
        img.save(cand)
        DIFF_DIR.mkdir(parents=True, exist_ok=True)
        rc = subprocess.run(
            [sys.executable, str(COMPARE), str(out), str(cand), str(DIFF_DIR / f"diff-{label}-{name}.png")]
        ).returncode
        if rc != 0:
            stale.append(tag)
    else:
        buf = io.BytesIO()
        img.save(buf, format="PNG")
        out.write_bytes(buf.getvalue())
        print(f"wrote {out.relative_to(ROOT)} ({img.width}x{img.height})")


def main():
    check = "--check" in sys.argv[1:]
    pal = load_palette()
    for _, out_dir in OUTPUTS:
        out_dir.mkdir(parents=True, exist_ok=True)
    stale = []
    work = Path(tempfile.mkdtemp(prefix="gen-pix-icons-"))
    try:
        for name, spec in ICONS.items():
            rows = sprite_rows(spec["sprite"]) if "sprite" in spec else [r.split() for r in spec["grid"]]
            img = render(name, rows, pal)
            readme_img = img.resize(
                (img.width * README_SCALE, img.height * README_SCALE), Image.Resampling.NEAREST
            )
            emit(name, img, "site", OUT, check, work, stale)
            emit(name, readme_img, "readme", README_OUT, check, work, stale)
    finally:
        shutil.rmtree(work, ignore_errors=True)

    if check:
        # F3: an orphaned committed PNG (its manifest entry removed) is invisible
        # to the loop above, which only ever iterates ICONS — diff the directory
        # listing the other way too.
        for label, out_dir in OUTPUTS:
            orphans = sorted(p.stem for p in out_dir.glob("*.png") if p.stem not in ICONS)
            if orphans:
                stale.append(f"{label}: orphaned {', '.join(orphans)}")

    if stale:
        sys.exit(f"gen-pix-icons --check: stale/missing: {', '.join(stale)} — run just gen-icons")
    if check:
        print(f"gen-pix-icons --check: OK ({len(ICONS)} icons match in both output dirs)")


if __name__ == "__main__":
    main()
