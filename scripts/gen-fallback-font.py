#!/usr/bin/env python3
"""Generate crates/pixtuoid-scene/fonts/fallback.bin, the glyphs the cutaway's
hand-drawn font lacks, from two pinned open bitmap fonts, and write their
licenses beside it.

Usage: python3 scripts/gen-fallback-font.py   (just gen-fallback-font)

Fusion Pixel 8px is the one open pixel font whose CJK fits a terminal cell's
art (an 8x8 box); it has no Cyrillic or Greek, which X11's
public-domain 4x6 supplies on the hand-drawn font's own 3x5 grid. Each source
is baseline-aligned into Fusion Pixel's line box, which `text.rs` paints, and a
glyph whose ink leaves the box or the cells `unicode-width` gives it is
dropped, not cropped.

The format, which `cutaway::text` reads: one byte, the rows per glyph; a u16 LE
glyph count n; n u16 LE code points, ascending; then n glyphs, one byte a row
from the top, the high bit the leftmost pixel.
"""

import hashlib
import io
import sys
import unicodedata
import urllib.request
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FONTS = ROOT / "crates/pixtuoid-scene/fonts"
OUT = FONTS / "fallback.bin"

FUSION_TAG = "2026.09.25"
FUSION_URL = (
    "https://github.com/TakWolf/fusion-pixel-font/releases/download/"
    f"{FUSION_TAG}/fusion-pixel-font-8px-monospaced-bdf-v{FUSION_TAG}.zip"
)
FUSION_SHA256 = "bd76834c43d6882184356394cfdd8c9c5e5623d9d1c9020564fe545b7ba769ce"
# Simplified Chinese forms: a language variant redraws only shared ideographs.
FUSION_MEMBER = "fusion-pixel-8px-monospaced-zh_hans.bdf"
# The font's license and its sources' licenses, as its release ships them.
FUSION_LICENSES = {"OFL.txt": "OFL-Fusion-Pixel.txt", "LICENSES/": "LICENSES-Fusion-Pixel/"}
MISC_FIXED_URL = (
    "https://gitlab.freedesktop.org/xorg/font/misc-misc/-/raw/"
    "font-misc-misc-1.1.3/4x6.bdf"
)
MISC_FIXED_SHA256 = "dab0fc40105137bae55cd60c8f061c942787a4da21e44df28deca1525851a9c0"
MISC_FIXED_COPYING_URL = MISC_FIXED_URL.replace("4x6.bdf", "COPYING")
MISC_FIXED_COPYING_SHA256 = "1711d038bca0efb51b5114e902412019d1c21531882866b1a6908c6386268cfb"

# `text.rs`'s LINE_H and the art pixels a cell is wide (its ADVANCE).
LINE_H = 8
CELL_W = 4
# Rows down to and including the baseline: Fusion Pixel's own, where the
# hand-drawn capitals stand five rows under two of accent room.
ASCENT = 7

RANGES = [
    (0x00A0, 0x024F),  # Latin-1, Latin Extended-A and -B
    (0x0370, 0x04FF),  # Greek, Cyrillic
    (0x2000, 0x206F),  # General Punctuation
    (0x3000, 0x30FF),  # CJK punctuation, Hiragana, Katakana
    (0x4E00, 0x9FFF),  # CJK Unified Ideographs
    (0xAC00, 0xD7A3),  # Hangul Syllables, narrowed to KS X 1001 below
    (0xFF00, 0xFFEF),  # Halfwidth and Fullwidth Forms
]


def fetch(url, sha256):
    with urllib.request.urlopen(url) as r:
        data = r.read()
    got = hashlib.sha256(data).hexdigest()
    if got != sha256:
        sys.exit(f"{url}: sha256 {got}, pinned {sha256}")
    return data


