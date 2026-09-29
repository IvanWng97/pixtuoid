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
def hair_clusters(g, clusters):
    """Big curl clusters, back to front: each a disc lit from the top-left, with
    a highlight nub, and a shaded rim on its lower-right edge so every curl
    stands apart from the one behind it."""
    for cx, cy, r in clusters:
        for y in range(int(cy - r) - 1, int(cy + r) + 2):
            for x in range(int(cx - r) - 1, int(cx + r) + 2):
                dx, dy = x + 0.5 - cx, y + 0.5 - cy
                d = math.hypot(dx, dy)
                if d > r:
                    continue
                t = (dx + dy) / r  # -1.4 top-left .. +1.4 bottom-right
                if d > r - 1.2 and t > -0.1:
                    k = HAIR_SH
                elif t < -0.4:
                    k = HAIR_LT
                else:
                    k = HAIR
                put(g, x, y, k)
        hx, hy = int(cx - r * 0.45), int(cy - r * 0.55)
        put(g, hx, hy, HAIR_HI)
        put(g, hx + 1, hy, HAIR_HI)


def seated_front():
    """The front-facing sitter: curls, a face of two eyes and a mouth
    hint, a lit shirt and two hands."""
    w, h = 8 * S, 10 * S
    g = canvas(w, h)
    # shirt: shoulders from row 23, body to the seat, lit on the west
    for y in range(23, 36):
        t = min(1.0, (y - 23) / 4)
        half = 8 + (14 - 8) * math.sin(t * math.pi / 2)
        x0, x1 = int(round(15.5 - half)), int(round(15.5 + half)) + 1
        for x in range(x0, x1):
            k = SHIRT
            if x < x0 + 3:
                k = SHIRT_LT
            elif x >= x1 - 3:
                k = SHIRT_SH
            g[y][x] = k
    # arms: sleeve shade lines down each side, cuffs at the hands
    for x in (4, 5):
        rect(g, x, 30, x + 1, 36, SHIRT_LT if x == 4 else SHIRT)
    rect(g, 7, 28, 8, 35, SHIRT_SH)
    rect(g, 24, 28, 25, 35, SHIRT_SH)
    # collar: a V of skin, edged by the shirt's shade
    for i in range(3):
        rect(g, 14 + i // 2, 23 + i, 18 - i // 2, 24 + i, SKIN)
        put(g, 13 + i // 2, 23 + i, SHIRT_SH)
        put(g, 18 - i // 2, 23 + i, SHIRT_SH)
    # neck
    rect(g, 13, 20, 19, 23, SKIN_SH)
    # hair first: big curl clusters over the crown and down the sides, so the
    # face drawn after sits framed inside them
    hair_clusters(g, [
        (9.5, 8.5, 5.4), (22.5, 8.5, 5.4), (16, 5.5, 6.4),
        (7.0, 14.0, 3.4), (25.0, 14.0, 3.4), (12.0, 3.2, 4.0), (20.0, 3.0, 4.2),
    ])
    # face: a rounded block from the brow to the chin, shaded on the east
    for y in range(9, 22):
        x0, x1 = (10, 22) if y in (9, 21) else (9, 23)
        for x in range(x0, x1):
            g[y][x] = SKIN_SH if x >= x1 - 3 or y == 21 else SKIN
    # the fringe: curls over the brow with two gaps of forehead
    rect(g, 9, 9, 23, 10, HAIR)
    for x in range(9, 23):
        if x not in (12, 13, 18, 19):
            put(g, x, 10, HAIR_SH if x % 3 else HAIR)
    # eyes (2x2, a lit pixel in each), blush, a mouth hint
    for ex in (11, 19):
        rect(g, ex, 13, ex + 2, 15, EYE)
    rect(g, 10, 16, 12, 17, BLUSH)
    rect(g, 20, 16, 22, 17, BLUSH)
    rect(g, 15, 18, 17, 19, MOUTH)
    # hands: skin mitts under the sleeves, lit on their tops
    for hx in (4, 24):
        rect(g, hx, 36, hx + 4, 40, SKIN)
        rect(g, hx, 39, hx + 4, 40, SKIN_SH)
    # outlines: hair by its own darkest ramp, skin by its dark, shirt by its own
    outline(g, {HAIR, HAIR_SH, HAIR_LT, HAIR_HI}, HAIR_OUT)
    outline(g, {SKIN, SKIN_SH, EYE, BLUSH, MOUTH}, SKIN_DK)
    outline(g, {SHIRT, SHIRT_SH, SHIRT_LT, WHITE, WHITE_SH}, SHIRT_OUT)
    despeckle(g, {HAIR_HI, HAIR_LT, SHIRT_LT, SHIRT_SH})
    return g


def seated_back():
    """The back-view sitter: the head all curls, the shirt's back lit on
    the west, hands just past the seat at each side."""
    w, h = 8 * S, 10 * S
    g = canvas(w, h)
    for y in range(22, 36):
        t = min(1.0, (y - 22) / 4)
        half = 8 + (14 - 8) * math.sin(t * math.pi / 2)
        x0, x1 = int(round(15.5 - half)), int(round(15.5 + half)) + 1
        for x in range(x0, x1):
            k = SHIRT
            if x < x0 + 3:
                k = SHIRT_LT
            elif x >= x1 - 3:
                k = SHIRT_SH
            g[y][x] = k
    # the back's centre fold and shoulder blades
    rect(g, 15, 27, 16, 35, SHIRT_SH)
    rect(g, 9, 26, 13, 27, SHIRT_SH)
    rect(g, 19, 26, 23, 27, SHIRT_SH)
    rect(g, 7, 28, 8, 35, SHIRT_SH)
    rect(g, 24, 28, 25, 35, SHIRT_SH)
    # neck, just showing under the curls
    rect(g, 13, 19, 19, 22, SKIN_SH)
    hair_clusters(g, [
        (9.5, 9.5, 5.4), (22.5, 9.5, 5.4), (16, 6.5, 6.5), (16, 13.5, 6.0),
        (10.5, 16.0, 4.6), (21.5, 16.0, 4.6), (12.5, 4.0, 4.2), (19.5, 3.8, 4.4),
        (16, 18.0, 3.6),
    ])
    for hx in (4, 24):
        rect(g, hx, 36, hx + 4, 40, SKIN)
        rect(g, hx, 39, hx + 4, 40, SKIN_SH)
    outline(g, {HAIR, HAIR_SH, HAIR_LT, HAIR_HI}, HAIR_OUT)
    outline(g, {SKIN, SKIN_SH}, SKIN_DK)
    outline(g, {SHIRT, SHIRT_SH, SHIRT_LT}, SHIRT_OUT)
    despeckle(g, {HAIR_HI, HAIR_LT, SHIRT_LT, SHIRT_SH})
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
        "seated": ("Front view at rest: curl clusters framing a face of two eyes and a\nmouth, a lit shirt, both hands level.", [seated_front()]),
        "seated_back": ("Back view at rest: the head all curls, the shirt's back lit on the\nwest.", [seated_back()]),
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
