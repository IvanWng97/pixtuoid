#!/usr/bin/env python3
"""Generate the scene's two pixel faces from pinned open fonts, and
write their licenses beside them: crates/pixtuoid-scene/fonts/world.bin, the
text drawn into the office's art, and screen.bin, the floating window's own
text, from Fusion Pixel 12px. Noto Emoji supplies supplementary glyphs.

Usage: Python 3.14 with requirements-dev.txt, scripts/gen-fonts.py (just gen-fonts).

Fusion Pixel 8px is the one open pixel font whose CJK fits a terminal cell's
art (an 8x8 box); it has no Cyrillic or Greek, which draw as tofu. Its glyphs are
baseline-aligned into its face's line box, which `text.rs` and `grid.rs`
paint, and one whose ink leaves the box or the cells `unicode-width` gives it
is dropped, not cropped.

The format, which `cutaway::text` and `cutaway::grid` read: one byte, the rows
per glyph; a u16 LE glyph count n; n u32 LE code points, ascending; then n
glyphs, a row at a time from the top, the high bit the leftmost pixel: a byte
a row in world.bin, a u16 LE in screen.bin, whose glyphs are wider than a byte.
A symbol Fusion Pixel draws only full-width fails the width check and is drawn
by hand in `text.rs` and `grid.rs`.
"""

import hashlib
import io
import sys
import unicodedata
import urllib.request
import zipfile
from pathlib import Path

from fontTools.ttLib import TTFont
from PIL import Image, ImageFont, features

ROOT = Path(__file__).resolve().parent.parent
FONTS = ROOT / "crates/pixtuoid-scene/fonts"
OUT = FONTS / "world.bin"
SCREEN_OUT = FONTS / "screen.bin"

FETCH_TIMEOUT_S = 30

FUSION_TAG = "2026.09.25"
FUSION_URL = (
    "https://github.com/TakWolf/fusion-pixel-font/releases/download/"
    f"{FUSION_TAG}/fusion-pixel-font-8px-monospaced-bdf-v{FUSION_TAG}.zip"
)
FUSION_SHA256 = "bd76834c43d6882184356394cfdd8c9c5e5623d9d1c9020564fe545b7ba769ce"
# Latin's half-width punctuation (`·`, `…`) first; then Simplified Chinese
# forms, since a language variant redraws only shared ideographs.
FUSION_MEMBERS = (
    "fusion-pixel-8px-monospaced-latin.bdf",
    "fusion-pixel-8px-monospaced-zh_hans.bdf",
)
# The font's license and its sources' licenses, as its release ships them.
FUSION_LICENSES = {"OFL.txt": "OFL-Fusion-Pixel.txt", "LICENSES/": "LICENSES-Fusion-Pixel/"}
SCREEN_URL = FUSION_URL.replace("8px", "12px")
SCREEN_SHA256 = "d75f5262f108757edb0f47ee8e3d2dfdfbecfd94558faf5ffb0c06dc5866fb3b"
SCREEN_MEMBERS = tuple(m.replace("8px", "12px") for m in FUSION_MEMBERS)
# Its sources join the 8px's: the OFL and the shared ones are the same text.
SCREEN_LICENSES = {"LICENSES/": "LICENSES-Fusion-Pixel/"}
NOTO_ROOT = (
    "https://raw.githubusercontent.com/google/fonts/"
    "b979dba422e445492b0eb9951ac52ee0b4d648c3/ofl/notoemoji/"
)
NOTO_SHA256 = "de6c18832938afc99caf132b39d6a30a19bac7f2e812e28db2535b4608d27551"
NOTO_LICENSE_SHA256 = "500bb1ccf43df7bbb522112f9133a52b16e1c35e809632f5d8609b179152de5b"
# Pillow's wheel pins this rasterizer; a system build can otherwise change glyph bits.
FREETYPE_VERSION = "2.14.3"
# Unicode 16 widened the trigrams; older width data drops existing screen glyphs.
UNICODE_VERSION = "16.0.0"
BMP_MAX = 0xFFFF
# `grid.rs`'s SCREEN_LINE_H and SCREEN_CELL_W, and Fusion Pixel 12px's ascent.
SCREEN_LINE_H = 12
SCREEN_CELL_W = 6
SCREEN_ASCENT = 10
# `text.rs`'s LINE_H and the art pixels a cell is wide (its ADVANCE).
LINE_H = 8
CELL_W = 4
# Rows down to and including the baseline: Fusion Pixel's own, capitals
# five rows under two of accent room.
ASCENT = 7

# Screen text adds the symbol blocks Fusion Pixel 12px draws half-width.
SCREEN_RANGES = [(0x2190, 0x2BFF)]

RANGES = [
    (0x0020, 0x007E),  # ASCII
    (0x00A0, 0x024F),  # Latin-1, Latin Extended-A and -B
    (0x0370, 0x04FF),  # Greek, Cyrillic
    (0x2000, 0x206F),  # General Punctuation
    (0x3000, 0x30FF),  # CJK punctuation, Hiragana, Katakana
    (0x4E00, 0x9FFF),  # CJK Unified Ideographs
    (0xAC00, 0xD7A3),  # Hangul Syllables, narrowed to KS X 1001 below
    (0xFF00, 0xFFEF),  # Halfwidth and Fullwidth Forms
]


def fetch(url, sha256):
    with urllib.request.urlopen(url, timeout=FETCH_TIMEOUT_S) as r:
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


