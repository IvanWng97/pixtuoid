#!/usr/bin/env python3
"""Author the bundled pack's generated sprite art: the cutaway profile's `@4x`
pieces, and the classic profile's 1x pieces drawn from the same layout.

Draws in PALETTE-KEY space, so the output is .sprite text and the recolor keys
(H/B/S/P and their [ramps] shades) survive per-agent recoloring. The .sprite
files it writes are committed; edit this, not them, and rerun:

    just gen-art

`--check` writes nothing and fails if a sprite it draws is missing or differs
from the committed one, or if a sprite carrying the provenance line is no longer
drawn here (a write deletes those).

It resolves no colour itself: the engine owns the palette and its ramps, so look
at the result through the real renderer: `cargo run --release --example
cutaway_snapshot -- <out.png> --scale 8` for the `@4x` art, `cargo run --release
--example snapshot -- --crop-furniture desk <out.png>` for the 1x desk.
"""

import argparse
import difflib
import math
import pathlib
import random
import sys
import tempfile

S = 4  # the cutaway art's density

# ---- palette keys ---------------------------------------------------------
# recolor bases and their shades (bases in [palette], shades in [ramps])
HAIR, HAIR_SH, HAIR_LT, HAIR_HI, HAIR_OUT = "H", "h", "Y", "Z", "X"
SKIN, SKIN_SH, SKIN_DK = "S", "s", "i"
SHIRT, SHIRT_SH, SHIRT_LT, SHIRT_OUT = "B", "v", "W", "U"
PANTS, PANTS_SH, PANTS_LT, PANTS_OUT = "P", "p", "(", ")"
# fixed colours, named by role
OUTLINE, WHITE, WHITE_SH, BRIGHT = "n", "&", "*", "w"
EYE, MOUTH, BLUSH = "e", "m", "^"
SHOE, SHOE_HI = ";", ":"
WOOD, WOOD_SH, WOOD_LT, WOOD_HI, WOOD_DK, POOL = "D", "d", "O", "σ", "ψ", "ω"
BEZEL, SLATE, SHADOW = "M", "3", "4"
# The desk monitor's screen: the cutaway relights these keys, so only desk art
# draws them.
GLASS, GLASS_TXT = "j", "J"
# Any other screen's glass.
DISPLAY, DISPLAY_SHEEN = "Ξ", "Ж"
KEY_DK, KEYCAP, GREY = "k", "φ", "6"
LAMP, LAMP_HI, BULB = "7", "Θ", "9"
OFFWHITE, OFFWHITE_SH, PRINT = "¤", "!", "$"
MUG, MUG_SH = "V", "%"
CHAIR, CHAIR_RIM, CHAIR_BASE = "T", "I", "/"
# A character's one outline round hair, face and clothes alike.
SILHOUETTE = "κ"
COFFEE = "ρ"
INK, CYAN, RED, DARK_RED, ORANGE, BLUE, GOLD, TAN, BROWN = (
    "q", "c", "r", "N", "o", "b", "y", "x", "z")
T = "."



def canvas(w, h):
    return [[T] * w for _ in range(h)]


def put(g, x, y, k):
    if 0 <= y < len(g) and 0 <= x < len(g[0]):
        g[y][x] = k


def rect(g, x0, y0, x1, y1, k):
    for y in range(max(0, y0), min(len(g), y1)):
        for x in range(max(0, x0), min(len(g[0]), x1)):
            g[y][x] = k


def despeckle(g, keys):
    """A pixel of `keys` with no 4-neighbour of its own key takes its most common
    opaque neighbour: a lone highlight reads as noise."""
    h, w = len(g), len(g[0])
    fixes = []
    for y in range(h):
        for x in range(w):
            k = g[y][x]
            if k not in keys:
                continue
            nbrs = [g[y + dy][x + dx] for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1))
                    if 0 <= x + dx < w and 0 <= y + dy < h]
            if k in nbrs:
                continue
            opaque = [n for n in nbrs if n != T]
            if opaque:
                fixes.append((x, y, max(sorted(set(opaque)), key=opaque.count)))
    for x, y, k in fixes:
        g[y][x] = k


def fill_pinholes(g):
    """A transparent pixel walled in on all four sides is a hole in a solid."""
    h, w = len(g), len(g[0])
    fixes = []
    for y in range(1, h - 1):
        for x in range(1, w - 1):
            if g[y][x] != T:
                continue
            nbrs = [g[y + dy][x + dx] for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1))]
            if T not in nbrs:
                fixes.append((x, y, max(sorted(set(nbrs)), key=nbrs.count)))
    for x, y, k in fixes:
        g[y][x] = k


def outline(g, inside, out):
    h, w = len(g), len(g[0])
    marks = []
    for y in range(h):
        for x in range(w):
            if g[y][x] != T:
                continue
            for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                xx, yy = x + dx, y + dy
                if 0 <= xx < w and 0 <= yy < h and g[yy][xx] in inside:
                    marks.append((x, y))
                    break
    for x, y in marks:
        g[y][x] = out


# ---- the desk: one skeleton, drawn at 1x (classic) and at `S` (cutaway) -----
# Logical units. Both drawings scale every row and column below by their own
# density, so the `@4x` art cannot drift from the 1x it redraws.
#
# Rust owns these facts; the copies here are pinned against the generated art
# by `embedded_pack`'s `a_desks_rows_follow_the_layout` and `effects`'s
# `the_glow_lands_on_the_desk_arts_monitor`.
#
# The desk `FurnitureDef`'s visual width (`desk_sprite_width_tracks_the_footprint_overhang`).
DESK_ART_W = 14
# `pixel_painter::drawable`'s `DESK_BEZEL_RAISE`: the monitor's row above the wood.
DESK_BEZEL_RAISE = 1
# `layout`'s `DESK_SURFACE_ROWS`, `DESK_FRONT_ROWS`, `DESK_LEG_ROWS`.
DESK_SURFACE_ROWS, DESK_FRONT_ROWS, DESK_LEG_ROWS = 5, 1, 2
DESK_ART_H = DESK_BEZEL_RAISE + DESK_SURFACE_ROWS + DESK_FRONT_ROWS + DESK_LEG_ROWS
# The back-turned desk's extra rows, all above, so the occupant (who y-sorts in
# FRONT of the desk) leaves the upper screen row clear. Its lower screen row
# sits inside the wood, flanked by it: the owner picked a monitor standing on
# its desk over one floating clear of every head.
DESK_NORTH_LIFT = 2
DESK_LEG_W = 2
# The desk top's grain: a seam between boards this many rows deep, and a few
# short streaks inside the boards, sparse enough to read as wood, not stripes.
DESK_BOARD_ROWS = 12
DESK_STREAKS = 3
# The monitor box: its columns (half-open), and from the sprite's top the first
# glass row and the chin row — casing above the glass, glass down to the chin —
# where the classic painter's glow lands (`pixel_painter::effects`'s `SCREEN_*`).
DESK_MONITOR_X0, DESK_MONITOR_X1 = 3, 11
DESK_GLASS_Y0, DESK_CHIN_Y = 2, 4


def desk_rows(lift):
    """The wood's rows for a desk `lift` rows taller above: `(top, lip, legs,
    height)`, the first rows of the surface, the front lip and the legs."""
    top = DESK_BEZEL_RAISE + lift
    lip = top + DESK_SURFACE_ROWS
    legs = lip + DESK_FRONT_ROWS
    return top, lip, legs, legs + DESK_LEG_ROWS


def desk_1x(lift):
    """The classic desk's wood: a lit back edge, a bright front lip, and legs
    dark on their inner side, with open floor between them so the carpet and
    anyone walking behind the desk show through. The props (lamp, mug, paper
    tower) are the classic painter's live overlays, so the art draws none of
    them."""
    top, lip, legs, h = desk_rows(lift)
    g = canvas(DESK_ART_W, h)
    rect(g, 0, top, DESK_ART_W, lip, WOOD)
    rect(g, 0, top, DESK_ART_W, top + 1, WOOD_LT)
    rect(g, 0, lip, DESK_ART_W, legs, WOOD_HI)
    for x0, inner in ((0, DESK_LEG_W - 1), (DESK_ART_W - DESK_LEG_W, DESK_ART_W - DESK_LEG_W)):
        rect(g, x0, legs, x0 + DESK_LEG_W, h, WOOD_SH)
        rect(g, inner, legs, inner + 1, h, WOOD_DK)
    return g


def monitor_stand_1x(g):
    """The chin under the monitor, the stand's shadow either side of it."""
    rect(g, DESK_MONITOR_X0 + 1, DESK_CHIN_Y, DESK_MONITOR_X1 - 1, DESK_CHIN_Y + 1, BEZEL)
    for x in (DESK_MONITOR_X0 + 1, DESK_MONITOR_X1 - 2):
        put(g, x, DESK_CHIN_Y, SHADOW)


def desk_south_1x():
    """The viewer-facing desk at 1x: the monitor's back, lit along its top
    edge. Its glass rows stay casing: a viewer-facing seat never shows a
    screen, though the glow still lights them where a pack ships no
    `desk_north` and this art stands in for it."""
    g = desk_1x(0)
    rect(g, DESK_MONITOR_X0, 0, DESK_MONITOR_X1, DESK_CHIN_Y, BEZEL)
    rect(g, DESK_MONITOR_X0, 0, DESK_MONITOR_X1, 1, SLATE)
    monitor_stand_1x(g)
    return g


def desk_north_1x():
    """The back-turned desk at 1x: the raised monitor, its casing outlined in
    grey so it reads against a dark carpet, two lines of dim text on its glass,
    and a keyboard a row clear of the stand so the two never merge into one
    dark base."""
    g = desk_1x(DESK_NORTH_LIFT)
    x0, x1 = DESK_MONITOR_X0, DESK_MONITOR_X1
    rect(g, x0, 0, x1, DESK_CHIN_Y, SLATE)
    rect(g, x0 + 1, 1, x1 - 1, DESK_CHIN_Y, BEZEL)
    rect(g, x0 + 1, DESK_GLASS_Y0, x1 - 1, DESK_CHIN_Y, GLASS)
    rect(g, x0 + 2, DESK_GLASS_Y0, x0 + 6, DESK_GLASS_Y0 + 1, GLASS_TXT)
    rect(g, x0 + 2, DESK_GLASS_Y0 + 1, x0 + 4, DESK_GLASS_Y0 + 2, GLASS_TXT)
    monitor_stand_1x(g)
    keys = DESK_CHIN_Y + 2
    rect(g, x0 + 1, keys, x1 - 1, keys + 1, GREY)
    for x in (x0 + 1, x1 - 2):
        put(g, x, keys, KEY_DK)
    return g