def parse_bdf(text):
    """{code point: (advance, ink as (row, col)s, row 0 the first under the baseline)}"""
    glyphs = {}
    ascent = None
    cp = advance = bbx = bitmap = None
    for line in text.splitlines():
        p = line.split()
        if not p:
            continue
        if p[0] == "FONT_ASCENT":
            ascent = int(p[1])
        elif p[0] == "ENCODING":
            cp = int(p[1])
        elif p[0] == "DWIDTH":
            advance = int(p[1])
        elif p[0] == "BBX":
            bbx = [int(v) for v in p[1:5]]
        elif p[0] == "BITMAP":
            bitmap = []
        elif p[0] == "ENDCHAR":
            w, h, xoff, yoff = bbx
            top = ascent - (h + yoff)
            ink = set()
            for r, row in enumerate(bitmap):
                bits, nbits = int(row, 16), len(row) * 4
                for c in range(w):
                    if bits >> (nbits - 1 - c) & 1:
                        ink.add((top + r - ascent, xoff + c))
            glyphs[cp] = (advance, ink)
            bitmap = None
        elif bitmap is not None:
            bitmap.append(p[0])
    return glyphs


def cells(cp):
    return 2 if unicodedata.east_asian_width(chr(cp)) in ("W", "F") else 1


# The halfwidth kana sound marks, which `unicode-width` counts as combining
# (Grapheme_Extend), a property `unicodedata` does not expose.
ZERO_WIDTH = {0xFF9E, 0xFF9F}


def wanted(cp):
    if unicodedata.category(chr(cp)) in ("Cc", "Cf", "Cn", "Co", "Cs", "Mn", "Me", "Zl", "Zp"):
        return False
    if cp in ZERO_WIDTH:
        return False
    # KS X 1001's 2,350 syllables: EUC-KR's two-byte set, the repertoire a
    # Korean bitmap font ships; the other 8,822 would quadruple the Hangul.
    if 0xAC00 <= cp <= 0xD7A3:
        # CPython's euc_kr spells any other syllable as an 8-byte KS X 1001
        # Annex 3 make-up sequence instead of raising.
        return len(chr(cp).encode("euc_kr")) == 2
    return True


def rows_of(advance, ink, n_cells):
    """The glyph as LINE_H row bytes, or None when it leaves its box."""
    if advance != n_cells * CELL_W:
        return None
    rows = [0] * LINE_H
    for dy, x in ink:
        y = dy + ASCENT
        if not (0 <= y < LINE_H and 0 <= x < n_cells * CELL_W):
            return None
        rows[y] |= 0x80 >> x
    return rows


def main():
    FONTS.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(io.BytesIO(fetch(FUSION_URL, FUSION_SHA256))) as z:
        fusion = parse_bdf(z.read(FUSION_MEMBER).decode("utf-8"))
        for name in z.namelist():
            for prefix, dest in FUSION_LICENSES.items():
                if name.startswith(prefix) and not name.endswith("/"):
                    path = FONTS / (dest + name[len(prefix):])
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(z.read(name))
    misc_fixed = parse_bdf(fetch(MISC_FIXED_URL, MISC_FIXED_SHA256).decode("latin-1"))
    copying = fetch(MISC_FIXED_COPYING_URL, MISC_FIXED_COPYING_SHA256)
    (FONTS / "COPYING-misc-fixed.txt").write_bytes(copying)

    picked = {}
    for lo, hi in RANGES:
        for cp in range(lo, hi + 1):
            if not wanted(cp):
                continue
            for source in (fusion, misc_fixed):
                if cp in source:
                    rows = rows_of(*source[cp], cells(cp))
                    if rows is not None:
                        picked[cp] = rows
                        break

    order = sorted(picked)
    out = bytearray([LINE_H])
    out += len(order).to_bytes(2, "little")
    for cp in order:
        out += cp.to_bytes(2, "little")
    for cp in order:
        out += bytes(picked[cp])
    OUT.write_bytes(out)
    print(f"{OUT.relative_to(ROOT)}: {len(order)} glyphs, {len(out)} bytes")


if __name__ == "__main__":
    main()