def wanted(cp):
    if unicodedata.category(chr(cp)) in ("Cc", "Cf", "Cn", "Co", "Cs", "Mn", "Me", "Zl", "Zp"):
        return False
    # KS X 1001's 2,350 syllables: EUC-KR's two-byte set, the repertoire a
    # Korean bitmap font ships; the other 8,822 would quadruple the Hangul.
    if 0xAC00 <= cp <= 0xD7A3:
        # CPython's euc_kr spells any other syllable as an 8-byte KS X 1001
        # Annex 3 make-up sequence instead of raising.
        return len(chr(cp).encode("euc_kr")) == 2
    return True


def rows_of(advance, ink, n_cells, cell_w=CELL_W, line_h=LINE_H, ascent=ASCENT, bits=8):
    """The glyph as `line_h` rows of `bits`-bit ink, the high bit leftmost,
    or None when it leaves its box."""
    if advance != n_cells * cell_w:
        return None
    rows = [0] * line_h
    for dy, x in ink:
        y = dy + ascent
        if not (0 <= y < line_h and 0 <= x < n_cells * cell_w):
            return None
        rows[y] |= (1 << (bits - 1)) >> x
    return rows


def write(path, rows_per_glyph, picked, row_bytes):
    order = sorted(picked)
    out = bytearray([rows_per_glyph])
    out += len(order).to_bytes(2, "little")
    for cp in order:
        out += cp.to_bytes(4, "little")
    for cp in order:
        for row in picked[cp]:
            out += row.to_bytes(row_bytes, "little")
    path.write_bytes(out)
    print(f"{path.relative_to(ROOT)}: {len(order)} glyphs, {len(out)} bytes")


def unzip(url, sha256, members, licenses):
    """The parsed `members` of the pinned zip at `url`, its licenses written."""
    with zipfile.ZipFile(io.BytesIO(fetch(url, sha256))) as z:
        fonts = [parse_bdf(z.read(m).decode("utf-8")) for m in members]
        for name in z.namelist():
            for prefix, dest in licenses.items():
                if name.startswith(prefix) and not name.endswith("/"):
                    path = FONTS / (dest + name[len(prefix):])
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(z.read(name))
    return fonts


def emoji_rows(data, line_h, cell_w, row_bytes):
    with TTFont(io.BytesIO(data)) as font:
        points = sorted(font.getBestCmap())
    faces = [
        ImageFont.truetype(io.BytesIO(data), size, layout_engine=ImageFont.Layout.BASIC)
        for size in range(line_h, 0, -1)
    ]
    picked = {}
    for cp in points:
        if cp <= BMP_MAX or not wanted(cp):
            continue
        width = cells(cp) * cell_w
        for face in faces:
            mask = face.getmask(chr(cp), mode="1")
            image = Image.frombytes("L", mask.size, bytes(mask))
            box = image.getbbox()
            if box is None:
                continue
            ink = image.crop(box)
            if ink.width > width or ink.height > line_h:
                continue
            xoff, yoff = (width - ink.width) // 2, (line_h - ink.height) // 2
            rows = [0] * line_h
            for y in range(ink.height):
                for x in range(ink.width):
                    if ink.getpixel((x, y)):
                        rows[y + yoff] |= 1 << (row_bytes * 8 - 1 - x - xoff)
            picked[cp] = rows
            break
        else:
            sys.exit(f"Noto Emoji U+{cp:04X}: no ink fits {width}x{line_h}")
    return picked


def main():
    if unicodedata.unidata_version != UNICODE_VERSION:
        sys.exit(f"Use Python 3.14 (Unicode {UNICODE_VERSION})")
    if features.version("freetype2") != FREETYPE_VERSION:
        sys.exit(f"Use requirements-dev.txt's Pillow wheel (FreeType {FREETYPE_VERSION})")
    FONTS.mkdir(parents=True, exist_ok=True)
    noto = fetch(NOTO_ROOT + "NotoEmoji%5Bwght%5D.ttf", NOTO_SHA256)
    (FONTS / "OFL-Noto-Emoji.txt").write_bytes(
        fetch(NOTO_ROOT + "OFL.txt", NOTO_LICENSE_SHA256)
    )
    fusion = unzip(FUSION_URL, FUSION_SHA256, FUSION_MEMBERS, FUSION_LICENSES)

    picked = {}
    for lo, hi in RANGES:
        for cp in range(lo, hi + 1):
            if not wanted(cp):
                continue
            for source in fusion:
                if cp in source:
                    rows = rows_of(*source[cp], cells(cp))
                    if rows is not None:
                        picked[cp] = rows
                        break

    picked.update(emoji_rows(noto, LINE_H, CELL_W, 1))
    write(OUT, LINE_H, picked, 1)

    screen_fonts = unzip(SCREEN_URL, SCREEN_SHA256, SCREEN_MEMBERS, SCREEN_LICENSES)
    screen = {}
    for lo, hi in SCREEN_RANGES + RANGES:
        for cp in range(lo, hi + 1):
            if not wanted(cp):
                continue
            for source in screen_fonts:
                if cp in source:
                    rows = rows_of(
                        *source[cp], cells(cp), SCREEN_CELL_W, SCREEN_LINE_H, SCREEN_ASCENT, 16
                    )
                    if rows is not None:
                        screen[cp] = rows
                        break
    screen.update(emoji_rows(noto, SCREEN_LINE_H, SCREEN_CELL_W, 2))
    write(SCREEN_OUT, SCREEN_LINE_H, screen, 2)


if __name__ == "__main__":
    main()