# ---- shared strokes ----------------------------------------------------------
def capsule(g, pts, r, key, only_empty=False):
    """Stroke a polyline of radius `r` — an arm or a leg."""
    for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
        steps = int(max(abs(x1 - x0), abs(y1 - y0)) * 2) + 1
        for i in range(steps + 1):
            t = i / steps
            px, py = x0 + (x1 - x0) * t, y0 + (y1 - y0) * t
            for y in range(int(py - r) - 1, int(py + r) + 2):
                for x in range(int(px - r) - 1, int(px + r) + 2):
                    if (x + 0.5 - px) ** 2 + (y + 0.5 - py) ** 2 <= r * r:
                        if not only_empty or (0 <= y < len(g) and 0 <= x < len(g[0]) and g[y][x] == T):
                            put(g, x, y, key)


# ---- furniture: plants, whiteboard, bookshelf, meeting sofa ------------------------
LEAF, LEAF_SH, LEAF_HI, LEAF_DK = "l", "L", "Φ", "Ψ"
POT, POT_SH, POT_HI, SOIL = "g", "Γ", "δ", "Σ"
PETAL_R, PETAL_Y, PETAL_HI = RED, GOLD, "ζ"
ALU, ALU_DK = "Π", "θ"
FABRIC, FABRIC_HI, FABRIC_SH, FABRIC_SEAM = "C", "G", "Λ", "Ω"
BOOKS = (RED, BLUE, GOLD, FABRIC, TAN, DARK_RED, ORANGE, "λ")


def leaf(g, x0, y0, ang, length, width, lit_side=1):
    """A pointed leaf from (x0, y0) along `ang` (radians), lit on one half."""
    ca, sa = math.cos(ang), math.sin(ang)
    for y in range(len(g)):
        for x in range(len(g[0])):
            dx, dy = x + 0.5 - x0, y + 0.5 - y0
            u = dx * ca + dy * sa  # along the leaf
            v = -dx * sa + dy * ca  # across it
            if 0 <= u <= length:
                half = width * math.sin(math.pi * u / length) ** 0.8
                if abs(v) <= half:
                    if abs(v) < 0.7 and u > 1:
                        g[y][x] = LEAF_DK  # midrib
                    elif (v > 0) == (lit_side > 0):
                        g[y][x] = LEAF_HI if (u < length * 0.5 and abs(v) > half * 0.4) else LEAF
                    else:
                        g[y][x] = LEAF_SH


def pot(g, cx, top, bot, half_top, half_bot, dark=False):
    for y in range(top, bot):
        t = (y - top) / max(1, bot - top - 1)
        half = half_top + (half_bot - half_top) * t
        for x in range(len(g[0])):
            d = x + 0.5 - cx
            if abs(d) <= half:
                if dark:  # lit from the upper left, like the terracotta
                    g[y][x] = SLATE if d < -half * 0.45 else OUTLINE if d < half * 0.35 else SHADOW
                else:
                    g[y][x] = POT_HI if d < -half * 0.45 else POT if d < half * 0.35 else POT_SH
    rect(g, int(cx - half_top) - 1, top, int(cx + half_top) + 1, top + 3, POT_HI if not dark else SLATE)
    rect(g, int(cx - half_top) + 1, top + 1, int(cx + half_top) - 1, top + 3, SOIL)


# ---- the meeting sofa seen from behind: facing north ----------------------------
# One layout for both densities, in classic rows. A back-view sitter's anchor
# (`back_couch_anchor`) lands their shoulders on the seat rows and their lap on
# the backrest's rows, so the seat sits under a sitter and the backrest over one.
SOFA_W, SOFA_H = 20, 7
SOFA_SEAT_ROWS = 3  # rows [0, SOFA_SEAT_ROWS): the seat; the backrest below
SOFA_RIDGE_ROWS = 1  # the backrest's lit top, then its back panel to the foot
# The three seats' centres (the seat waypoints' `SEAT_DX` about the sofa's
# centre column), on column boundaries.
SOFA_SEAT_COLS = (4, 10, 16)


def meeting_sofa_north_1x():
    """The north-facing sofa at classic density: lit cushion tops, a seam where
    the seat meets the backrest, its lit ridge, and the back panel shading down."""
    ridge = SOFA_SEAT_ROWS
    g = canvas(SOFA_W, SOFA_H)
    rect(g, 0, 1, SOFA_W, SOFA_H - 1, FABRIC)
    rect(g, 2, 1, SOFA_W - 1, 2, FABRIC_HI)
    for c in SOFA_SEAT_COLS[1:]:  # the seams between cushions, a column left of each boundary
        put(g, c - 3, 1, OUTLINE)
    rect(g, 1, ridge - 1, SOFA_W - 1, ridge, OUTLINE)
    rect(g, 1, ridge, SOFA_W - 1, ridge + SOFA_RIDGE_ROWS, FABRIC_HI)
    rect(g, 1, SOFA_H - 2, SOFA_W - 1, SOFA_H - 1, FABRIC_SH)
    for y in range(1, SOFA_H - 1):
        put(g, 0, y, OUTLINE)
        put(g, SOFA_W - 1, y, OUTLINE)
    put(g, 1, 1, OUTLINE)
    rect(g, 1, 0, SOFA_W - 1, 1, OUTLINE)
    rect(g, 1, SOFA_H - 1, SOFA_W - 1, SOFA_H, OUTLINE)
    return g


# ---- furniture: screens, pantry, pods and small fixtures --------------------------
CORK, CORK_SH, STEEL, STEEL_SH = "Δ", "π", "K", "ξ"


# ---- the characters ----------------------------------------------------------
# A chibi figure: a big head over a small body, one near-black line round the
# whole silhouette. Every pose is a bald BODY, the face or the back of the head
# but never a scalp, that a hairstyle's layers dress per view: `behind` under
# the body, `over` on top. The union is outlined last, so no line runs between
# hair and face.
FIG_W = 8 * S
FIG_CX = 15.5
# Rows every standing and front- or back-view seated pose shares: the face
# (the eyes and mouth hang off it) and the shoulders.
FACE_TOP, FACE_BOTTOM, SHOULDER_Y = 12, 26, 26
# A seated shirt ends on the seat, where the base's hand row starts; a
# standing one on the belt.
SEAT_Y, BELT_Y = 36, 38
HAIR_KEYS = {HAIR, HAIR_SH, HAIR_LT, HAIR_HI}
SKIN_KEYS = {SKIN, SKIN_SH}


def union_outline(g, k=SILHOUETTE):
    """One line round everything drawn, whatever its material."""
    outline(g, {c for row in g for c in row} - {T, k}, k)


def disc(g, cx, cy, r, k=HAIR):
    for y in range(max(0, int(cy - r) - 1), min(len(g), int(cy + r) + 2)):
        for x in range(1, len(g[0]) - 1):
            if (x + 0.5 - cx) ** 2 + (y + 0.5 - cy) ** 2 <= r * r:
                g[y][x] = k


def ellipse(g, cx, cy, rx, ry, k=HAIR):
    for y in range(max(0, int(cy - ry) - 1), min(len(g), int(cy + ry) + 2)):
        for x in range(1, len(g[0]) - 1):
            if ((x + 0.5 - cx) / rx) ** 2 + ((y + 0.5 - cy) / ry) ** 2 <= 1:
                g[y][x] = k


def tuft(g, bx, by, ang, length, half):
    """A tapered tuft from (bx, by) along `ang` degrees (screen: -90 is up)."""
    ux, uy = math.cos(math.radians(ang)), math.sin(math.radians(ang))
    for y in range(len(g)):
        for x in range(1, len(g[0]) - 1):
            dx, dy = x + 0.5 - bx, y + 0.5 - by
            u, v = dx * ux + dy * uy, -dx * uy + dy * ux
            if 0 <= u <= length and abs(v) <= half * (1 - u / length) + 0.35:
                g[y][x] = HAIR


def lock(g, x0, y0, x1, y1, w0, w1):
    """A lock from (x0, y0) to (x1, y1), `w0` wide at the root tapering to `w1`:
    long hair's fall and a ponytail's tail."""
    n = int(max(abs(x1 - x0), abs(y1 - y0)) * 2) + 1
    for i in range(n + 1):
        t = i / n
        cx, cy, hw = x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, (w0 + (w1 - w0) * t) / 2
        for y in range(int(cy) - 1, int(cy) + 2):
            for x in range(int(cx - hw) - 1, int(cx + hw) + 2):
                if 1 <= x < len(g[0]) - 1 and 0 <= y < len(g) and abs(x + 0.5 - cx) <= hw and abs(y + 0.5 - cy) <= 0.8:
                    g[y][x] = HAIR


def curl(g, cx, cy, r):
    """One round curl lit on its own top-left, its lower-right rim in shade:
    drawn back to front, each stands apart from the one behind it."""
    for y in range(max(0, int(cy - r) - 1), min(len(g), int(cy + r) + 2)):
        for x in range(max(1, int(cx - r) - 1), min(len(g[0]) - 1, int(cx + r) + 2)):
            dx, dy = x + 0.5 - cx, y + 0.5 - cy
            d = math.hypot(dx, dy)
            if d <= r:
                t = (dx + dy) / r
                g[y][x] = HAIR_SH if (d > r - 1.1 and t > -0.1) else HAIR_LT if t < -0.5 else HAIR


def rim_light(g, strands=(), sparkle=(), lit_top=True):
    """Every bump's top-left edge catches the light and its lower-right edge
    falls into shade, so a silhouette reads as locks rather than a helmet. A
    fringe passes `lit_top=False`: its top edge sits under the mop, unlit."""
    mop = {(x, y) for y in range(len(g)) for x in range(len(g[0])) if g[y][x] == HAIR}
    for x, y in mop:
        top_open = (x, y - 2) not in mop
        if (lit_top or not top_open) and ((x - 1, y - 1) not in mop or top_open or (x - 2, y) not in mop):
            g[y][x] = HAIR_LT
        elif (x + 1, y + 1) not in mop or (x + 2, y) not in mop:
            g[y][x] = HAIR_SH
    for x, y in strands:
        if (x, y) in mop:
            g[y][x] = HAIR_SH
    for x, y in sparkle:
        if (x, y) in mop:
            g[y][x] = HAIR_HI


def terminator(g, cx, cy, r, from_y=0):
    """A head turning from the light: hair past a circle offset up and west,
    toward the light, drops a shade, so the mop reads as a ball."""
    lx, ly = cx - 3.0, cy - 3.5
    for y in range(from_y, len(g)):
        for x in range(len(g[0])):
            if g[y][x] == HAIR and math.hypot(x + 0.5 - lx, y + 0.5 - ly) > r:
                g[y][x] = HAIR_SH


