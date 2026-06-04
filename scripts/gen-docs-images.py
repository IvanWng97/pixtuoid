#!/usr/bin/env python3
"""Regenerate every office image under docs/images/ from a release build.

Single source of truth for the doc images — run via `just demo` whenever a
change alters the office's look, so the README screenshots stay current and
nobody has to re-guess the render parameters or the themes-composite diagonal
angle.

Everything derives from one 192x64 release `snapshot` render at a fixed dusk
clock (deterministic layout + lighting):

  docs/images/screenshot.png                        full office @2x
  docs/images/gallery-{cubicle,meeting,pantry}.png  crop quadrants @3x
  docs/images/themes-composite.png                  6 themes stitched along
                                                    down-left DIAGONAL bands
  docs/images/demo.gif                              15s @ 10fps animated

`screenshot-real.png` is a live-agent capture and is NOT regenerated here.

Requires the venv (Pillow) — see README "Visual verification".
"""

import subprocess
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
SNAP = ROOT / "target/release/examples/snapshot"
OUT = ROOT / "docs/images"

COLS, ROWS = 192, 64  # -> 1536x1024 px
HOUR, DAY = 19, 1  # fixed dusk clock = deterministic layout + lighting
THEMES = ["normal", "cyberpunk", "dracula", "tokyo-night", "catppuccin", "gruvbox"]
GIF_SECS, GIF_FPS = 15, 10
# themes-composite diagonal: down-left, matching the window light-beams. dx/dy.
SLANT = -0.20
QUADRANTS = {  # same regions as scripts/crop-snapshot.py
    "cubicle": (0.30, 0.00, 1.00, 0.55),
    "meeting": (0.00, 0.00, 0.30, 0.55),
    "pantry": (0.00, 0.49, 0.30, 1.00),
}


def render(out_path, theme, *extra):
    subprocess.run(
        [
            str(SNAP), "--cols", str(COLS), "--rows", str(ROWS), "--theme", theme,
            "--now-hour", str(HOUR), "--now-day", str(DAY), *extra, str(out_path),
        ],
        check=True,
        stdout=subprocess.DEVNULL,  # suppress the text preview; gif progress is on stderr
    )


def main():
    print("building release snapshot example ...")
    subprocess.run(["cargo", "build", "--release", "--example", "snapshot"], cwd=ROOT, check=True)
    tmp = Path(tempfile.mkdtemp())

    # Base office render (normal theme) -> screenshot + gallery crops.
    render(tmp / "office.png", "normal")
    office = Image.open(tmp / "office.png").convert("RGB")
    w, h = office.size

    office.resize((w * 2, h * 2), Image.NEAREST).save(OUT / "screenshot.png")
    for name, (x0, y0, x1, y1) in QUADRANTS.items():
        crop = office.crop((int(w * x0), int(h * y0), int(w * x1), int(h * y1)))
        crop.resize((crop.width * 3, crop.height * 3), Image.NEAREST).save(OUT / f"gallery-{name}.png")

    # themes-composite: render each theme, then paint each one's slice through a
    # diagonal mask that runs from its left boundary to the right edge. Painted
    # left-to-right, so band i (between boundary i and i+1) ends up showing theme
    # i, the leftmost stays theme 0, and the rightmost stays the last theme.
    n = len(THEMES)
    for i, theme in enumerate(THEMES):
        render(tmp / f"th_{i}.png", theme)
    comp = Image.open(tmp / "th_0.png").convert("RGB")
    far = w + abs(SLANT) * h + 10  # extend past the right edge so nothing is left uncovered
    for i in range(1, n):
        im = Image.open(tmp / f"th_{i}.png").convert("RGB")
        bx = i * w / n  # i-th boundary, x at the top row
        poly = [(bx, 0), (far, 0), (far, h), (bx + SLANT * h, h)]
        mask = Image.new("L", (w, h), 0)
        ImageDraw.Draw(mask).polygon(poly, fill=255)
        comp.paste(im, (0, 0), mask)
    comp.save(OUT / "themes-composite.png")

    # Animated demo.
    render(OUT / "demo.gif", "normal", "--gif", "--gif-duration", str(GIF_SECS), "--gif-fps", str(GIF_FPS))

    print(f"wrote screenshot, gallery-*, themes-composite, demo.gif -> {OUT}")


if __name__ == "__main__":
    main()
