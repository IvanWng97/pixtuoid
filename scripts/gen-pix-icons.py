#!/usr/bin/env python3
"""Generate the site's pixel-icon PNGs (site/src/assets/pix-icons/).

Single color source: the embedded sprite pack's palette
(crates/pixtuoid-scene/sprites/default/pack.toml) — an icon grid may only use
keys defined there, so the icons can never drift off the office's own colors.
An icon is either extracted verbatim from a pack sprite ("sprite") or authored
here as a pixel grid ("grid"). Output is 1x RGBA PNGs; PixIcon.astro
integer-upscales them with image-rendering: pixelated (upscale-crisp only).

Usage:
  .venv/bin/python3 scripts/gen-pix-icons.py          # (re)generate (just gen-icons)
  .venv/bin/python3 scripts/gen-pix-icons.py --check  # exit 1 on drift
"""

import io
import sys
import tomllib
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
PACK = ROOT / "crates/pixtuoid-scene/sprites/default"
OUT = ROOT / "site/src/assets/pix-icons"

# Icon manifest. "sprite": extract a whole pack sprite frame verbatim.
# "grid": rows of space-separated pack-palette keys ('.' = transparent).
ICONS = {
    # the office's own walker, straight from the pack (8x12)
    "walk": {"sprite": "walking_0.sprite"},
    "coffee": {
        "grid": [
            ". . w . . w . . . .",
            ". w . . w . . . . .",
            ". . w . . w . . . .",
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
    "glow": {
        "grid": [
            ". M M M M M M M M .",
            ". M j j j j j j M .",
            ". M j c c c c j M .",
            ". M j c c c c j M .",
            ". M j c c c c j M .",
            ". M j j j j j j M .",
            ". M M M M M M M M .",
            ". . . . M M . . . .",
            ". . M M M M M M . .",
            ". . . . . . . . . .",
        ]
    },
    "magnify": {
        "grid": [
            ". . K K K K . . . .",
            ". K c c c c K . . .",
            "K c w c c c c K . .",
            "K c c c c c c K . .",
            "K c c c c c c K . .",
            "K c c c c c c K . .",
            ". K c c c c K . . .",
            ". . K K K K D D . .",
            ". . . . . . D D D .",
            ". . . . . . . D D D",
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


def main():
    check = "--check" in sys.argv[1:]
    pal = load_palette()
    OUT.mkdir(parents=True, exist_ok=True)
    stale = []
    for name, spec in ICONS.items():
        rows = sprite_rows(spec["sprite"]) if "sprite" in spec else [r.split() for r in spec["grid"]]
        img = render(name, rows, pal)
        buf = io.BytesIO()
        img.save(buf, format="PNG")
        out = OUT / f"{name}.png"
        if check:
            if not out.exists() or out.read_bytes() != buf.getvalue():
                stale.append(name)
        else:
            out.write_bytes(buf.getvalue())
            print(f"wrote {out.relative_to(ROOT)} ({img.width}x{img.height})")
    if stale:
        sys.exit(f"gen-pix-icons --check: stale/missing: {', '.join(stale)} — run just gen-icons")


if __name__ == "__main__":
    main()