def comb(g, lines, dy=0):
    """Comb lines in hair shade, the way the hair is drawn."""
    for pts in lines:
        for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
            n = max(abs(x1 - x0), abs(y1 - y0))
            for i in range(n + 1):
                x, y = round(x0 + (x1 - x0) * i / n), round(y0 + (y1 - y0) * i / n) + dy
                if g[y][x] in (HAIR, HAIR_LT):
                    g[y][x] = HAIR_SH


def fringe(g, tips, top, dy=0):
    """A fringe cut straight: per column, hair from `top` down to its tip row."""
    for x, tip in tips.items():
        for y in range(top + dy, tip + dy):
            g[y][x] = HAIR


def fringe_locks(g, locks, top, dy=0):
    """A fringe of separate locks, each a wedge from the hairline to its tip, so
    the brow shows between them."""
    for cx, tip, half in locks:
        tip = int(round(tip)) + dy
        for y in range(top + dy, tip):
            w = half * (tip - y) / (tip - top - dy)
            for x in range(int(cx - w - 0.5), int(cx + w + 1.5)):
                if 1 <= x < len(g[0]) - 1 and abs(x + 0.5 - cx) <= w + 0.5:
                    g[y][x] = HAIR


def ball(g, cx, cy, r):
    """A bun: a round of its own, its rim dark where it sits on the head."""
    for y in range(len(g)):
        for x in range(1, len(g[0]) - 1):
            d = math.hypot(x + 0.5 - cx, y + 0.5 - cy)
            if r <= d < r + 1.0 and g[y][x] != T and (y + 0.5 > cy + 1 or x + 0.5 > cx + 1 and cy > 8):
                g[y][x] = SILHOUETTE
    curl(g, cx, cy, r)


def paste(dst, src):
    for y in range(min(len(dst), len(src))):
        for x in range(len(dst[0])):
            if src[y][x] != T:
                dst[y][x] = src[y][x]


# ---- hairstyles ----------------------------------------------------------------
# Each style draws four views on its own layer canvas, the tallest pose's rows
# plus HAIR_HEADROOM above for a bun or a tuft: `front` and `side` as
# (behind, over) pairs, `back` and `crown` as one layer over the body. Every
# coordinate below is on the pose grid; `o` shifts it onto the layer.
HAIR_HEADROOM = 6
LAYER_H = 12 * S + HAIR_HEADROOM
o = HAIR_HEADROOM
# The skull in profile, and the head seen from above as it lies on the arms.
SIDE_CX, SIDE_CY = 14.0, 12.5
CROWN_CX, CROWN_CY, CROWN_R = 15.5, 16.5, 10.0


def layer():
    return canvas(FIG_W, LAYER_H)


def lit(g, *a, **k):
    rim_light(g, *a, **k)
    return g


def front_fringe(tips):
    f = layer()
    fringe(f, tips, FACE_TOP - 4, o)
    return lit(f, lit_top=False)


def front_locks(locks):
    f = layer()
    fringe_locks(f, locks, FACE_TOP - 4, o)
    return lit(f, lit_top=False)


def side_locks(locks, top=8):
    f = layer()
    fringe_locks(f, locks, top, o)
    return lit(f, lit_top=False)


def nape_cut(g, half=6, rows=(20, 23)):
    """Short hair stops above the nape: clear its middle so the neck shows."""
    for y in range(rows[0] + o, rows[1] + o):
        for x in range(1, FIG_W - 1):
            if g[y][x] != T and abs(x + 0.5 - FIG_CX) < half:
                g[y][x] = T


def crown_base(r_extra=0.0):
    c = layer()
    disc(c, CROWN_CX, CROWN_CY + o, CROWN_R + r_extra)
    return c


def crown_finish(c, r=CROWN_R):
    rim_light(c)
    terminator(c, CROWN_CX, CROWN_CY + o, r + 1.5)
    return c


MOP_PUFFS = [(FIG_CX, 12.0, 10.8), (6.8, 6.0, 3.6), (10.6, 3.8, 3.4), (15.2, 2.9, 3.4), (19.8, 3.4, 3.4),
             (24.2, 5.8, 3.6), (4.6, 10.6, 3.6), (26.4, 10.6, 3.6), (4.4, 15.0, 3.4), (26.6, 15.0, 3.4),
             (5.6, 18.6, 2.8), (25.4, 18.6, 2.8)]


def style_mop():
    """A round cloud of a mop (the reference's elder)."""
    b = layer()
    for px, py, r in MOP_PUFFS:
        disc(b, px, py + o, r)
    rim_light(b, [(11, 7 + o), (12, 8 + o), (19, 6 + o), (19, 7 + o), (23, 10 + o), (7, 10 + o)],
              [(9, 3 + o), (10, 3 + o), (14, 2 + o), (15, 2 + o)])
    back = layer()
    for px, py, r in MOP_PUFFS + [(10.5, 21.5, 3.2), (15.5, 22.2, 3.2), (20.5, 21.5, 3.2)]:
        disc(back, px, py + (0.5 if r > 10 else 0) + o, r)
    rim_light(back, [], [(9, 3 + o), (10, 3 + o), (14, 2 + o), (15, 2 + o)])
    terminator(back, FIG_CX, 12.5 + o, 12.5, 8 + o)
    comb(back, [((11, 7), (12, 10)), ((19, 6), (18, 9)), ((15, 11), (16, 14))], o)
    sb = layer()
    for px, py, r in ((SIDE_CX, SIDE_CY, 11.2), (8, 5, 3.6), (13, 3, 3.6), (18.5, 3.5, 3.4), (22.5, 6.2, 3.2),
                      (4.5, 11, 3.8), (4.5, 16, 3.6), (6.5, 20, 3.2), (10, 21.5, 3.0)):
        disc(sb, px, py + o, r)
    rim_light(sb, [(10, 8 + o), (16, 7 + o), (8, 14 + o)], [(12, 2 + o), (13, 2 + o)])
    c = crown_base(1.2)
    for a in range(0, 360, 40):
        disc(c, CROWN_CX + math.cos(math.radians(a)) * CROWN_R, CROWN_CY + o + math.sin(math.radians(a)) * CROWN_R, 3.2)
    return {"front": (b, front_fringe({7: 16, 8: 15, 9: 14, 10: 13, 11: 13, 12: 14, 13: 13, 14: 12, 15: 13, 16: 13,
                                       17: 12, 18: 13, 19: 14, 20: 13, 21: 13, 22: 14, 23: 15, 24: 16})),
            "back": back,
            "side": (sb, side_locks([(19.5, 13, 2.2), (22.5, 12.5, 2.2), (24.5, 11.5, 1.6)])),
            "crown": crown_finish(c, CROWN_R + 1.2), "ears": False}


MESSY_TUFTS = ((8.5, 5.0, -125, 3.2, 2.8), (12.0, 2.8, -100, 2.8, 2.6), (16.0, 2.4, -80, 2.8, 2.6),
               (20.0, 3.2, -60, 3.2, 2.8), (23.5, 6.0, -35, 3.0, 2.6), (6.0, 8.5, -150, 2.6, 2.4),
               (26.0, 9.8, -8, 2.4, 2.2))


def style_messy():
    """The mop with short tufts breaking its top and sides (the reference's
    standing elder)."""
    b = layer()
    for px, py, r in ((FIG_CX, 12.5, 10.6), (5.4, 13.0, 3.9), (26.0, 13.0, 3.9), (5.8, 17.4, 3.0), (25.6, 17.4, 3.0)):
        disc(b, px, py + o, r)
    for t in MESSY_TUFTS:
        tuft(b, t[0], t[1] + o, *t[2:])
    back = [row[:] for row in b]
    for t in ((10.0, 21.0, 100, 3.0, 2.2), (14.0, 22.0, 85, 3.4, 2.4), (18.0, 22.0, 95, 3.2, 2.4), (22.0, 21.0, 80, 3.0, 2.2)):
        tuft(back, t[0], t[1] + o, *t[2:])
    for px, py, r in ((6.0, 17.6, 3.0), (25.4, 17.6, 3.0)):
        disc(back, px, py + o, r)
    strands = [(12, 6 + o), (13, 7 + o), (18, 5 + o), (18, 6 + o), (21, 7 + o), (9, 8 + o)]
    rim_light(b, strands, [(10, 3 + o), (11, 3 + o), (15, 2 + o), (16, 2 + o)])
    rim_light(back, strands + [(10, 12 + o), (16, 15 + o)], [(10, 3 + o), (11, 3 + o), (15, 2 + o), (16, 2 + o)])
    sb = layer()
    for px, py, r in ((SIDE_CX, SIDE_CY, 11.0), (4.8, 13, 3.8), (6.2, 18.5, 3.2)):
        disc(sb, px, py + o, r)
    for t in ((9, 4, -135, 3.4, 2.8), (13.5, 2.4, -105, 3.0, 2.6), (18.5, 2.8, -75, 3.0, 2.6),
              (22.5, 5.5, -40, 3.0, 2.4), (4.5, 8.5, -165, 3.0, 2.4), (5.5, 19.5, 160, 3.0, 2.2)):
        tuft(sb, t[0], t[1] + o, *t[2:])
    rim_light(sb, [(11, 7 + o), (17, 6 + o)], [(12, 2 + o), (13, 2 + o)])
    c = crown_base(1.2)
    for a in range(200, 350, 30):
        tuft(c, CROWN_CX + math.cos(math.radians(a)) * CROWN_R * 0.9,
             CROWN_CY + o + math.sin(math.radians(a)) * CROWN_R * 0.9, a, 3.2, 2.6)
    return {"front": (b, front_fringe({7: 16, 8: 15, 9: 13, 10: 13, 11: 14, 12: 16, 13: 13, 14: 12, 15: 12, 16: 13,
                                       17: 13, 18: 15, 19: 13, 20: 12, 21: 13, 22: 14, 23: 15, 24: 16})),
            "back": back,
            "side": (sb, side_locks([(19.5, 13.5, 2.2), (22.5, 12, 2.2), (25, 13, 1.6)])),
            "crown": crown_finish(c, CROWN_R + 1.2), "ears": False}


def style_side_part():
    """Less volume, parted west of centre, the fringe swept east in one sheet."""
    b = layer()
    for px, py, r in ((FIG_CX, 12.5, 10.2), (6.0, 14.0, 3.6), (25.0, 14.0, 3.8), (6.4, 18.0, 2.6), (24.8, 18.2, 2.8)):
        disc(b, px, py + o, r)
    rim_light(b, [(11, 2 + o), (11, 3 + o), (11, 4 + o), (10, 5 + o), (10, 6 + o)], [(15, 3 + o), (16, 3 + o)])
    back = layer()
    for px, py, r in ((FIG_CX, 12.8, 10.4), (6.0, 14.0, 3.6), (25.0, 14.0, 3.8), (7.0, 18.4, 3.0), (24.2, 18.4, 3.2)):
        disc(back, px, py + o, r)
    rect(back, 9, 18 + o, 23, 22 + o, HAIR)
    rim_light(back, [], [(15, 3 + o), (16, 3 + o)])
    terminator(back, FIG_CX, 12.8 + o, 12.0, 8 + o)
    comb(back, [((12, 4), (10, 9), (9, 14)), ((17, 4), (19, 9), (21, 13))], o)
    sb = layer()
    disc(sb, SIDE_CX, SIDE_CY + o, 10.8)
    ellipse(sb, 7, 17 + o, 4, 5)
    rim_light(sb, [(10, 5 + o), (8, 9 + o), (7, 13 + o)], [(15, 2 + o), (16, 2 + o)])
    c = crown_finish(crown_base())
    for y in range(int(CROWN_CY - CROWN_R) + 1 + o, int(CROWN_CY) + 2 + o):  # the part
        if c[y][int(CROWN_CX) - 2] != T:
            c[y][int(CROWN_CX) - 2] = HAIR_SH
    return {"front": (b, front_fringe({7: 14, 8: 13, 9: 12, 10: 12, 11: 12, 12: 12, 13: 13, 14: 13, 15: 13, 16: 14,
                                       17: 14, 18: 14, 19: 15, 20: 15, 21: 15, 22: 16, 23: 16, 24: 17})),
            "back": back,
            "side": (sb, side_locks([(18.5, 11.5, 2.4), (21.5, 12.5, 2.6), (24.5, 13.5, 2.2)], top=7)),
            "crown": c, "ears": False}


def style_long():
    """Long and parted in the middle, falling behind the shoulders to the
    chest in pointed ends (the reference's blonde)."""
    b = layer()
    ellipse(b, FIG_CX, 12.0 + o, 11.2, 11.0)
    for y in range(12 + o, 35 + o):
        spread = (y - 12 - o) / 23
        rect(b, int(3 - spread * 1.5), y, int(29 + spread * 1.5), y + 1, HAIR)
    for x0 in (1, 5, 9, 22, 26, 30):
        for i in range(3):
            rect(b, max(1, x0 - 2 + i), 35 + o + i, min(FIG_W - 1, x0 + 2 - i), 36 + o + i, HAIR)
    rim_light(b, [(9, 6 + o), (10, 7 + o), (21, 6 + o), (22, 7 + o), (4, 22 + o), (4, 26 + o), (27, 22 + o), (27, 26 + o)],
              [(10, 3 + o), (11, 3 + o)])
    back = layer()
    ellipse(back, FIG_CX, 12.2 + o, 11.2, 11.0)
    for y in range(12 + o, 37 + o):
        spread = (y - 12 - o) / 25
        rect(back, int(4 - spread * 2), y, int(28 + spread * 2), y + 1, HAIR)
    for x0 in (4, 9, 14, 19, 24, 28):
        for i in range(3):
            rect(back, max(1, x0 - 2 + i), 37 + o + i, min(FIG_W - 1, x0 + 2 - i), 38 + o + i, HAIR)
    rim_light(back, [], [(10, 3 + o), (11, 3 + o)])
    terminator(back, FIG_CX, 20.0 + o, 19.0, 20 + o)
    for x in range(7, 25):  # a band of sheen across the crown
        y = 7 + o + abs(x - 15) // 4
        if back[y][x] == HAIR:
            back[y][x] = HAIR_LT
    comb(back, [((9, 12), (8, 22), (7, 33)), ((15, 14), (15, 24), (14, 35)), ((21, 12), (22, 22), (23, 33)),
                ((12, 26), (11, 34)), ((19, 26), (20, 34))], o)
    sb = layer()
    disc(sb, SIDE_CX, SIDE_CY + o, 11.0)
    lock(sb, 7.5, 12 + o, 5.5, 38 + o, 9, 6)
    rim_light(sb, [(9, 18 + o), (8, 24 + o), (7, 30 + o), (11, 7 + o)], [(14, 2 + o), (15, 2 + o)])
    so = side_locks([(19.5, 13, 2.2), (22.5, 12.5, 2.2), (24.5, 12, 1.6)])
    strand = layer()
    lock(strand, 15, 14 + o, 15.5, 24 + o, 3, 2)
    paste(so, lit(strand))
    c = layer()
    ellipse(c, CROWN_CX, CROWN_CY + 4 + o, 12.5, 13.5)
    disc(c, CROWN_CX, CROWN_CY + o, CROWN_R)
    crown_finish(c)
    for x0 in (6, 11, 16, 21, 26):
        for y in range(int(CROWN_CY + 4) + o, int(CROWN_CY + 16) + o):
            if c[y][x0] in (HAIR, HAIR_LT):
                c[y][x0] = HAIR_SH
    return {"front": (b, front_fringe({7: 20, 8: 17, 9: 15, 10: 14, 11: 13, 12: 12, 13: 12, 14: 11, 17: 11, 18: 12,
                                       19: 12, 20: 13, 21: 14, 22: 15, 23: 17, 24: 20})),
            "back": back, "side": (sb, so), "crown": c, "ears": False}


def style_bun():
    """Pulled back tight into a bun on the crown (the reference's grey updo):
    combed lines run up to it, the ears show, the bun rises into the headroom."""
    b = layer()
    ellipse(b, FIG_CX, 13.0 + o, 10.4, 9.6)
    rim_light(b, [(11, 6 + o), (11, 8 + o), (20, 6 + o), (20, 8 + o)])
    ball(b, FIG_CX, 2.6 + o, 4.0)
    back = layer()
    ellipse(back, FIG_CX, 13.0 + o, 10.2, 9.6)
    nape_cut(back)
    rim_light(back)
    terminator(back, FIG_CX, 13.0 + o, 11.5, 8 + o)
    comb(back, [((8, 17), (10, 12), (13, 8)), ((15, 19), (15, 13), (15, 8)), ((23, 17), (21, 12), (18, 8))], o)
    ball(back, FIG_CX, 2.6 + o, 4.0)
    sb = layer()
    disc(sb, SIDE_CX, SIDE_CY + 0.5 + o, 10.2)
    rim_light(sb, [(10, 6 + o), (8, 10 + o), (16, 5 + o)])
    ball(sb, 5.0, 5.5 + o, 4.0)
    c = crown_finish(crown_base())
    curl(c, CROWN_CX, CROWN_CY - CROWN_R + 3.5 + o, 3.8)
    return {"front": (b, front_fringe({7: 12, 8: 11, 9: 11, 10: 10, 11: 10, 12: 10, 13: 10, 14: 10, 15: 10, 16: 10,
                                       17: 10, 18: 10, 19: 10, 20: 10, 21: 10, 22: 11, 23: 11, 24: 12})),
            "back": back, "side": (sb, side_locks([(20.5, 11, 2.6), (23.5, 11, 2.2)], top=7)),
            "crown": c, "ears": True}


def style_crop():
    """Short and close to the skull, the fringe cut in points, the ears
    showing (the reference's man by the lift)."""
    b = layer()
    ellipse(b, FIG_CX, 13.0 + o, 10.2, 9.6)
    rim_light(b, [(12, 6 + o), (19, 6 + o)], [(12, 5 + o), (13, 5 + o)])
    back = layer()
    ellipse(back, FIG_CX, 13.2 + o, 10.2, 9.4)
    for y in range(20 + o, 24 + o):  # the nape tapers
        for x in range(1, FIG_W - 1):
            if back[y][x] == HAIR and abs(x + 0.5 - FIG_CX) > 7 - (y - 20 - o):
                back[y][x] = T
    rim_light(back, [(12, 7 + o), (19, 7 + o), (15, 12 + o)], [(12, 5 + o), (13, 5 + o)])
    sb = layer()
    disc(sb, SIDE_CX, SIDE_CY + 0.6 + o, 10.0)
    for y in range(18 + o, 24 + o):
        for x in range(1, FIG_W - 1):
            if sb[y][x] == HAIR and x > 11 - (y - 18 - o):
                sb[y][x] = T
    rim_light(sb, [(10, 7 + o), (16, 5 + o)], [(13, 3 + o), (14, 3 + o)])
    return {"front": (b, front_locks([(8.5, 13, 1.8), (11.5, 13.5, 2.0), (14.5, 12.5, 2.0), (17.5, 13.5, 2.0),
                                      (20.5, 12.5, 2.0), (23.0, 13, 1.6)])),
            "back": back, "side": (sb, side_locks([(20.5, 11.5, 2.0), (23.5, 11.5, 1.8)])),
            "crown": crown_finish(crown_base()), "ears": True}


def style_curls():
    """Tight curls in a close cap (the reference's curly hair): rows of small
    rounds, each lit on its own."""
    b = layer()
    ellipse(b, FIG_CX, 12.5 + o, 10.6, 10.0)
    rim_light(b)
    for cy, xs in ((2.8, (10.0, 13.5, 17.0, 20.5)), (5.2, (7.2, 10.6, 14.0, 17.4, 20.8, 24.0)),
                   (8.0, (5.4, 8.6, 12.0, 15.5, 19.0, 22.4, 25.6)), (11.2, (4.6, 26.4)), (14.4, (4.6, 26.4))):
        for cx in xs:
            curl(b, cx, cy + o, 2.3)
    f = layer()
    for cx in (8.4, 11.6, 14.8, 18.0, 21.2, 23.8):
        curl(f, cx, 11.0 + o, 2.0)
    back = layer()
    ellipse(back, FIG_CX, 13.2 + o, 10.4, 9.8)
    rim_light(back)
    for cy, xs in ((2.8, (10.0, 13.5, 17.0, 20.5)), (5.2, (7.2, 10.6, 14.0, 17.4, 20.8, 24.0)),
                   (8.0, (5.4, 8.6, 12.0, 15.5, 19.0, 22.4, 25.6)), (11.2, (4.6, 7.8, 11.2, 14.6, 18.0, 21.4, 24.6)),
                   (14.4, (4.8, 8.2, 11.6, 15.0, 18.4, 21.8, 25.2)), (17.6, (6.4, 9.8, 13.2, 16.6, 20.0, 23.6)),
                   (20.4, (9.6, 13.0, 16.4, 19.8))):
        for cx in xs:
            curl(back, cx, cy + o, 2.3)
    sb = layer()
    disc(sb, SIDE_CX, SIDE_CY + o, 10.8)
    rim_light(sb)
    for cy, xs in ((3.0, (9.5, 13, 16.5, 20)), (5.8, (6.5, 10, 13.5, 17, 20.5, 23.5)),
                   (8.8, (4.5, 7.8, 11.2, 14.6, 18, 21.4)), (12, (4, 7.2, 10.6)), (15.2, (4.2, 7.6, 11)),
                   (18.4, (5.4, 8.8)), (21, (8, 11.2))):
        for cx in xs:
            curl(sb, cx, cy + o, 2.3)
    so = layer()
    for cx in (20.5, 23.5):
        curl(so, cx, 10.5 + o, 2.0)
    c = crown_finish(crown_base())
    for rr, n in ((0, 1), (4.5, 6), (8.2, 10)):
        for i in range(n):
            a = 2 * math.pi * i / n
            curl(c, CROWN_CX + math.cos(a) * rr, CROWN_CY + o + math.sin(a) * rr, 2.3)
    return {"front": (b, f), "back": back, "side": (sb, so), "crown": c, "ears": False}


def style_shaggy():
    """A shaggy mop whose fringe falls in locks to the eyes (the reference's boy
    in green)."""
    b = layer()
    for px, py, r in ((FIG_CX, 12.0, 10.8), (5.2, 14.5, 3.8), (26.2, 14.5, 3.8), (5.6, 19.0, 2.8), (25.8, 19.0, 2.8)):
        disc(b, px, py + o, r)
    for t in ((9, 4.5, -120, 2.6, 2.6), (14.5, 2.4, -95, 2.4, 2.6), (20, 3.2, -65, 2.6, 2.6)):
        tuft(b, t[0], t[1] + o, *t[2:])
    back = [row[:] for row in b]
    for px, py, r in ((FIG_CX, 12.4, 10.8), (5.8, 19.0, 3.0), (25.6, 19.0, 3.0)):
        disc(back, px, py + o, r)
    for t in ((8.0, 20.5, 105, 4.0, 2.4), (12.0, 21.5, 92, 4.4, 2.6), (16.0, 22.0, 88, 4.6, 2.6),
              (20.0, 21.5, 85, 4.4, 2.6), (24.0, 20.5, 75, 4.0, 2.4)):
        tuft(back, t[0], t[1] + o, *t[2:])
    rim_light(b, [(12, 5 + o), (18, 5 + o)], [(12, 3 + o), (13, 3 + o)])
    rim_light(back, [(12, 6 + o), (18, 6 + o), (10, 18 + o), (14, 19 + o), (18, 19 + o), (22, 18 + o)],
              [(12, 3 + o), (13, 3 + o)])
    sb = layer()
    for px, py, r in ((SIDE_CX, SIDE_CY, 11.0), (4.8, 13.5, 3.8), (6, 19, 3.2), (10, 21.5, 2.8)):
        disc(sb, px, py + o, r)
    for t in ((10, 3.5, -125, 2.8, 2.6), (15, 2.2, -95, 2.6, 2.6), (20, 3.4, -65, 2.6, 2.4)):
        tuft(sb, t[0], t[1] + o, *t[2:])
    rim_light(sb, [(11, 7 + o), (17, 6 + o), (8, 14 + o)], [(13, 2 + o), (14, 2 + o)])
    c = crown_base(1.2)
    for a in range(200, 350, 30):
        tuft(c, CROWN_CX + math.cos(math.radians(a)) * CROWN_R * 0.9,
             CROWN_CY + o + math.sin(math.radians(a)) * CROWN_R * 0.9, a, 3.2, 2.6)
    return {"front": (b, front_locks([(7.5, 18, 1.8), (10.0, 16.5, 2.2), (13.0, 17.5, 2.4), (16.2, 16.5, 2.4),
                                      (19.2, 17.5, 2.4), (22.0, 16.5, 2.2), (24.4, 18, 1.8)])),
            "back": back,
            "side": (sb, side_locks([(17.5, 16, 2.2), (20.5, 17.5, 2.4), (23.5, 16.5, 2.2), (25.5, 15, 1.4)])),
            "crown": crown_finish(c, CROWN_R + 1.2), "ears": False}


def style_ponytail():
    """Gathered into a ponytail swept over the east shoulder."""
    b = layer()
    ellipse(b, FIG_CX, 12.5 + o, 10.6, 10.2)
    rim_light(b, [(12, 5 + o), (13, 6 + o)], [(11, 3 + o), (12, 3 + o)])
    f = front_fringe({7: 15, 8: 14, 9: 13, 10: 12, 11: 12, 12: 12, 13: 12, 14: 12, 15: 12, 16: 12, 17: 12,
                      18: 12, 19: 13, 20: 13, 21: 14, 22: 15, 23: 16, 24: 17})
    paste(f, tail(25, 17, 27, 33, 18))
    back = layer()
    ellipse(back, FIG_CX, 12.8 + o, 10.6, 10.0)
    nape_cut(back)
    rim_light(back)
    terminator(back, FIG_CX, 12.8 + o, 12.0, 8 + o)
    comb(back, [((8, 9), (14, 12), (22, 16)), ((10, 16), (16, 17), (22, 17)), ((13, 5), (18, 9), (23, 15))], o)
    paste(back, tail(24, 16, 27, 30, 17))
    sb = layer()
    disc(sb, SIDE_CX, SIDE_CY + 0.4 + o, 10.4)
    rim_light(sb, [(10, 6 + o), (8, 10 + o), (13, 9 + o), (16, 12 + o)])
    so = side_locks([(20.5, 11.5, 2.4), (23.5, 12, 2.0)])
    paste(so, tail(9, 17, 10, 33, 18))
    c = crown_finish(crown_base())
    paste(c, tail(CROWN_CX + 8, CROWN_CY + 2, CROWN_CX + 13, CROWN_CY + 12, CROWN_CY + 3))
    return {"front": (b, f), "back": back, "side": (sb, so), "crown": c, "ears": False}


def tail(x0, y0, x1, y1, tie_y):
    """A ponytail's tail, lit on its own, with its tie."""
    t = layer()
    lock(t, x0, y0 + o, x1, y1 + o, 5, 2)
    rim_light(t, [(round(x0 + (x1 - x0) * f), round(y0 + (y1 - y0) * f) + o) for f in (0.4, 0.7)])
    for x in range(int(min(x0, x1)) - 2, int(max(x0, x1)) + 3):
        if 0 <= x < FIG_W and t[int(tie_y) + o][x] != T:
            t[int(tie_y) + o][x] = HAIR_SH
    return t


# Per-agent hairstyles, by the name a pack registers them under.
HAIRSTYLES = {
    "mop": style_mop, "messy": style_messy, "side_part": style_side_part, "long": style_long,
    "bun": style_bun, "crop": style_crop, "curls": style_curls, "shaggy": style_shaggy,
    "ponytail": style_ponytail,
}


# ---- bodies ------------------------------------------------------------------------
# Drawn on the pose grid, shifted down `dy` rows onto a dressing canvas.
def mitt(g, x, y):
    """A hand: a skin block, shaded along its foot, rounded at the bottom."""
    rect(g, x, y, x + S, y + S, SKIN)
    rect(g, x, y + S - 1, x + S, y + S, SKIN_SH)
    g[y + S - 1][x] = T
    g[y + S - 1][x + S - 1] = T


def face_front(g, dy, ears):
    """The face: round, shaded east and along the chin, two tall eyes and a
    small mouth; the ears where a style leaves them bare."""
    for y in range(FACE_TOP - 2, FACE_BOTTOM):
        for x in range(7, 25):
            nx = (x + 0.5 - FIG_CX) / 9.0
            ny = max(0.0, (y + 0.5 - (FACE_TOP + 6)) / (FACE_BOTTOM - FACE_TOP - 6))
            if nx * nx + ny * ny <= 1.0:
                g[y + dy][x] = SKIN_SH if (x >= 21 or y >= FACE_BOTTOM - 2) else SKIN
    if ears:
        for x, inner in ((5, 6), (25, 25)):
            rect(g, x, FACE_TOP + 5 + dy, x + 2, FACE_TOP + 9 + dy, SKIN)
            rect(g, inner, FACE_TOP + 6 + dy, inner + 1, FACE_TOP + 8 + dy, SKIN_SH)
    eye_y = FACE_TOP + 5 + dy
    rect(g, 11, eye_y, 13, eye_y + 4, EYE)
    rect(g, 19, eye_y, 21, eye_y + 4, EYE)
    rect(g, 15, FACE_BOTTOM - 4 + dy, 17, FACE_BOTTOM - 3 + dy, SKIN_DK)


def nape(g, dy, ears):
    """The back of the head under the hair: its round, the neck, the ears."""
    ellipse(g, FIG_CX, 17.0 + dy, 9.0, 8.6, SKIN_SH)
    rect(g, 12, 23 + dy, 20, 27 + dy, SKIN_DK)
    rect(g, 13, 23 + dy, 19, 26 + dy, SKIN_SH)
    if ears:
        for x in (5, 25):
            rect(g, x, 17 + dy, x + 2, 21 + dy, SKIN)
            rect(g, x + (1 if x < 16 else 0), 18 + dy, x + (2 if x < 16 else 1), 20 + dy, SKIN_SH)


def side_face(g, dy, ears):
    """The profile, facing east: brow, a nose bump, the mouth, a chin rounding
    back to the jaw, one eye; the ear mid-head."""
    rows = {10: (15, 24), 11: (14, 25), 12: (13, 25), 13: (13, 25), 14: (13, 25), 15: (13, 26),
            16: (13, 27), 17: (13, 27), 18: (13, 26), 19: (13, 25), 20: (13, 25), 21: (13, 25),
            22: (14, 25), 23: (15, 24), 24: (16, 23), 25: (17, 21)}
    for y, (x0, x1) in rows.items():
        for x in range(x0, x1):
            g[y + dy][x] = SKIN_SH if (y >= 24 or x < 15) else SKIN
    rect(g, 21, 15 + dy, 23, 19 + dy, EYE)
    rect(g, 23, 22 + dy, 25, 23 + dy, SKIN_DK)
    rect(g, 13, 24 + dy, 19, 28 + dy, SKIN_DK)  # the neck
    if ears:
        rect(g, 12, 16 + dy, 15, 20 + dy, SKIN)
        rect(g, 13, 17 + dy, 14, 19 + dy, SKIN_SH)


def shirt(g, dy, bottom, back=False, sleeves=True):
    """The shirt from the shoulders to `bottom`, lit on the west, with sleeves
    hanging at the sides; the collar in the chin's shadow, or, from behind, a
    fold at the foot of the back."""
    rect(g, 9, SHOULDER_Y + dy, 23, SHOULDER_Y + 1 + dy, SHIRT)
    rect(g, 6, SHOULDER_Y + 1 + dy, 26, bottom + dy, SHIRT)
    rect(g, 20, SHOULDER_Y + 1 + dy, 26, bottom + dy, SHIRT_SH)
    rect(g, 6, SHOULDER_Y + 1 + dy, 7, bottom + dy, SHIRT_LT)
    if sleeves:
        rect(g, 5, SHOULDER_Y + 2 + dy, 27, 34 + dy, SHIRT)
        rect(g, 22, SHOULDER_Y + 2 + dy, 27, 34 + dy, SHIRT_SH)
        rect(g, 5, SHOULDER_Y + 2 + dy, 6, 34 + dy, SHIRT_LT)
        rect(g, 8, SHOULDER_Y + 3 + dy, 9, bottom + dy, SHIRT_SH)
        rect(g, 23, SHOULDER_Y + 3 + dy, 24, bottom + dy, SHIRT_OUT)
    if back:
        rect(g, 15, bottom - 5 + dy, 17, bottom + dy, SHIRT_SH)
    else:
        rect(g, 12, SHOULDER_Y + dy, 20, SHOULDER_Y + 1 + dy, SKIN_DK)
        rect(g, 13, SHOULDER_Y + 1 + dy, 19, SHOULDER_Y + 2 + dy, SKIN_DK)


def hands_level(g, dy):
    for x in (4, 24):
        mitt(g, x, SEAT_Y + dy)


def hands_typing(frame):
    """Both hands low on the keys, one pressing lower than the other, swapping
    each frame as the base frames do."""
    def draw(g, dy):
        up, down = (4, 24) if frame == 0 else (24, 4)
        rect(g, down, SEAT_Y + dy, down + S, SEAT_Y + 2 + dy, SHIRT_SH)
        mitt(g, up, SEAT_Y + dy)
        mitt(g, down, SEAT_Y + 2 + dy)
    return draw


def legs(g, dy, stride, view):
    """Hips, two legs and shoes: the stepping foot reaches the canvas foot a
    little outward, the other lifts its heel."""
    rect(g, 8, BELT_Y + dy, 24, BELT_Y + 2 + dy, PANTS)
    for side, x0 in ((-1, 8), (1, 17)):
        out = stride == side
        foot = 12 * S - (2 if stride != 0 and not out else 0)
        dx = side if out else 0
        rect(g, x0 + dx, BELT_Y + 2 + dy, x0 + 7 + dx, foot - 3 + dy, PANTS)
        rect(g, x0 + 5 + dx, BELT_Y + dy, x0 + 7 + dx, foot - 3 + dy, PANTS_SH)
        rect(g, x0 - 1 + dx, foot - 3 + dy, x0 + 7 + dx, foot + dy, SHOE)
        if view == "front":
            rect(g, x0 + dx, foot - 3 + dy, x0 + 6 + dx, foot - 2 + dy, SHOE_HI)


def arms_hanging(g, dy, stride):
    """Sleeves down the sides; a walker's arms swing against their legs, so the
    forward hand hangs lower."""
    for side, x0 in ((-1, 4), (1, 24)):
        drop = 0 if stride == 0 else (2 if side != stride else -1)
        rect(g, x0 + (1 if side < 0 else 0), SHOULDER_Y + 2 + dy, x0 + (4 if side < 0 else 3), 34 + drop + dy,
             SHIRT if side < 0 else SHIRT_SH)
        if side < 0:
            rect(g, x0 + 1, SHOULDER_Y + 2 + dy, x0 + 2, 34 + drop + dy, SHIRT_LT)
        mitt(g, x0, 34 + drop + dy)


def mug(g, x, y):
    """A mug with coffee showing, shaded east, its handle east, steam rising."""
    rect(g, x, y, x + 6, y + 7, MUG)
    rect(g, x + 4, y, x + 6, y + 7, MUG_SH)
    rect(g, x + 1, y, x + 5, y + 1, COFFEE)
    rect(g, x + 6, y + 2, x + 8, y + 5, MUG_SH)
    g[y + 3][x + 6] = T
    for sx, sy in ((2, -2), (3, -3), (2, -4)):
        g[y + sy][x + sx] = OFFWHITE


def arms_mug_both(g, dy, stride):
    for x0 in (5, 25):  # upper arms bent in at the elbow
        rect(g, x0, SHOULDER_Y + 2 + dy, x0 + 3, 32 + dy, SHIRT if x0 < 16 else SHIRT_SH)
    rect(g, 7, 31 + dy, 12, 35 + dy, SHIRT)
    rect(g, 20, 31 + dy, 25, 35 + dy, SHIRT_SH)
    mug(g, 13, 29 + dy)
    mitt(g, 10, 31 + dy)
    mitt(g, 19, 31 + dy)


def arms_mug_east(g, dy, stride):
    drop = 2 if stride == 1 else -1
    rect(g, 5, SHOULDER_Y + 2 + dy, 8, 34 + drop + dy, SHIRT)
    rect(g, 5, SHOULDER_Y + 2 + dy, 6, 34 + drop + dy, SHIRT_LT)
    mitt(g, 4, 34 + drop + dy)
    rect(g, 24, SHOULDER_Y + 2 + dy, 27, 32 + dy, SHIRT_SH)
    rect(g, 21, 29 + dy, 26, 33 + dy, SHIRT_SH)
    mug(g, 16, 27 + dy)
    mitt(g, 21, 29 + dy)


def side_body(g, dy):
    """Seated in profile: a narrow torso, the near arm reaching forward, the lap
    to the knee."""
    rect(g, 9, 27 + dy, 22, SEAT_Y + dy, SHIRT)
    rect(g, 9, 27 + dy, 11, SEAT_Y + dy, SHIRT_LT)
    rect(g, 19, 28 + dy, 22, SEAT_Y + dy, SHIRT_SH)
    rect(g, 12, 28 + dy, 17, 32 + dy, SHIRT_SH)
    rect(g, 16, 30 + dy, 24, 34 + dy, SHIRT)
    rect(g, 16, 33 + dy, 24, 34 + dy, SHIRT_SH)
    mitt(g, 24, 30 + dy)
    rect(g, 9, SEAT_Y + dy, 28, 10 * S + dy, PANTS)
    rect(g, 9, SEAT_Y + dy, 28, SEAT_Y + 1 + dy, PANTS_LT)
    rect(g, 25, SEAT_Y + 1 + dy, 28, 10 * S + dy, PANTS_SH)


def asleep_body(g, dy, lean):
    """Face down on folded arms: shoulders rising behind the head, forearms
    crossed under it on the desk, hands tucked at the elbows, the torso behind
    the arms, the thighs at the chair's edge."""
    rect(g, 4 + lean, 12 + dy, 28 + lean, 26 + dy, SHIRT)
    rect(g, 22 + lean, 12 + dy, 28 + lean, 26 + dy, SHIRT_SH)
    rect(g, 4 + lean, 12 + dy, 6 + lean, 26 + dy, SHIRT_LT)
    rect(g, 1, 23 + dy, 10, 31 + dy, SHIRT)
    rect(g, 22, 23 + dy, 31, 31 + dy, SHIRT_SH)
    rect(g, 1, 23 + dy, 10, 24 + dy, SHIRT_LT)
    rect(g, 8, 26 + dy, 24, 31 + dy, SKIN)
    rect(g, 8, 30 + dy, 24, 31 + dy, SKIN_SH)
    rect(g, 15, 26 + dy, 17, 31 + dy, SKIN_SH)
    mitt(g, 1, 20 + dy)
    mitt(g, 27, 20 + dy)
    rect(g, 4, 31 + dy, 28, 36 + dy, SHIRT)
    rect(g, 22, 31 + dy, 28, 36 + dy, SHIRT_SH)
    rect(g, 1, 34 + dy, 9, 38 + dy, PANTS)
    rect(g, 23, 34 + dy, 31, 38 + dy, PANTS_SH)


# ---- poses -------------------------------------------------------------------------
# name: (height in logical rows, view, body). A `crown` pose's body also takes
# where its head lies: the crown layer is drawn there instead of at rest.
def seated_front(hands):
    return lambda g, dy: (shirt(g, dy, SEAT_Y), hands(g, dy))


def seated_rear(hands):
    return lambda g, dy: (shirt(g, dy, SEAT_Y, back=True), hands(g, dy))


def standing_body(stride, view, arms=arms_hanging):
    return lambda g, dy: (legs(g, dy, stride, view), shirt(g, dy, BELT_Y, back=view == "back", sleeves=False),
                          arms(g, dy, stride))


def couch_back(g, dy):
    rect(g, 9, SHOULDER_Y + dy, 23, SHOULDER_Y + 1 + dy, SHIRT)
    rect(g, 5, SHOULDER_Y + 1 + dy, 27, 9 * S + dy, SHIRT)
    rect(g, 21, SHOULDER_Y + 1 + dy, 27, 9 * S + dy, SHIRT_SH)
    rect(g, 5, SHOULDER_Y + 1 + dy, 7, 9 * S + dy, SHIRT_LT)


POSES = {
    "seated": (10, "front", seated_front(hands_level)),
    "typing_0": (11, "front", seated_front(hands_typing(0))),
    "typing_1": (11, "front", seated_front(hands_typing(1))),
    "seated_back": (10, "back", seated_rear(hands_level)),
    "typing_back_0": (11, "back", seated_rear(hands_typing(0))),
    "typing_back_1": (11, "back", seated_rear(hands_typing(1))),
    "back_couch": (9, "back", couch_back),
    "standing": (12, "front", standing_body(0, "front")),
    "walking_0": (12, "front", standing_body(-1, "front")),
    "walking_1": (12, "front", standing_body(1, "front")),
    "walking_back_0": (12, "back", standing_body(-1, "back")),
    "walking_back_1": (12, "back", standing_body(1, "back")),
    "holding_coffee": (12, "front", standing_body(0, "front", arms_mug_both)),
    "walking_coffee_0": (12, "front", standing_body(-1, "front", arms_mug_east)),
    "walking_coffee_1": (12, "front", standing_body(1, "front", arms_mug_east)),
    "side_seated": (10, "side", lambda g, dy: side_body(g, dy)),
    "seated_sleeping": (10, "crown", lambda g, dy: asleep_body(g, dy, 0)),
    "seated_sleeping_alt": (10, "crown", lambda g, dy: asleep_body(g, dy, 3)),
}
CHARACTER_HEADERS = {
    "seated": "Front view at rest, both hands level.",
    "typing_0": "Front view, typing, frame 0: the west hand level, the east pressing.",
    "typing_1": "Front view, typing, frame 1: the hands swap.",
    "seated_back": "Back view at rest, both hands level.",
    "typing_back_0": "Back view, typing, frame 0.",
    "typing_back_1": "Back view, typing, frame 1.",
    "back_couch": "On the couch, facing the windows: shoulders square over the seat back.",
    "standing": "Standing: arms at the sides.",
    "walking_0": "Walking, frame 0: the west foot out.",
    "walking_1": "Walking, frame 1: the east foot out.",
    "walking_back_0": "Walking away, frame 0.",
    "walking_back_1": "Walking away, frame 1.",
    "holding_coffee": "Standing with a steaming mug in both hands.",
    "walking_coffee_0": "Walking with a mug, frame 0.",
    "walking_coffee_1": "Walking with a mug, frame 1.",
    "side_seated": "Seated in profile, facing east, one arm reaching forward.",
    "seated_sleeping": "Asleep face-down on folded arms.",
    "seated_sleeping_alt": "Dozed off, slumped east.",
}
# The hairstyle every baked character wears until the renderer dresses each
# agent in its own.
BAKED_STYLE = "mop"
# Where a face-down head lies, as an offset from the crown's rest.
CROWN_SHIFT = {"seated_sleeping": (0, 0), "seated_sleeping_alt": (4, 1)}


def shade_by_skin(g):
    """Hair touching skin sits in the fringe's shadow."""
    for y in range(len(g)):
        for x in range(1, FIG_W - 1):
            if g[y][x] in (HAIR, HAIR_LT) and any(
                    0 <= y + ddy < len(g) and g[y + ddy][x + ddx] in SKIN_KEYS
                    for ddx, ddy in ((1, 0), (-1, 0), (0, 1))):
                g[y][x] = HAIR_SH


def shifted(layer_, dx, dy_):
    out = layer()
    for y in range(LAYER_H):
        for x in range(FIG_W):
            if layer_[y][x] != T and 0 <= y + dy_ < LAYER_H and 0 <= x + dx < FIG_W:
                out[y + dy_][x + dx] = layer_[y][x]
    return out


def dressed(pose, style):
    """`pose` dressed in `style`, the hair's headroom cut off: the frame the
    bundled pack bakes until the renderer composes hair itself."""
    rows, view, body = POSES[pose]
    look = HAIRSTYLES[style]()
    h = rows * S
    g = canvas(FIG_W, h + o)
    if view in ("front", "side"):
        behind, over = look[view]
    else:
        behind, over = None, look[view]
    if view == "crown":
        over = shifted(over, *CROWN_SHIFT[pose])
    if behind is not None:
        paste(g, behind[:h + o])
    body(g, o)
    if view == "front":
        face_front(g, o, look["ears"])
    elif view == "back":
        nape(g, o, look["ears"])
    elif view == "side":
        side_face(g, o, look["ears"])
    paste(g, over[:h + o])
    shade_by_skin(g)
    for y in range(o + 1):  # the headroom, and the outline's row at the top
        g[y] = [T] * FIG_W
    union_outline(g)
    return g[o:]


def desk_chair():
    """The task chair from behind: a padded back, rounded and lit along its top,
    a centre seam; the arm pads; the gas stem and a five-star base on casters."""
    g = canvas(FIG_W, 5 * S)
    for y in range(12):
        for x in range(7, 25):
            if ((x + 0.5 - FIG_CX) / 8.5) ** 2 + (max(0, 5 - y) / 5.5) ** 2 <= 1:
                g[y][x] = CHAIR
    for y in range(12):
        for x in range(FIG_W):
            if g[y][x] == CHAIR and (y == 0 or g[y - 1][x] == T):
                g[y][x] = CHAIR_RIM
    rect(g, 15, 2, 17, 11, SHADOW)
    for x0 in (3, 25):
        rect(g, x0, 7, x0 + 4, 11, CHAIR_BASE)
        rect(g, x0, 7, x0 + 4, 8, CHAIR_RIM)
    rect(g, 8, 11, 24, 13, CHAIR_BASE)
    rect(g, 15, 13, 17, 16, SHADOW)
    rect(g, 6, 16, 26, 17, CHAIR_BASE)
    for x0 in (5, 14, 24):
        rect(g, x0, 17, x0 + 3, 19, KEY_DK)
    union_outline(g)
    return g


# ---- the desks ---------------------------------------------------------------


def desk_wood(lift, seed):
    top, lip, legs, rows = desk_rows(lift)
    w, h = DESK_ART_W * S, rows * S
    ty, ly, gy, leg = top * S, lip * S, legs * S, DESK_LEG_W * S
    g = canvas(w, h)
    rng = random.Random(seed)
    rect(g, 0, ty, w, ly, WOOD)
    for y in range(ty + DESK_BOARD_ROWS, ly, DESK_BOARD_ROWS):
        rect(g, 0, y, w, y + 1, WOOD_SH)
    for _ in range(DESK_STREAKS):
        y = rng.randrange(ty + 2, ly - 2)
        if (y - ty) % DESK_BOARD_ROWS == 0:
            continue
        x0 = rng.randrange(2, w - 10)
        rect(g, x0, y, x0 + rng.randrange(5, 10), y + 1, WOOD_SH)
    rect(g, 0, ty, w, ty + 1, WOOD_LT)
    # front lip: a bright top edge, the face, a dark underside
    rect(g, 0, ly, w, gy, WOOD)
    rect(g, 0, ly, w, ly + 1, WOOD_HI)
    rect(g, 0, gy - 1, w, gy, WOOD_SH)
    # legs, dark on the inner side
    rect(g, 0, gy, leg, h, WOOD_SH)
    rect(g, w - leg, gy, w, h, WOOD_SH)
    rect(g, leg - 1, gy, leg, h, WOOD_DK)
    rect(g, w - leg, gy, w - leg + 1, h, WOOD_DK)
    return g, (w, h, ty, ly, gy)


def lamp_pool(g, cx, cy, rx, ry, y_min):
    for y in range(max(y_min, int(cy - ry)), int(cy + ry) + 1):
        for x in range(int(cx - rx), int(cx + rx) + 1):
            if 0 <= y < len(g) and 0 <= x < len(g[0]) and g[y][x] == WOOD:
                d = math.hypot((x - cx) / rx, (y - cy) / ry)
                if d < 0.6:
                    g[y][x] = POOL
                elif d < 1.0:
                    g[y][x] = WOOD_LT


def desk_south():
    """The viewer-facing desk: the monitor's back (casing, two vent rows,
    a badge) on its stand, a lamp west, papers and a mug east."""
    g, (w, h, ty, ly, gy) = desk_wood(0, 5)
    mx0, mx1, chin = DESK_MONITOR_X0 * S, DESK_MONITOR_X1 * S, DESK_CHIN_Y * S
    mid = (mx0 + mx1) // 2
    lamp_pool(g, 7, 13, 12, 7, ty + 1)
    # monitor back
    rect(g, mx0, 0, mx1, chin - 1, BEZEL)
    rect(g, mx0, 0, mx1, 1, SLATE)
    rect(g, mx0, 0, mx0 + 1, chin - 1, SLATE)
    rect(g, mx0, chin - 2, mx1, chin - 1, SHADOW)
    for vy in (4, 6):
        rect(g, mx0 + 6, vy, mx1 - 6, vy + 1, SHADOW)
    rect(g, mid - 2, 9, mid + 2, 11, SLATE)
    # stand
    rect(g, mid - 2, chin - 1, mid + 2, chin + 2, SHADOW)
    rect(g, mid - 6, chin + 1, mid + 6, chin + 3, BEZEL)
    rect(g, mid - 6, chin + 1, mid + 6, chin + 2, SLATE)
    # lamp, west wing: base, arm, shade, bulb
    rect(g, 2, 15, 8, 18, LAMP)
    rect(g, 2, 15, 8, 16, LAMP_HI)
    for t in range(8):
        put(g, 5 + t // 4, 14 - t, LAMP_HI)
    rect(g, 4, 3, 10, 6, LAMP)
    rect(g, 4, 3, 10, 4, LAMP_HI)
    rect(g, 5, 6, 9, 7, BULB)
    # papers and a mug, east wing
    rect(g, 46, 9, 54, 17, OFFWHITE_SH)
    rect(g, 45, 8, 53, 16, OFFWHITE)
    for i in range(3):
        rect(g, 46, 10 + i * 2, 51 - (i % 2) * 2, 11 + i * 2, PRINT)
    rect(g, 47, 18, 51, 22, MUG)
    rect(g, 47, 18, 51, 19, MUG_SH)
    put(g, 51, 19, MUG_SH)
    put(g, 51, 20, MUG_SH)
    outline(g, {BEZEL, SLATE}, SHADOW)
    return g


def desk_north():
    """The back-turned desk: the raised monitor's glass with a few lines
    of text, a keyboard band, a lamp east, papers and a mug west."""
    g, (w, h, ty, ly, gy) = desk_wood(DESK_NORTH_LIFT, 3)
    mx0, mx1, chin = DESK_MONITOR_X0 * S, DESK_MONITOR_X1 * S, DESK_CHIN_Y * S
    mid = (mx0 + mx1) // 2
    lamp_pool(g, 50, 22, 12, 8, ty + 1)
    # monitor: bezel, glass, text lines
    rect(g, mx0, 0, mx1, chin, BEZEL)
    rect(g, mx0, 0, mx1, 1, SLATE)
    rect(g, mx0 + 2, 2, mx1 - 2, chin - 2, GLASS)
    for i, (dx, ln) in enumerate(((2, 12), (4, 8), (2, 14), (4, 6))):
        rect(g, mx0 + 2 + dx, 4 + i * 2, mx0 + 2 + dx + ln, 5 + i * 2, GLASS_TXT)
    rect(g, mx0, chin - 1, mx1, chin, SHADOW)
    # stand
    rect(g, mid - 2, chin, mid + 2, chin + 2, SHADOW)
    rect(g, mid - 6, chin + 1, mid + 6, chin + 3, BEZEL)
    rect(g, mid - 6, chin + 1, mid + 6, chin + 2, SLATE)
    # keyboard: one band of keys, a mouse beside it
    ky = chin + 5
    rect(g, mx0 + 3, ky, mx1 - 3, ky + 3, KEY_DK)
    rect(g, mx0 + 4, ky + 1, mx1 - 4, ky + 2, KEYCAP)
    rect(g, mx1 - 1, ky, mx1 + 2, ky + 3, KEYCAP)
    # lamp, east wing
    rect(g, 47, 22, 53, 25, LAMP)
    rect(g, 47, 22, 53, 23, LAMP_HI)
    for t in range(8):
        put(g, 50 - t // 4, 21 - t, LAMP_HI)
    rect(g, 43, 10, 49, 13, LAMP)
    rect(g, 43, 10, 49, 11, LAMP_HI)
    rect(g, 44, 13, 48, 14, BULB)
    # papers and a mug, west wing
    rect(g, 3, 17, 11, 25, OFFWHITE_SH)
    rect(g, 2, 16, 10, 24, OFFWHITE)
    for i in range(3):
        rect(g, 3, 18 + i * 2, 8 - (i % 2) * 2, 19 + i * 2, PRINT)
    rect(g, 4, 27, 8, 31, MUG)
    rect(g, 4, 27, 8, 28, MUG_SH)
    put(g, 8, 28, MUG_SH)
    put(g, 8, 29, MUG_SH)
    outline(g, {BEZEL, SLATE, GLASS, GLASS_TXT}, SHADOW)
    return g


# ---- the sofas ---------------------------------------------------------------
def meeting_sofa():
    """The south-facing sofa: the backrest, arms, three seat cushions
    with their fronts, feet."""
    w, h = SOFA_W * S, SOFA_H * S
    seat = SOFA_SEAT_ROWS * S  # the backrest's foot, the seat's back
    g = canvas(w, h)
    # backrest
    rect(g, 3, 1, w - 3, seat, FABRIC)
    rect(g, 3, 1, w - 3, 4, FABRIC_HI)
    rect(g, 3, seat - 2, w - 3, seat, FABRIC_SH)
    # seat cushions
    for c in SOFA_SEAT_COLS:
        x0, x1 = c * S - 3 * S + 1, c * S + 3 * S - 1
        x0, x1 = max(x0, 6), min(x1, w - 6)
        rect(g, x0, seat, x1, h - 4, FABRIC)
        rect(g, x0 + 1, seat + 1, x1 - 1, seat + 3, FABRIC_HI)
        rect(g, x0, h - 6, x1, h - 4, FABRIC_SH)
    rect(g, 5, seat, w - 5, seat + 1, FABRIC_SEAM)
    # arms
    for ax in (0, w - 6):
        rect(g, ax, 3, ax + 6, h - 3, FABRIC)
        rect(g, ax, 3, ax + 6, 6, FABRIC_HI)
        rect(g, ax + (5 if ax == 0 else 0), 6, ax + (6 if ax == 0 else 1), h - 3, FABRIC_SEAM)
    # front rail, feet
    rect(g, 6, h - 4, w - 6, h - 3, FABRIC_SEAM)
    rect(g, 3, h - 2, 6, h, KEY_DK)
    rect(g, w - 6, h - 2, w - 3, h, KEY_DK)
    outline(g, {FABRIC, FABRIC_HI, FABRIC_SH, FABRIC_SEAM}, OUTLINE)
    return g


def meeting_sofa_north():
    """The north-facing sofa: three cushions beyond the backrest, its lit
    ridge, a back panel dithering into shade, arms, feet."""
    w, h = SOFA_W * S, SOFA_H * S
    seat, ridge_end = SOFA_SEAT_ROWS * S, (SOFA_SEAT_ROWS + SOFA_RIDGE_ROWS) * S
    panel_end = h - 2
    g = canvas(w, h)
    rect(g, 5, 2, w - 5, seat, FABRIC_SEAM)
    for c in SOFA_SEAT_COLS:
        x0, x1 = max(c * S - 3 * S + 1, 5), min(c * S + 3 * S - 1, w - 5)
        rect(g, x0, 2, x1, seat, FABRIC)
        rect(g, x0 + 1, 3, x1 - 1, 5, FABRIC_HI)
        rect(g, x0, seat - 2, x1, seat, FABRIC_SH)
    # ridge
    rect(g, 3, seat, w - 3, ridge_end, FABRIC_HI)
    rect(g, 3, seat, w - 3, seat + 1, FABRIC_SEAM)
    rect(g, 3, ridge_end - 1, w - 3, ridge_end, FABRIC)
    # back panel, dithering into shade in its lower half
    rect(g, 0, ridge_end, w, panel_end, FABRIC)
    shade = ridge_end + (panel_end - ridge_end) // 2
    for y in range(shade, panel_end):
        for x in range(w):
            if y >= panel_end - 2 or (x + y) % 2 == 0:
                g[y][x] = FABRIC_SH
    # arms
    for ax, inner in ((0, 5), (w - 6, w - 6)):
        rect(g, ax, 1, ax + 6, panel_end, FABRIC)
        rect(g, ax, 1, ax + 6, 4, FABRIC_HI)
        for y in range(shade, panel_end):
            for x in range(ax, ax + 6):
                if y >= panel_end - 2 or (x + y) % 2 == 0:
                    g[y][x] = FABRIC_SH
        rect(g, inner, 4, inner + 1, panel_end, FABRIC_SEAM)
    rect(g, 3, panel_end, 6, h, KEY_DK)
    rect(g, w - 6, panel_end, w - 3, h, KEY_DK)
    outline(g, {FABRIC, FABRIC_HI, FABRIC_SH, FABRIC_SEAM}, OUTLINE)
    return g


# ---- output -----------------------------------------------------------------
PROVENANCE = "Generated by scripts/gen-art.py: edit the generator, not this file."
ENCODING = "utf-8"  # the keys include σ/ψ/Θ, and the locale's encoding need not be the file's


def render_sprite(header, frames):
    text = header.strip("\n") + "\n" + PROVENANCE
    lines = [f"# {l}" if l else "#" for l in text.split("\n")]
    body = []
    for i, g in enumerate(frames):
        body.append(f"@frame {i}")
        body.extend(" ".join(r) for r in g)
    return "\n".join(lines + body) + "\n"


def orphans(pack, sprites):
    """The generated sprites on disk that `sprites` no longer draws."""
    return sorted(
        p.name
        for p in pack.glob("*.sprite")
        if p.name not in sprites and f"# {PROVENANCE}" in p.read_text(encoding=ENCODING).splitlines()
    )


def check(pack, sprites):
    """Why the committed pack is not what `sprites` draws, or None when it is."""
    stale = sorted(
        name
        for name, text in sprites.items()
        if not (pack / name).is_file() or (pack / name).read_text(encoding=ENCODING) != text
    )
    gone = orphans(pack, sprites)
    if not stale and not gone:
        return None
    lines = [f"gen-art --check: stale {stale}, orphaned {gone} — run `just gen-art`"]
    if stale and (pack / stale[0]).is_file():
        committed = (pack / stale[0]).read_text(encoding=ENCODING).splitlines()
        drawn = sprites[stale[0]].splitlines()
        diff = difflib.unified_diff(committed, drawn, f"committed/{stale[0]}", f"drawn/{stale[0]}", n=0, lineterm="")
        lines.extend(list(diff)[:20])
    return "\n".join(lines)


def write(pack, sprites):
    for name in orphans(pack, sprites):
        (pack / name).unlink()
        print("deleted", name)
    for name, text in sprites.items():
        (pack / name).write_text(text, encoding=ENCODING)
        print("wrote", name)


def selftest(sprites):
    """A drift gate that cannot fail is no gate."""
    with tempfile.TemporaryDirectory() as tmp:
        pack = pathlib.Path(tmp)
        write(pack, sprites)
        assert check(pack, sprites) is None, "a freshly drawn pack must pass"
        first = min(sprites)
        (pack / first).write_text(sprites[first] + ". .\n", encoding=ENCODING)
        assert check(pack, sprites) is not None, "an edited sprite must fail"
        (pack / first).unlink()
        assert check(pack, sprites) is not None, "a missing sprite must fail"
        write(pack, sprites)
        (pack / "gone@4x.sprite").write_text(f"# {PROVENANCE}\n", encoding=ENCODING)
        assert check(pack, sprites) is not None, "an orphan must fail"
        write(pack, sprites)
        assert check(pack, sprites) is None, "a write must delete the orphan"
        (pack / "hand.sprite").write_text(f"# copied from {first}: {PROVENANCE}\n@frame 0\n.\n", encoding=ENCODING)
        assert check(pack, sprites) is None, "hand art quoting the provenance is not an orphan"
    print(f"gen-art --selftest: OK ({len(sprites)} sprites)")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("pack", nargs="?", type=pathlib.Path, help="the pack directory to write or check")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="fail on drift; write nothing")
    mode.add_argument("--selftest", action="store_true", help="prove the check can fail")
    args = parser.parse_args()
    if not args.selftest and args.pack is None:
        parser.error("a pack directory is required")
    if args.pack is not None and not args.pack.is_dir():
        parser.error(f"not a directory: {args.pack}")
    pieces = {
        **{pose: (CHARACTER_HEADERS[pose], [dressed(pose, BAKED_STYLE)]) for pose in POSES},
        "desk_chair": ("The task chair from behind: a padded back, arm pads, a five-star base on\ncasters.", [desk_chair()]),
        "desk": ("The viewer-facing desk: the monitor turns its BACK to us (casing, vents,\nbadge), a lamp west, papers and a mug east.", [desk_south()]),
        "desk_north": ("The back-turned desk: the raised monitor's glass, a keyboard band, a\nlamp east, papers and a mug west.", [desk_north()]),
        "meeting_sofa": ("A three-seat sofa facing south: backrest, arms, three cushions, feet.", [meeting_sofa()]),
        "meeting_sofa_north": ("A three-seat sofa from behind: cushions beyond the backrest, its lit\nridge, a back panel dithering into shade.", [meeting_sofa_north()]),
    }
    classic = {
        "meeting_sofa_north": ("The sofa from behind: seat rows under a sitter, the backrest over one.", [meeting_sofa_north_1x()]),
        "desk": ("The viewer-facing desk: the monitor's back on its stand, the wood lit\nalong its back edge and front lip.", [desk_south_1x()]),
        "desk_north": ("The back-turned desk: its raised monitor outlined in grey, dim text on\nthe glass, a keyboard a row clear of the stand.", [desk_north_1x()]),
    }
    sprites = {
        f"{base}@{S}x.sprite": render_sprite(header, frames)
        for base, (header, frames) in pieces.items()
    }
    sprites |= {f"{base}.sprite": render_sprite(header, frames) for base, (header, frames) in classic.items()}
    if args.selftest:
        selftest(sprites)
    elif args.check:
        problem = check(args.pack, sprites)
        if problem:
            sys.exit(problem)
        print(f"gen-art --check: OK ({len(sprites)} sprites)")
    else:
        write(args.pack, sprites)


if __name__ == "__main__":
    main()
