#!/usr/bin/env python3
"""Author the bundled pack's generated sprite art: the cutaway profile's `@Nx`
pieces (N = `S`), and the 1x pieces drawn from the same layout: the classic
profile's desks, the cutaway's base-density back-view sofa, and the city's
building bases.

Draws in PALETTE-KEY space, so the output is .sprite text and the recolor keys
(H/B/S/P and their [ramps] shades) survive per-agent recoloring. The .sprite
files it writes are committed; edit this, not them, and rerun:

    just gen-art

`--check` writes nothing and fails if a sprite it draws is missing or differs
from the committed one, or if a sprite carrying the provenance line is no longer
drawn here (a write deletes those).

It resolves no colour itself: the engine owns the palette and its ramps, so look
at the result through the real renderer: `cargo run --release --example
cutaway_snapshot -- <out.png>` for the `@Nx` art (`--scale 1` for the base-density
sofa), `cargo run --release --example snapshot -- --crop-furniture desk <out.png>`
for the 1x desk.
"""

import argparse
import difflib
import inspect
import math
import pathlib
import random
import sys
import tempfile
import tomllib

S = 4  # the cutaway art's density

# ---- palette keys ---------------------------------------------------------
# recolor bases and their shades (bases in [palette], shades in [ramps])
HAIR, HAIR_SH, HAIR_LT, HAIR_HI = "H", "h", "Y", "Z"
SKIN, SKIN_SH, SKIN_DK = "S", "s", "i"
SHIRT, SHIRT_SH, SHIRT_LT, SHIRT_OUT = "B", "v", "W", "U"
PANTS, PANTS_SH, PANTS_LT = "P", "p", "("
# fixed colours, named by role
OUTLINE, WHITE = "n", "&"
EYE = "e"
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
# The @Nx art's one outline, round each piece's whole silhouette whatever its
# material, and a bun's rim where it sits on the head (`ball`); pack.toml's
# `[characters] outline` names it for the renderer.
SILHOUETTE = "κ"
COFFEE = "ρ"
# furniture materials
LEAF, LEAF_SH, LEAF_HI, LEAF_DK = "l", "L", "Φ", "Ψ"
POT, POT_SH, POT_HI = "g", "Γ", "δ"
ALU, ALU_DK = "Π", "θ"
FABRIC, FABRIC_HI, FABRIC_SH, FABRIC_SEAM = "C", "G", "Λ", "Ω"
# fixed hues, named by hue: the 1x art's own keys, lent to the furniture
INK, CYAN, RED, DARK_RED, VERMILION, BLUE, GOLD, TAN, BROWN, PINK, GREEN = (
    "q", "c", "r", "N", "o", "b", "y", "x", "z", "^", "λ")
PETAL_R, PETAL_Y, PETAL_HI = RED, GOLD, "ζ"
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
# density, so the `@Nx` art cannot drift from the 1x it redraws.
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
# The back-turned desk's extra rows, all above, so the occupant (who y-sorts in
# FRONT of the desk) leaves the upper screen row clear. Its lower screen row
# sits inside the wood, flanked by it: the owner picked a monitor standing on
# its desk over one floating clear of every head.
DESK_NORTH_LIFT = 2
DESK_LEG_W = 2
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


# ---- the meeting sofas: one layout for both views and densities, in logical rows ----
# `Furniture::MeetingSofaBody`'s visual box
# (`every_hover_size_is_its_painted_sprite_size`).
SOFA_W, SOFA_H = 20, 7
# The back view's seat rows, under its sitter; the backrest below draws over
# their lap. `cutaway::paint`'s `NORTH_SOFA_SEAT_ROWS`
# (`the_north_sofas_backrest_starts_on_its_lit_ridge`).
SOFA_SEAT_ROWS = 3
SOFA_RIDGE_ROWS = 1  # the backrest's lit top, then its back panel to the foot
# The front view's backrest rows, above its seam: `meeting_sofa.sprite`'s.
SOFA_BACK_ROWS = 3
# The three seats' centres (the seat waypoints' `SEAT_DX` about the sofa's
# centre column), on column boundaries, and half the pitch between them: where
# one cushion ends and the next begins.
SOFA_SEAT_COLS = (4, 10, 16)
SOFA_SEAT_HALF = 3


def meeting_sofa_north_1x():
    """The north-facing sofa at base density: lit cushion tops, a seam where the
    seat meets the backrest, its lit ridge, and the back panel shading down."""
    ridge = SOFA_SEAT_ROWS
    g = canvas(SOFA_W, SOFA_H)
    rect(g, 0, 1, SOFA_W, SOFA_H - 1, FABRIC)
    rect(g, 2, 1, SOFA_W - 1, 2, FABRIC_HI)
    for c in SOFA_SEAT_COLS[1:]:  # a seam midway between each two seat centres
        put(g, c - SOFA_SEAT_HALF, 1, OUTLINE)
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



# ---- the characters ----------------------------------------------------------
# A chibi figure: a big head over a small body, one near-black line round the
# whole silhouette. Every pose is a bald BODY, the face or the back of the head
# but never a scalp, that a hairstyle's layers dress per view: `behind` under
# the body, `over` on top. Neither is outlined here: the renderer outlines the
# dressed union, so no line runs between hair and face.
FIG_W = 8 * S
FIG_CX = FIG_W / 2  # sampled at pixel centres (x + 0.5): x mirrors to FIG_W - 1 - x
# Each pose's height in logical rows, the frame height its 1x base shares.
STANDING_ROWS, TYPING_ROWS, SEATED_ROWS, COUCH_ROWS = 12, 11, 10, 9
# Rows every standing and front- or back-view seated pose shares: the face
# (the eyes and mouth hang off it) and the shoulders.
FACE_TOP, FACE_BOTTOM, SHOULDER_Y = 12, 26, 26
# A seated shirt ends on the seat, where the base's hand row starts; a
# standing one on the belt.
SEAT_Y, BELT_Y = 36, 38
# Where a sleeve ends and a hanging hand begins.
CUFF_Y = 34
SKIN_KEYS = {SKIN, SKIN_SH}


def union_outline(g, k=SILHOUETTE):
    """One line round everything drawn, whatever its material."""
    outline(g, {c for row in g for c in row} - {T, k}, k)


def ellipse(g, cx, cy, rx, ry, k=HAIR):
    for y in range(max(0, int(cy - ry) - 1), min(len(g), int(cy + ry) + 2)):
        for x in range(1, len(g[0]) - 1):
            if ((x + 0.5 - cx) / rx) ** 2 + ((y + 0.5 - cy) / ry) ** 2 <= 1:
                g[y][x] = k


def disc(g, cx, cy, r, k=HAIR):
    ellipse(g, cx, cy, r, r, k)


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
    return g


def terminator(g, cx, cy, r, from_y=0):
    """A head turning from the light: hair past a circle offset up and west,
    toward the light, drops a shade, so the mop reads as a ball."""
    lx, ly = cx - 3.0, cy - 3.5
    for y in range(from_y, len(g)):
        for x in range(len(g[0])):
            if g[y][x] == HAIR and math.hypot(x + 0.5 - lx, y + 0.5 - ly) > r:
                g[y][x] = HAIR_SH


def comb(g, lines, dy=0):
    """Comb lines in hair shade."""
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


def paste(dst, src, dx=0, dy=0):
    """`src`'s opaque pixels onto `dst`, moved `dx` columns and `dy` rows."""
    for y in range(len(src)):
        for x in range(len(src[0])):
            if src[y][x] != T and 0 <= y + dy < len(dst) and 0 <= x + dx < len(dst[0]):
                dst[y + dy][x + dx] = src[y][x]


# ---- hairstyles ----------------------------------------------------------------
# Each style draws four views on its own layer canvas, the tallest pose's rows
# plus HAIR_HEADROOM above for a bun or a tuft: `front` and `side` as
# (behind, over) pairs, `back` and `crown` as one layer over the body. Every
# coordinate below is on the pose grid; `o` shifts it onto the layer.
HAIR_HEADROOM = 6
LAYER_H = STANDING_ROWS * S + HAIR_HEADROOM
o = HAIR_HEADROOM
# The skull in profile, and the head seen from above as it lies on the arms.
SIDE_CX, SIDE_CY = 14.0, 12.5
CROWN_CX, CROWN_CY, CROWN_R = FIG_CX, 16.5, 10.0


def layer():
    return canvas(FIG_W, LAYER_H)


def front_fringe(tips):
    f = layer()
    fringe(f, tips, FACE_TOP - 4, o)
    return rim_light(f, lit_top=False)


def front_locks(locks):
    f = layer()
    fringe_locks(f, locks, FACE_TOP - 4, o)
    return rim_light(f, lit_top=False)


def side_locks(locks, top=8):
    f = layer()
    fringe_locks(f, locks, top, o)
    return rim_light(f, lit_top=False)


def nape_cut(g, half=6, rows=(20, 23)):
    """Hair gathered up leaves the nape bare: clear its middle so the neck shows."""
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
    """A round cloud of a mop."""
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
    """The mop with short tufts breaking its top and sides."""
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
    chest in pointed ends."""
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
    # The fall narrows below the head so the shoulders show either side of it:
    # the shirt is how a viewer tells one long-haired back from another.
    for y in range(12 + o, 37 + o):
        spread = (y - 12 - o) / 25
        narrow = min(1.0, (y - 12 - o) / 10) * 4
        rect(back, int(4 + narrow - spread), y, int(28 - narrow + spread), y + 1, HAIR)
    for x0 in (9, 13, 17, 21, 25):
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
    lock(sb, 8.5, 12 + o, 6.0, 38 + o, 11, 6)  # full enough to meet the neck
    rim_light(sb, [(9, 18 + o), (8, 24 + o), (7, 30 + o), (11, 7 + o)], [(14, 2 + o), (15, 2 + o)])
    so = side_locks([(19.5, 13, 2.2), (22.5, 12.5, 2.2), (24.5, 12, 1.6)])
    strand = layer()
    lock(strand, 14.5, 14 + o, 15.0, 24 + o, 3, 2)  # its west edge on the face's
    paste(so, rim_light(strand))
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
    """Pulled back tight into a bun on the crown: combed lines run up to it, the ears show, the bun rises into the headroom."""
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
    showing."""
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
    """Tight curls in a close cap: rows of small rounds, each lit on its own."""
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
    """A shaggy mop whose fringe falls in locks to the eyes."""
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
# Drawn on the pose grid, shifted down `dy` rows onto a taller canvas (a hair
# layer's).
def mitt(g, x, y, wrist_west=False):
    """A hand: a skin block, shaded along its foot, rounded at the bottom but
    for a corner its wrist joins: rounded there, it leaves a notch against the
    sleeve that the outline closes over a pinhole."""
    rect(g, x, y, x + S, y + S, SKIN)
    rect(g, x, y + S - 1, x + S, y + S, SKIN_SH)
    if not wrist_west:
        g[y + S - 1][x] = T
    g[y + S - 1][x + S - 1] = T


def ears_front(g, dy):
    for x, inner in ((5, 6), (25, 25)):
        rect(g, x, FACE_TOP + 5 + dy, x + 2, FACE_TOP + 9 + dy, SKIN)
        rect(g, inner, FACE_TOP + 6 + dy, inner + 1, FACE_TOP + 8 + dy, SKIN_SH)


def ears_back(g, dy):
    for x in (5, 25):
        rect(g, x, 17 + dy, x + 2, 21 + dy, SKIN)
        rect(g, x + (1 if x < 16 else 0), 18 + dy, x + (2 if x < 16 else 1), 20 + dy, SKIN_SH)


def ear_side(g, dy):
    rect(g, 12, 16 + dy, 15, 20 + dy, SKIN)
    rect(g, 13, 17 + dy, 14, 19 + dy, SKIN_SH)


def face_front(g, dy):
    """The face: round, shaded east and along the chin, two tall eyes and a
    small mouth."""
    for y in range(FACE_TOP - 2, FACE_BOTTOM):
        for x in range(7, 25):
            nx = (x + 0.5 - FIG_CX) / 9.0
            ny = max(0.0, (y + 0.5 - (FACE_TOP + 6)) / (FACE_BOTTOM - FACE_TOP - 6))
            if nx * nx + ny * ny <= 1.0:
                g[y + dy][x] = SKIN_SH if (x >= 21 or y >= FACE_BOTTOM - 2) else SKIN
    eye_y = FACE_TOP + 5 + dy
    rect(g, 11, eye_y, 13, eye_y + 4, EYE)
    rect(g, 19, eye_y, 21, eye_y + 4, EYE)
    rect(g, 15, FACE_BOTTOM - 4 + dy, 17, FACE_BOTTOM - 3 + dy, SKIN_DK)


def nape(g, dy):
    """The back of the head under the hair: its round and the neck."""
    ellipse(g, FIG_CX, 17.0 + dy, 9.0, 8.6, SKIN_SH)
    rect(g, 12, 23 + dy, 20, 27 + dy, SKIN_DK)
    rect(g, 13, 23 + dy, 19, 26 + dy, SKIN_SH)


def side_face(g, dy):
    """The profile, facing east: brow, a nose bump, the mouth, a chin rounding
    back to the jaw, one eye."""
    rows = {10: (15, 24), 11: (14, 25), 12: (13, 25), 13: (13, 25), 14: (13, 25), 15: (13, 26),
            16: (13, 27), 17: (13, 27), 18: (13, 26), 19: (13, 25), 20: (13, 25), 21: (13, 25),
            22: (14, 25), 23: (15, 24), 24: (16, 23), 25: (17, 21)}
    for y, (x0, x1) in rows.items():
        for x in range(x0, x1):
            g[y + dy][x] = SKIN_SH if (y >= 24 or x < 15) else SKIN
    rect(g, 21, 15 + dy, 23, 19 + dy, EYE)
    rect(g, 23, 22 + dy, 25, 23 + dy, SKIN_DK)
    rect(g, 13, 24 + dy, 19, 28 + dy, SKIN_DK)  # the neck


def neck_shadow(g, dy):
    """The neck's shadow above the collar, where the shoulders begin."""
    rect(g, 12, SHOULDER_Y + dy, 20, SHOULDER_Y + 1 + dy, SKIN_DK)


def shirt(g, dy, bottom, back=False, sleeves=True):
    """The shirt from the shoulders to `bottom`, lit on the west, with sleeves
    hanging at the sides; the neck's shadow above the collar, and the collar's
    V in front or a fold at the foot of the back."""
    rect(g, 9, SHOULDER_Y + dy, 23, SHOULDER_Y + 1 + dy, SHIRT)
    rect(g, 6, SHOULDER_Y + 1 + dy, 26, bottom + dy, SHIRT)
    rect(g, 20, SHOULDER_Y + 1 + dy, 26, bottom + dy, SHIRT_SH)
    rect(g, 6, SHOULDER_Y + 1 + dy, 7, bottom + dy, SHIRT_LT)
    if sleeves:
        rect(g, 5, SHOULDER_Y + 2 + dy, 27, CUFF_Y + dy, SHIRT)
        rect(g, 22, SHOULDER_Y + 2 + dy, 27, CUFF_Y + dy, SHIRT_SH)
        rect(g, 5, SHOULDER_Y + 2 + dy, 6, CUFF_Y + dy, SHIRT_LT)
        rect(g, 8, SHOULDER_Y + 3 + dy, 9, bottom + dy, SHIRT_SH)
        rect(g, 23, SHOULDER_Y + 3 + dy, 24, bottom + dy, SHIRT_OUT)
    neck_shadow(g, dy)
    if back:
        rect(g, 15, bottom - 5 + dy, 17, bottom + dy, SHIRT_SH)
    else:
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


def elbows_typing(frame):
    """Typing seen from behind, where the chair's arm pads hide most of the
    hands' press: the reaching arm's elbow juts out past the chair, swapping
    sides each frame as the hands do."""
    hands = hands_typing(frame)

    def draw(g, dy):
        hands(g, dy)
        west = frame == 0
        x0 = 2 if west else FIG_W - 5
        rect(g, x0, SHOULDER_Y + 3 + dy, x0 + 3, CUFF_Y + dy, SHIRT if west else SHIRT_SH)
        if west:
            rect(g, x0, SHOULDER_Y + 3 + dy, x0 + 1, CUFF_Y + dy, SHIRT_LT)
    return draw


def legs(g, dy, stride, view):
    """Hips, two legs and shoes: the stepping foot reaches the canvas foot a
    little outward, the other lifts its heel."""
    rect(g, 8, BELT_Y + dy, 24, BELT_Y + 2 + dy, PANTS)
    for side, x0 in ((-1, 8), (1, 17)):
        out = stride == side
        foot = STANDING_ROWS * S - (2 if stride != 0 and not out else 0)
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
        rect(g, x0 + (1 if side < 0 else 0), SHOULDER_Y + 2 + dy, x0 + (4 if side < 0 else 3), CUFF_Y + drop + dy,
             SHIRT if side < 0 else SHIRT_SH)
        if side < 0:
            rect(g, x0 + 1, SHOULDER_Y + 2 + dy, x0 + 2, CUFF_Y + drop + dy, SHIRT_LT)
        mitt(g, x0, CUFF_Y + drop + dy)


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
    rect(g, 5, SHOULDER_Y + 2 + dy, 8, CUFF_Y + drop + dy, SHIRT)
    rect(g, 5, SHOULDER_Y + 2 + dy, 6, CUFF_Y + drop + dy, SHIRT_LT)
    mitt(g, 4, CUFF_Y + drop + dy)
    rect(g, 24, SHOULDER_Y + 2 + dy, 27, 32 + dy, SHIRT_SH)
    rect(g, 21, 29 + dy, 26, 33 + dy, SHIRT_SH)
    mug(g, 16, 27 + dy)
    mitt(g, 21, 29 + dy)


def side_body(g, dy):
    """Seated in profile: a narrow torso, the near arm reaching forward, the lap
    to the knee."""
    rect(g, 9, 27 + dy, 22, SEAT_Y + dy, SHIRT)
    rect(g, 13, 27 + dy, 19, 28 + dy, SKIN_DK)  # the neck above the collar
    rect(g, 9, 27 + dy, 11, SEAT_Y + dy, SHIRT_LT)
    rect(g, 19, 28 + dy, 22, SEAT_Y + dy, SHIRT_SH)
    rect(g, 12, 28 + dy, 17, 32 + dy, SHIRT_SH)
    rect(g, 16, 30 + dy, 24, 34 + dy, SHIRT)
    rect(g, 16, 33 + dy, 24, 34 + dy, SHIRT_SH)
    mitt(g, 24, 30 + dy, wrist_west=True)
    rect(g, 9, SEAT_Y + dy, 28, SEATED_ROWS * S + dy, PANTS)
    rect(g, 9, SEAT_Y + dy, 28, SEAT_Y + 1 + dy, PANTS_LT)
    rect(g, 25, SEAT_Y + 1 + dy, 28, SEATED_ROWS * S + dy, PANTS_SH)


def asleep_body(g, dy, lean):
    """Face down on folded arms: shoulders rising behind the head, forearms
    crossed under it on the desk, hands tucked at the elbows, the torso behind
    the arms, the thighs at the chair's edge."""
    rect(g, 4 + lean, 18 + dy, 28 + lean, 26 + dy, SHIRT)
    rect(g, 22 + lean, 18 + dy, 28 + lean, 26 + dy, SHIRT_SH)
    rect(g, 4 + lean, 18 + dy, 6 + lean, 26 + dy, SHIRT_LT)
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
def seated_front(hands):
    return lambda g, dy: (shirt(g, dy, SEAT_Y), hands(g, dy))


def seated_rear(hands):
    return lambda g, dy: (shirt(g, dy, SEAT_Y, back=True), hands(g, dy))


def standing_body(stride, view, arms=arms_hanging):
    return lambda g, dy: (legs(g, dy, stride, view), shirt(g, dy, BELT_Y, back=view == "back", sleeves=False),
                          arms(g, dy, stride))


def couch_back(g, dy):
    rect(g, 9, SHOULDER_Y + dy, 23, SHOULDER_Y + 1 + dy, SHIRT)
    rect(g, 5, SHOULDER_Y + 1 + dy, 27, COUCH_ROWS * S + dy, SHIRT)
    rect(g, 21, SHOULDER_Y + 1 + dy, 27, COUCH_ROWS * S + dy, SHIRT_SH)
    rect(g, 5, SHOULDER_Y + 1 + dy, 7, COUCH_ROWS * S + dy, SHIRT_LT)
    neck_shadow(g, dy)


# name: (height in logical rows, view, body, the sprite's header).
POSES = {
    "seated": (SEATED_ROWS, "front", seated_front(hands_level),
        "Front view at rest, both hands level."),
    "typing_0": (TYPING_ROWS, "front", seated_front(hands_typing(0)),
        "Front view, typing, frame 0: the west hand level, the east pressing."),
    "typing_1": (TYPING_ROWS, "front", seated_front(hands_typing(1)),
        "Front view, typing, frame 1: the hands swap."),
    "seated_back": (SEATED_ROWS, "back", seated_rear(hands_level),
        "Back view at rest, both hands level."),
    "typing_back_0": (TYPING_ROWS, "back", seated_rear(elbows_typing(0)),
        "Back view, typing, frame 0."),
    "typing_back_1": (TYPING_ROWS, "back", seated_rear(elbows_typing(1)),
        "Back view, typing, frame 1."),
    "back_couch": (COUCH_ROWS, "back", couch_back,
        "On the couch, facing the windows: shoulders square over the seat back."),
    "standing": (STANDING_ROWS, "front", standing_body(0, "front"),
        "Standing: arms at the sides."),
    "walking_0": (STANDING_ROWS, "front", standing_body(-1, "front"),
        "Walking, frame 0: the west foot out."),
    "walking_1": (STANDING_ROWS, "front", standing_body(1, "front"),
        "Walking, frame 1: the east foot out."),
    "walking_back_0": (STANDING_ROWS, "back", standing_body(-1, "back"),
        "Walking away, frame 0."),
    "walking_back_1": (STANDING_ROWS, "back", standing_body(1, "back"),
        "Walking away, frame 1."),
    "holding_coffee": (STANDING_ROWS, "front", standing_body(0, "front", arms_mug_both),
        "Standing with a steaming mug in both hands."),
    "walking_coffee_0": (STANDING_ROWS, "front", standing_body(-1, "front", arms_mug_east),
        "Walking with a mug, frame 0."),
    "walking_coffee_1": (STANDING_ROWS, "front", standing_body(1, "front", arms_mug_east),
        "Walking with a mug, frame 1."),
    "side_seated": (SEATED_ROWS, "side", lambda g, dy: side_body(g, dy),
        "Seated in profile, facing east, one arm reaching forward."),
    "seated_sleeping": (SEATED_ROWS, "crown", lambda g, dy: asleep_body(g, dy, 0),
        "Asleep face-down on folded arms."),
    "seated_sleeping_alt": (SEATED_ROWS, "crown", lambda g, dy: asleep_body(g, dy, 3),
        "Dozed off, slumped east."),
}
# Where a face-down head lies, as an offset from the crown's rest.
CROWN_SHIFT = {"seated_sleeping": (0, 0), "seated_sleeping_alt": (1, 1)}


# Where every view's hair is laid: this point on a body, and the same point on
# a layer, whose canvas stands HAIR_HEADROOM rows taller.
HEAD_MARK = (16, FACE_TOP)
EARS = {"front": ears_front, "back": ears_back, "side": ear_side}


def bald_head(view, g, dy):
    """The head a view's body shows under the hair: the face, the back of the
    head, or the profile. The ears are the hairstyle's to show or cover."""
    if view == "front":
        face_front(g, dy)
    elif view == "back":
        nape(g, dy)
    elif view == "side":
        side_face(g, dy)


def body_frame(pose):
    """`pose`'s bald body, unoutlined (the renderer outlines the dressed union),
    and its head mark `(view, x, y)`."""
    rows, view, body, _ = POSES[pose]
    g = canvas(FIG_W, rows * S)
    # The head, then the body in front of it: a collar over the neck, a raised
    # mug and its steam over the chin.
    bald_head(view, g, 0)
    body(g, 0)
    dx, dy = CROWN_SHIFT.get(pose, (0, 0))
    return g, (view, HEAD_MARK[0] + dx, HEAD_MARK[1] + dy)


def finished(lyr, skin, cover):
    """`lyr` shaded where it meets bare skin in the dressed frame: `skin` holds
    the view's bald head, `cover` the layer that will lie over this one. Skin
    is bare where nothing lies over it: for the over layer, where the layer
    itself does not; for the behind layer, which the body lies over, where the
    over layer does not."""
    out = [row[:] for row in lyr]
    top = lyr if cover is None else cover
    for y in range(LAYER_H):
        for x in range(1, FIG_W - 1):
            if lyr[y][x] not in (HAIR, HAIR_LT):
                continue
            for dx, dy in ((1, 0), (-1, 0), (0, 1)):
                nx, ny = x + dx, y + dy
                if ny < LAYER_H and skin[ny][nx] in SKIN_KEYS and top[ny][nx] == T:
                    out[y][x] = HAIR_SH
                    break
    return out


def hair_layers(style):
    """`style`'s layers per view as the renderer lays them: `(behind, over)`,
    each shaded where it meets bare skin, and the ears where the style leaves
    them bare: drawn over the behind layer, or in profile, where the ear lies on
    the face, under the over layer's hair."""
    look = HAIRSTYLES[style]()
    views = {}
    for view in ("front", "back", "side", "crown"):
        behind, over = look[view] if view in ("front", "side") else (None, look[view])
        if look["ears"] and view in EARS:
            ears = layer()
            EARS[view](ears, o)
            if view == "side":
                paste(ears, over)
                over = ears
            else:
                if behind is not None:
                    paste(ears, behind)
                    EARS[view](ears, o)
                behind = ears
        skin = layer()
        if view == "crown":
            asleep_body(skin, o, 0)
        else:
            bald_head(view, skin, o)
        views[view] = (
            finished(behind, skin, over) if behind is not None else None,
            finished(over, skin, None),
        )
    return views


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
# The @Nx desk's own detail, in art pixels: the 1x desk has none of it.
#
# The top's grain: a seam between boards this many rows deep, and a few short
# streaks inside the boards, sparse enough to read as wood, not stripes.
DESK_BOARD_ROWS = 12
DESK_STREAKS = 3
# The drawer pedestal under the east end, and the one slim leg west of the open
# floor: a leg as wide as the 1x desk's would read as a second pedestal.
DESK_PEDESTAL_W = 16
DESK_WEST_LEG_W = 4


def desk_wood(lift, seed):
    """The desk's wood, inset a column each side so its outline fits the
    canvas: a top lit along its far edge with one board seam and a few grain
    streaks, its near edge catching the light over a shaded apron, a
    two-drawer pedestal east and one leg west, open floor between."""
    top, lip, legs, rows = desk_rows(lift)
    w, h = DESK_ART_W * S, rows * S
    ty, ly, gy = top * S, lip * S, legs * S
    x0, x1 = 1, w - 1
    g = canvas(w, h)
    rng = random.Random(seed)
    rect(g, x0, ty, x1, ly, WOOD)
    rect(g, x0, ty, x1, ty + 1, WOOD_LT)
    for y in range(ty + DESK_BOARD_ROWS, ly - 2, DESK_BOARD_ROWS):
        rect(g, x0, y, x1, y + 1, WOOD_SH)
    for _ in range(DESK_STREAKS):
        y = rng.randrange(ty + 3, ly - 3)
        if (y - ty) % DESK_BOARD_ROWS == 0:
            continue
        sx = rng.randrange(4, w - 12)
        rect(g, sx, y, sx + rng.randrange(5, 9), y + 1, WOOD_SH)
    rect(g, x0, ly, x1, gy, WOOD_SH)  # the apron
    rect(g, x0, ly, x1, ly + 1, WOOD_HI)  # the top's near edge
    ped = w - DESK_PEDESTAL_W
    rect(g, ped, ly + 1, x1, h - 1, WOOD)
    rect(g, x1 - 2, ly + 1, x1, h - 1, WOOD_SH)
    rect(g, ped, ly + 1, ped + 1, h - 1, WOOD_DK)
    mid = (ly + 1 + h - 1) // 2
    rect(g, ped + 1, mid, x1, mid + 1, WOOD_DK)  # between the drawers
    for y0 in (ly + 1, mid + 1):  # a pull on each drawer
        rect(g, ped + 5, y0 + 2, ped + 9, y0 + 3, GREY)
    rect(g, x0, gy, x0 + DESK_WEST_LEG_W, h - 1, WOOD_DK)
    rect(g, x0, gy, x0 + 1, h - 1, WOOD_SH)
    return g, (w, h, ty, ly, gy)


def lamp_pool(g, cx, cy, rx, ry, y_min):
    """The lamp's warm pool on the wood: a bright core ringed by lit wood."""
    for y in range(max(y_min, int(cy - ry)), int(cy + ry) + 1):
        for x in range(int(cx - rx), int(cx + rx) + 1):
            if 0 <= y < len(g) and 0 <= x < len(g[0]) and g[y][x] == WOOD:
                d = math.hypot((x - cx) / rx, (y - cy) / ry)
                if d < 0.55:
                    g[y][x] = POOL
                elif d < 1.0:
                    g[y][x] = WOOD_LT


def desk_lamp(g, bx, by, facing):
    """A black articulated lamp standing at (bx, by): a weighted base, a jointed
    arm, a shade tipped toward `facing` (-1 west, 1 east) with its bulb glowing
    under it."""
    rect(g, bx - 3, by, bx + 3, by + 2, LAMP)
    rect(g, bx - 3, by, bx + 3, by + 1, LAMP_HI)
    for t in range(6):  # the arm rising, then the elbow
        put(g, bx + (t // 3) * -facing, by - 1 - t, LAMP_HI)
    ex, ey = bx - 2 * facing, by - 7
    for t in range(4):
        put(g, ex + t * facing, ey - t // 2, LAMP_HI)
    sx = ex + 4 * facing
    rect(g, sx - 3, ey - 4, sx + 3, ey - 1, LAMP)
    rect(g, sx - 2, ey - 5, sx + 2, ey - 4, LAMP)
    rect(g, sx - 3, ey - 4, sx + 3, ey - 3, LAMP_HI)
    rect(g, sx - 2, ey - 1, sx + 2, ey, BULB)
    return sx, ey


def paper_stack(g, x, y):
    rect(g, x + 1, y + 1, x + 9, y + 9, OFFWHITE_SH)
    rect(g, x, y, x + 8, y + 8, OFFWHITE)
    for i in range(3):
        rect(g, x + 1, y + 2 + i * 2, x + 6 - (i % 2) * 2, y + 3 + i * 2, PRINT)


def desk_mug(g, x, y):
    rect(g, x, y, x + 4, y + 4, MUG)
    rect(g, x + 3, y, x + 4, y + 4, MUG_SH)
    rect(g, x + 1, y, x + 3, y + 1, COFFEE)
    put(g, x + 4, y + 1, MUG_SH)
    put(g, x + 4, y + 2, MUG_SH)


def monitor_stand(g, mid, chin):
    rect(g, mid - 2, chin, mid + 2, chin + 2, SHADOW)
    rect(g, mid - 6, chin + 2, mid + 6, chin + 4, BEZEL)
    rect(g, mid - 6, chin + 2, mid + 6, chin + 3, SLATE)


def desk_south():
    """The viewer-facing desk: the monitor turns its back to us (casing, vents,
    a badge) on its stand, a lamp west pooling light on the wood, papers and a
    mug east."""
    g, (w, h, ty, ly, gy) = desk_wood(0, 5)
    mx0, mx1, chin = DESK_MONITOR_X0 * S, DESK_MONITOR_X1 * S, DESK_CHIN_Y * S
    mid = (mx0 + mx1) // 2
    lamp_pool(g, 9, ty + 9, 11, 6, ty + 1)
    rect(g, mx0, 0, mx1, chin - 1, BEZEL)
    rect(g, mx0, 0, mx1, 1, SLATE)
    rect(g, mx0, 0, mx0 + 1, chin - 1, SLATE)
    rect(g, mx0 + 1, chin - 2, mx1, chin - 1, SHADOW)
    for vy in (4, 6):
        rect(g, mx0 + 8, vy, mx1 - 8, vy + 1, SHADOW)
    rect(g, mid - 2, 9, mid + 2, 11, SLATE)
    monitor_stand(g, mid, chin - 1)
    desk_lamp(g, 5, ty + 10, 1)
    paper_stack(g, 44, ty + 3)
    desk_mug(g, 46, ty + 14)
    union_outline(g)
    return g


def desk_north():
    """The back-turned desk: its raised monitor's glass with a few lines of
    code, a keyboard and mouse before it, a lamp east, papers and a mug west."""
    g, (w, h, ty, ly, gy) = desk_wood(DESK_NORTH_LIFT, 3)
    mx0, mx1, chin = DESK_MONITOR_X0 * S, DESK_MONITOR_X1 * S, DESK_CHIN_Y * S
    mid = (mx0 + mx1) // 2
    lamp_pool(g, 47, ty + 11, 11, 6, ty + 1)
    rect(g, mx0, 0, mx1, chin, BEZEL)
    rect(g, mx0, 0, mx1, 1, SLATE)
    rect(g, mx0 + 2, 2, mx1 - 2, chin - 2, GLASS)
    for i, (dx, ln) in enumerate(((1, 12), (3, 8), (3, 14), (1, 6), (3, 10))):
        rect(g, mx0 + 3 + dx, 3 + i * 2, mx0 + 3 + dx + ln, 4 + i * 2, GLASS_TXT)
    rect(g, mx0, chin - 1, mx1, chin, SHADOW)
    monitor_stand(g, mid, chin)
    ky = chin + 6
    rect(g, mx0 + 3, ky, mx1 - 5, ky + 3, KEY_DK)
    rect(g, mx0 + 4, ky + 1, mx1 - 6, ky + 2, KEYCAP)
    rect(g, mx1 - 2, ky, mx1 + 1, ky + 3, KEYCAP)
    desk_lamp(g, 49, ty + 12, -1)
    paper_stack(g, 3, ty + 4)
    desk_mug(g, 4, ty + 15)
    union_outline(g)
    return g


# ---- the sofas ---------------------------------------------------------------
def meeting_sofa():
    """The south-facing sofa: the backrest, arms, three seat cushions
    with their fronts, feet."""
    w, h = SOFA_W * S, SOFA_H * S
    seat = SOFA_BACK_ROWS * S  # the backrest's foot, the seat's back
    g = canvas(w, h)
    # backrest
    rect(g, 3, 1, w - 3, seat, FABRIC)
    rect(g, 3, 1, w - 3, 4, FABRIC_HI)
    rect(g, 3, seat - 2, w - 3, seat, FABRIC_SH)
    # seat cushions, the seam showing between them as it does from behind
    rect(g, 6, seat, w - 6, h - 4, FABRIC_SEAM)
    for c in SOFA_SEAT_COLS:
        x0, x1 = (c - SOFA_SEAT_HALF) * S + 1, (c + SOFA_SEAT_HALF) * S - 1
        x0, x1 = max(x0, 6), min(x1, w - 6)
        rect(g, x0, seat, x1, h - 4, FABRIC)
        rect(g, x0 + 1, seat + 1, x1 - 1, seat + 3, FABRIC_HI)
        rect(g, x0, h - 6, x1, h - 4, FABRIC_SH)
    rect(g, 5, seat, w - 5, seat + 1, FABRIC_SEAM)
    # arms, a column in from each edge so the outline fits the canvas
    for ax, inner in ((1, 6), (w - 7, w - 7)):
        rect(g, ax, 3, ax + 6, h - 3, FABRIC)
        rect(g, ax, 3, ax + 6, 6, FABRIC_HI)
        rect(g, inner, 6, inner + 1, h - 3, FABRIC_SEAM)
    # front rail, feet
    rect(g, 6, h - 4, w - 6, h - 3, FABRIC_SEAM)
    rect(g, 3, h - 2, 6, h, KEY_DK)
    rect(g, w - 6, h - 2, w - 3, h, KEY_DK)
    union_outline(g)
    return g


def fade_into_shade(g, x0, x1, y0, y1):
    """Fabric dithering into shade from row `y0`, solid shade its last two rows."""
    for y in range(y0, y1):
        for x in range(x0, x1):
            if y >= y1 - 2 or (x + y) % 2 == 0:
                g[y][x] = FABRIC_SH


def meeting_sofa_north():
    """The north-facing sofa: three cushions beyond the backrest, its lit
    ridge, a back panel dithering into shade, arms, feet."""
    w, h = SOFA_W * S, SOFA_H * S
    seat, ridge_end = SOFA_SEAT_ROWS * S, (SOFA_SEAT_ROWS + SOFA_RIDGE_ROWS) * S
    panel_end = h - 2
    g = canvas(w, h)
    rect(g, 5, 2, w - 5, seat, FABRIC_SEAM)
    for c in SOFA_SEAT_COLS:
        x0, x1 = max((c - SOFA_SEAT_HALF) * S + 1, 5), min((c + SOFA_SEAT_HALF) * S - 1, w - 5)
        rect(g, x0, 2, x1, seat, FABRIC)
        rect(g, x0 + 1, 3, x1 - 1, 5, FABRIC_HI)
        rect(g, x0, seat - 2, x1, seat, FABRIC_SH)
    # the seam closes the seat, so the backrest band opens on its lit ridge
    rect(g, 3, seat - 1, w - 3, seat, FABRIC_SEAM)
    rect(g, 3, seat, w - 3, ridge_end, FABRIC_HI)
    rect(g, 3, ridge_end - 1, w - 3, ridge_end, FABRIC)
    # back panel, dithering into shade in its lower half
    rect(g, 1, ridge_end, w - 1, panel_end, FABRIC)
    shade = ridge_end + (panel_end - ridge_end) // 2
    fade_into_shade(g, 1, w - 1, shade, panel_end)
    # arms, a column in from each edge so the outline fits the canvas
    for ax, inner in ((1, 6), (w - 7, w - 7)):
        rect(g, ax, 1, ax + 6, panel_end, FABRIC)
        rect(g, ax, 1, ax + 6, 4, FABRIC_HI)
        fade_into_shade(g, ax, ax + 6, shade, panel_end)
        rect(g, inner, 4, inner + 1, panel_end, FABRIC_SEAM)
    rect(g, 3, panel_end, 6, h, KEY_DK)
    rect(g, w - 6, panel_end, w - 3, h, KEY_DK)
    union_outline(g)
    return g


# ---- the plants ----------------------------------------------------------------
def blade(g, x0, y0, ang, length, width, lit_side=1, rib=True):
    """One leaf from (x0, y0) along `ang` (radians), drawn over the leaves
    behind it: lit on the half facing the window, a dark rim along its
    lower-right edge so it parts from the leaf it covers, a midrib if long."""
    ca, sa = math.cos(ang), math.sin(ang)
    mask = set()
    for y in range(len(g)):
        for x in range(len(g[0])):
            dx, dy = x + 0.5 - x0, y + 0.5 - y0
            u, v = dx * ca + dy * sa, -dx * sa + dy * ca
            t = u / length  # widest a third of the way out, tapering to a point
            if 0 <= t <= 1 and abs(v) <= width * 2.2 * t ** 0.6 * (1 - t):
                mask.add((x, y, u, v))
    cells = {(x, y) for x, y, _, _ in mask}
    for x, y, u, v in mask:
        if (x + 1, y) not in cells or (x, y + 1) not in cells:
            g[y][x] = LEAF_DK
        elif rib and abs(v) < 0.6 and 1.5 < u < length - 1.5:
            g[y][x] = LEAF_SH if (v > 0) == (lit_side > 0) else LEAF_DK
        elif (v > 0) == (lit_side > 0):
            g[y][x] = LEAF_HI if u < length * 0.55 and abs(v) > width * 0.35 else LEAF
        else:
            g[y][x] = LEAF_SH


def planter(g, cx, top, bot, half_top, half_bot, dark=False):
    """A pot tapering from its rim down: lit west, shaded east, a rim a pixel
    proud of the body with its top edge lit."""
    body, lit_, shade, rim = (SHADOW, SLATE, KEY_DK, SLATE) if dark else (POT, POT_HI, POT_SH, POT_HI)
    for y in range(top + 2, bot):
        t = (y - top - 2) / max(1, bot - top - 3)
        half = half_top + (half_bot - half_top) * t
        for x in range(len(g[0])):
            d = x + 0.5 - cx
            if abs(d) <= half:
                g[y][x] = lit_ if d < -half * 0.5 else shade if d > half * 0.45 else body
    rect(g, int(cx - half_top - 1), top, int(cx + half_top + 1) + 1, top + 2, body)
    rect(g, int(cx - half_top - 1), top, int(cx + half_top + 1) + 1, top + 1, rim)


def plant_bush():
    """A leafy bush in terracotta: long pointed leaves fanning up and out from
    the pot, every tip clear of its neighbours."""
    g = canvas(6 * S, 7 * S)
    for ang, ln, wd, side in ((-2.95, 10.0, 2.4, 1), (-0.2, 10.0, 2.4, -1), (-2.55, 12.0, 2.6, 1),
                              (-0.6, 12.0, 2.6, -1), (-1.35, 14.5, 2.6, -1), (-1.8, 14.0, 2.6, 1),
                              (-2.2, 12.0, 2.6, 1), (-0.95, 12.0, 2.6, -1), (-1.57, 10.0, 2.4, 1)):
        blade(g, 12, 18.5, ang, ln, wd, side)
    planter(g, 12, 17, 27, 6.0, 4.5)
    union_outline(g)
    return g


def plant_tall():
    """A tall palm in a dark pot: a slim trunk, and long leaves fanning out from
    its crown and from one joint below it."""
    g = canvas(6 * S, 10 * S)
    rect(g, 11, 10, 13, 31, BROWN)
    rect(g, 12, 10, 13, 31, WOOD_DK)
    for ang, ln, side, y in ((-2.5, 10.0, 1, 21), (-0.64, 10.0, -1, 21),
                             (-2.95, 10.5, 1, 11), (-0.2, 10.5, -1, 11), (-2.45, 11.5, 1, 11),
                             (-0.7, 11.5, -1, 11), (-1.9, 11.0, 1, 11), (-1.25, 11.0, -1, 11)):
        blade(g, 12, y, ang, ln, 2.4, side, rib=False)
    planter(g, 12, 30, 39, 6.0, 4.8, dark=True)
    union_outline(g)
    return g


def stem(g, pts, r, key):
    """Stroke a polyline of radius `r`: a flower's stem."""
    for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
        steps = int(max(abs(x1 - x0), abs(y1 - y0)) * 2) + 1
        for i in range(steps + 1):
            t = i / steps
            px, py = x0 + (x1 - x0) * t, y0 + (y1 - y0) * t
            for y in range(int(py - r) - 1, int(py + r) + 2):
                for x in range(int(px - r) - 1, int(px + r) + 2):
                    if (x + 0.5 - px) ** 2 + (y + 0.5 - py) ** 2 <= r * r:
                        put(g, x, y, key)


def plant_flower():
    """Potted flowers: a cluster of red and gold blooms over two leaves."""
    g = canvas(6 * S, 6 * S)
    blooms = [(8.5, 7.0, PETAL_R), (15.5, 6.0, PETAL_Y), (12, 3.5, PETAL_R), (6.5, 11.0, PETAL_Y),
              (17.5, 10.5, PETAL_R), (12, 9.5, PETAL_Y)]
    for bx, by, _ in blooms:
        stem(g, [(12, 16), (bx, by + 1)], 0.5, LEAF_DK)
    blade(g, 12, 16, -2.55, 7.5, 2.6, 1, rib=False)
    blade(g, 12, 16, -0.6, 7.5, 2.6, -1, rib=False)
    for bx, by, col in blooms:
        disc(g, bx, by, 2.4, col)
        put(g, int(bx) - 1, int(by) - 1, PETAL_HI)
        put(g, int(bx), int(by), GOLD if col == PETAL_R else DARK_RED)
    planter(g, 12, 15, 23, 4.5, 3.5)
    union_outline(g)
    return g


def plant_succulent():
    """A succulent rosette in a wide shallow pot: plump pointed leaves, the
    inner ones standing up over the outer."""
    g = canvas(5 * S, 4 * S)
    for i in range(7):
        ang = -math.pi + 0.15 + i * (math.pi - 0.3) / 6
        blade(g, 10, 10, ang, 7.0, 2.6, 1 if i < 3 else -1, rib=False)
    for ang in (-2.05, -1.1):
        blade(g, 10, 10, ang, 6.0, 2.4, 1 if ang < -1.57 else -1, rib=False)
    planter(g, 10, 9, 15, 6.5, 5.0)
    union_outline(g)
    return g


# ---- furniture: fixtures, boards, shelves, screens, the pantry -------------------
CORK, CORK_SH, STEEL, STEEL_SH = "Δ", "π", "K", "ξ"
BOOKS = (RED, BLUE, GOLD, FABRIC, TAN, DARK_RED, VERMILION, GREEN)


def floor_lamp():
    """A floor lamp: a weighted base, a slim pole, a drum shade lit from within,
    the bulb glowing at its open foot."""
    g = canvas(4 * S, 10 * S)
    rect(g, 3, 36, 13, 39, LAMP)
    rect(g, 3, 36, 13, 37, LAMP_HI)
    rect(g, 7, 10, 9, 36, LAMP)
    rect(g, 7, 10, 8, 36, LAMP_HI)
    rect(g, 2, 2, 14, 10, OFFWHITE)
    rect(g, 3, 1, 13, 2, OFFWHITE)
    rect(g, 10, 1, 14, 10, OFFWHITE_SH)
    rect(g, 2, 9, 14, 10, BULB)
    rect(g, 5, 10, 11, 11, BULB)
    union_outline(g)
    return g


def filing_cabinet():
    """A steel filing cabinet: three drawers, each with a pull and a label slot,
    the top catching the light."""
    g = canvas(4 * S, 6 * S)
    rect(g, 1, 1, 15, 23, STEEL)
    rect(g, 1, 1, 15, 2, WHITE)
    rect(g, 12, 2, 15, 23, STEEL_SH)
    for i in range(3):
        y0 = 3 + i * 7
        if i < 2:  # between drawers; the last one's foot is the cabinet's
            rect(g, 2, y0 + 6, 15, y0 + 7, SHADOW)
        rect(g, 5, y0 + 2, 11, y0 + 3, OFFWHITE)
        rect(g, 6, y0 + 4, 10, y0 + 5, KEY_DK)
    union_outline(g)
    return g


def bulletin_board():
    """A corkboard in a wooden frame, notes pinned across it."""
    g = canvas(10 * S, 6 * S)
    rect(g, 1, 1, 39, 23, WOOD)
    rect(g, 1, 1, 39, 2, WOOD_LT)
    rect(g, 3, 3, 37, 21, CORK)
    for x, y in ((6, 5), (14, 17), (22, 8), (30, 15), (33, 5), (9, 12), (27, 19)):
        put(g, x, y, CORK_SH)
    for x0, y0, c in ((5, 5, GOLD), (13, 4, CYAN), (22, 6, PINK), (30, 4, OFFWHITE), (8, 13, OFFWHITE),
                      (18, 13, GOLD), (27, 13, CYAN)):
        rect(g, x0, y0, x0 + 6, y0 + 6, c)
        rect(g, x0 + 1, y0 + 2, x0 + 5, y0 + 3, PRINT)
        rect(g, x0 + 1, y0 + 4, x0 + 4, y0 + 5, PRINT)
        put(g, x0 + 3, y0, RED)
    union_outline(g)
    return g


EXIT_GLYPHS = {"E": ("111", "100", "110", "100", "111"), "X": ("101", "101", "010", "101", "101"),
               "I": ("1",) * 5, "T": ("111", "010", "010", "010", "010")}


def exit_sign():
    """A lit EXIT sign: dark red letters on a warm-lit panel in a red frame."""
    w, h = 5 * S, 3 * S
    g = canvas(w, h)
    rect(g, 1, 1, w - 1, h - 1, RED)
    rect(g, 2, 2, w - 2, h - 2, BULB)
    x = 4
    for letter in "EXIT":
        rows = EXIT_GLYPHS[letter]
        for r, row in enumerate(rows):
            for c, bit in enumerate(row):
                if bit == "1":
                    put(g, x + c, 3 + r, DARK_RED)
        x += len(rows[0]) + 1
    union_outline(g)
    return g


def whiteboard():
    """A mobile whiteboard: an aluminium frame round a sprint board of sticky
    notes and a burndown line, a marker tray, a wheeled stand."""
    g = canvas(14 * S, 11 * S)
    rect(g, 1, 1, 55, 30, ALU)
    rect(g, 1, 1, 55, 2, WHITE)
    rect(g, 1, 29, 55, 30, ALU_DK)
    rect(g, 3, 3, 53, 28, OFFWHITE)
    rect(g, 3, 26, 53, 28, OFFWHITE_SH)
    for i, x0 in enumerate((6, 20, 34)):
        rect(g, x0, 5, x0 + 11, 6, INK)
        for r in range(2 + (i != 1)):
            c = (GOLD, CYAN, PINK)[(i + r) % 3]
            rect(g, x0 + 1, 8 + r * 6, x0 + 8, 13 + r * 6, c)
            rect(g, x0 + 2, 10 + r * 6, x0 + 6, 11 + r * 6, PRINT)
    for x in (17, 31):
        rect(g, x, 5, x + 1, 25, INK)
    for t in range(12):
        put(g, 42 + t, 10 + t + (t % 3 == 1), RED)
    rect(g, 12, 30, 44, 32, ALU_DK)
    rect(g, 16, 30, 20, 31, RED)
    rect(g, 22, 30, 26, 31, BLUE)
    for lx in (10, 44):
        rect(g, lx, 32, lx + 2, 41, ALU_DK)
        rect(g, lx - 3, 41, lx + 5, 43, KEY_DK)
    rect(g, 10, 36, 46, 37, ALU_DK)
    union_outline(g)
    return g


def bookshelf():
    """A wooden bookshelf: three shelves of books, one leaning, a potted sprig
    and a box on the top shelf, a cabinet below."""
    g = canvas(8 * S, 12 * S)
    rect(g, 1, 1, 31, 46, WOOD)
    rect(g, 1, 1, 31, 2, WOOD_LT)
    rect(g, 28, 2, 31, 46, WOOD_SH)
    rect(g, 3, 3, 29, 34, WOOD_DK)
    rng = random.Random(21)
    for shelf in range(3):
        floor_y = 12 + shelf * 11
        x = 4
        while x < 27:
            bw = rng.choice((2, 2, 3))
            bh = rng.randrange(6, 9)
            col = BOOKS[rng.randrange(len(BOOKS))]
            if shelf == 0 and x > 18:
                break
            rect(g, x, floor_y - bh, min(27, x + bw), floor_y, col)
            put(g, x, floor_y - bh, WHITE if col in (BLUE, DARK_RED, FABRIC) else PRINT)
            x += bw + (1 if rng.random() < 0.25 else 0)
        if shelf == 1:
            for t in range(7):  # a book leaning on its neighbour
                put(g, 24 + t // 3, floor_y - 1 - t, RED)
                put(g, 25 + t // 3, floor_y - 1 - t, RED)
        rect(g, 3, floor_y, 29, floor_y + 2, WOOD)
        rect(g, 3, floor_y, 29, floor_y + 1, WOOD_LT)
    rect(g, 21, 7, 26, 12, POT)  # a potted sprig and a box on the top shelf
    rect(g, 21, 7, 26, 8, POT_HI)
    for x, y in ((22, 5), (23, 4), (24, 5), (25, 3), (23, 6)):
        put(g, x, y, LEAF)
    rect(g, 4, 9, 9, 12, TAN)
    rect(g, 3, 35, 29, 45, WOOD)
    rect(g, 3, 35, 29, 36, WOOD_LT)
    rect(g, 15, 36, 16, 45, WOOD_DK)
    rect(g, 12, 39, 14, 41, GREY)
    rect(g, 17, 39, 19, 41, GREY)
    union_outline(g)
    return g


SLIDE_BAR_PITCH = 4


def slide(g, x0, y0, x1, y1):
    """A lit slide sized to its screen: a title bar, bullet lines, a bar chart."""
    rect(g, x0, y0, x1, y1, DISPLAY)
    rect(g, x0 + 2, y0 + 2, x1 - 2, y0 + 4, CYAN)
    w = x1 - x0
    for i in range(3):
        put(g, x0 + 3, y0 + 7 + i * 3, GREY)
        rect(g, x0 + 5, y0 + 7 + i * 3, x0 + 5 + w * (3 if i % 2 else 4) // 10, y0 + 8 + i * 3, GREY)
    bars = (0.4, 0.65, 0.5, 0.85)
    # the chart hangs from the glass's east edge, so a narrow screen keeps it
    bx, base = x1 - 2 - len(bars) * SLIDE_BAR_PITCH, y1 - 3
    for i, frac in enumerate(bars):
        top = base - int((base - y0 - 8) * frac)
        x = bx + i * SLIDE_BAR_PITCH
        rect(g, x, top, x + SLIDE_BAR_PITCH - 1, base, BLUE if i % 2 else LEAF)
    rect(g, bx - 1, base, x1 - 2, base + 1, GREY)
    for y in range(y0, y1):  # a soft sheen across the glass
        for x in range(x0, x1):
            if (x + y) % 23 == 0 and g[y][x] == DISPLAY:
                g[y][x] = DISPLAY_SHEEN


def meeting_screen():
    """A wall-mounted widescreen showing a slide, on its bracket."""
    g = canvas(14 * S, 12 * S)
    rect(g, 1, 3, 55, 39, SHADOW)
    rect(g, 1, 3, 55, 4, SLATE)
    slide(g, 3, 5, 53, 37)
    rect(g, 22, 39, 34, 42, BEZEL)
    rect(g, 26, 42, 30, 47, SHADOW)
    union_outline(g)
    return g


def fridge(g, x0, y0, w, h):
    """A fridge: a freezer door over the main one, each with a steel handle,
    two magnets, the east side in shade."""
    seam = y0 + h * 2 // 5
    rect(g, x0, y0, x0 + w, y0 + h, OFFWHITE)
    rect(g, x0 + w - max(2, w // 7), y0, x0 + w, y0 + h, OFFWHITE_SH)
    rect(g, x0, y0, x0 + w, y0 + 1, WHITE)
    rect(g, x0, seam, x0 + w, seam + 1, OFFWHITE_SH)
    hx = x0 + w - max(2, w // 7) - 3
    rect(g, hx, y0 + 3, hx + 1, seam - 2, STEEL)
    rect(g, hx, seam + 3, hx + 1, seam + 3 + (y0 + h - seam) // 2, STEEL)
    rect(g, x0 + 3, y0 + h - 9, x0 + 5, y0 + h - 7, CYAN)
    rect(g, x0 + 7, y0 + h - 8, x0 + 9, y0 + h - 6, RED)


def counter(g, x0, x1, top, bottom):
    """A counter: a lit top edge, cabinet doors with pulls below it."""
    rect(g, x0, top, x1, bottom, WOOD)
    rect(g, x0, top, x1, top + 2, WOOD_HI)
    rect(g, x0, top + 2, x1, top + 3, WOOD_SH)
    for cx in range(x0 + 10, x1 - 4, 12):
        rect(g, cx, top + 4, cx + 1, bottom - 1, WOOD_DK)
        rect(g, cx - 3, top + 6, cx - 1, top + 7, STEEL)
        rect(g, cx + 2, top + 6, cx + 4, top + 7, STEEL)


def espresso(g, x0, y0):
    rect(g, x0, y0, x0 + 12, y0 + 12, STEEL)
    rect(g, x0, y0, x0 + 12, y0 + 1, WHITE)
    rect(g, x0 + 9, y0, x0 + 12, y0 + 12, STEEL_SH)
    rect(g, x0 + 2, y0 + 2, x0 + 7, y0 + 4, CYAN)
    rect(g, x0 + 4, y0 + 6, x0 + 8, y0 + 7, SHADOW)
    rect(g, x0 + 4, y0 + 8, x0 + 7, y0 + 11, MUG)


def microwave(g, x0, y0, w):
    rect(g, x0, y0, x0 + w, y0 + 10, BEZEL)
    rect(g, x0, y0, x0 + w, y0 + 1, SLATE)
    rect(g, x0 + 2, y0 + 2, x0 + w - 6, y0 + 8, INK)
    rect(g, x0 + 3, y0 + 3, x0 + 6, y0 + 4, DISPLAY_SHEEN)
    rect(g, x0 + w - 4, y0 + 2, x0 + w - 1, y0 + 4, CYAN)
    rect(g, x0 + w - 4, y0 + 5, x0 + w - 1, y0 + 6, KEYCAP)
    rect(g, x0 + w - 4, y0 + 7, x0 + w - 1, y0 + 8, KEYCAP)


def bottle(g, x0, y0, col):
    rect(g, x0, y0 + 3, x0 + 3, y0 + 10, col)
    rect(g, x0 + 1, y0, x0 + 2, y0 + 3, col)
    rect(g, x0, y0 + 4, x0 + 1, y0 + 9, WHITE)


def pantry():
    """The office pantry: a fridge, an espresso counter with mugs, a microwave
    counter with bottles."""
    g = canvas(32 * S, 10 * S)
    fridge(g, 1, 1, 26, 38)
    counter(g, 32, 76, 16, 39)
    espresso(g, 38, 4)
    for x in (54, 60):
        desk_mug(g, x, 12)
    counter(g, 82, 127, 16, 39)
    microwave(g, 86, 5, 22)
    bottle(g, 112, 6, GOLD)
    bottle(g, 117, 6, RED)
    bottle(g, 122, 6, LEAF)
    union_outline(g)
    return g


def pantry_small():
    """The compact pantry: a vending machine lit from within, a mini fridge, a
    coffee-and-microwave counter."""
    g = canvas(20 * S, 8 * S)
    rect(g, 1, 1, 15, 31, BEZEL)
    rect(g, 1, 1, 15, 2, SLATE)
    rect(g, 3, 3, 12, 22, INK)
    for r in range(4):
        for c in range(3):
            rect(g, 4 + c * 3, 4 + r * 5, 6 + c * 3, 7 + r * 5, (RED, GOLD, BLUE, LEAF)[(r + c) % 4])
    rect(g, 13, 6, 14, 10, CYAN)
    rect(g, 3, 24, 12, 27, KEY_DK)
    fridge(g, 18, 9, 14, 22)
    counter(g, 36, 79, 14, 31)
    espresso(g, 40, 2)
    microwave(g, 56, 4, 20)
    union_outline(g)
    return g


def snack_shelf():
    """A wooden rack of snack packets, three shelves, a drawer below."""
    g = canvas(7 * S, 10 * S)
    rect(g, 1, 1, 27, 40, WOOD)
    rect(g, 1, 1, 27, 2, WOOD_LT)
    rect(g, 24, 2, 27, 40, WOOD_SH)
    rect(g, 3, 3, 24, 30, WOOD_DK)
    rng = random.Random(33)
    for shelf in range(3):
        yb = 11 + shelf * 9
        x = 4
        while x < 22:
            pw = rng.choice((3, 4))
            ph = rng.randrange(4, 7)
            col = (GOLD, RED, BLUE, LEAF, CYAN, VERMILION)[rng.randrange(6)]
            rect(g, x, yb - ph, min(24, x + pw), yb, col)
            rect(g, x, yb - ph, min(24, x + pw), yb - ph + 1, WHITE)
            x += pw + 1
        rect(g, 3, yb, 24, yb + 2, WOOD)
        rect(g, 3, yb, 24, yb + 1, WOOD_LT)
    rect(g, 3, 31, 24, 37, WOOD)
    rect(g, 11, 33, 16, 34, STEEL)
    union_outline(g)
    return g


def tv_stand():
    """A lit TV on a slim neck over a low media cabinet."""
    g = canvas(10 * S, 10 * S)
    rect(g, 1, 1, 39, 24, SHADOW)
    rect(g, 1, 1, 39, 2, SLATE)
    slide(g, 3, 3, 37, 22)
    rect(g, 18, 24, 22, 29, BEZEL)
    rect(g, 4, 29, 36, 40, WOOD)
    rect(g, 4, 29, 36, 30, WOOD_LT)
    rect(g, 4, 38, 36, 40, WOOD_SH)
    rect(g, 19, 31, 20, 37, WOOD_DK)
    rect(g, 15, 33, 18, 34, STEEL)
    rect(g, 22, 33, 25, 34, STEEL)
    union_outline(g)
    return g


def phone_booth():
    """A glass privacy pod: a dark frame round tinted glass with streaks of
    reflection, a stool and a shelf inside, its occupied LED on the roof."""
    g = canvas(6 * S, 12 * S)
    rect(g, 1, 1, 23, 47, BEZEL)
    rect(g, 1, 1, 23, 2, SLATE)
    rect(g, 9, 2, 15, 4, BULB)
    rect(g, 3, 6, 21, 40, CYAN)
    for y in range(6, 40):
        for x in range(3, 21):
            if (x - y * 0.6) % 13 < 1.5:
                g[y][x] = WHITE
    rect(g, 3, 20, 21, 22, BEZEL)  # the transom
    rect(g, 7, 26, 17, 28, WOOD)  # the shelf inside
    rect(g, 10, 33, 14, 40, SHADOW)  # a stool
    rect(g, 3, 40, 21, 46, WOOD)
    rect(g, 3, 40, 21, 41, WOOD_LT)
    rect(g, 17, 42, 19, 44, STEEL)
    union_outline(g)
    return g


def standing_desk():
    """A sit-stand desk with a lit laptop, on slim legs."""
    g = canvas(8 * S, 8 * S)
    rect(g, 1, 1, 31, 17, WOOD)
    rect(g, 1, 1, 31, 2, WOOD_LT)
    rect(g, 1, 15, 31, 17, WOOD_SH)
    rect(g, 7, 3, 25, 11, BEZEL)
    rect(g, 8, 4, 24, 10, CYAN)
    rect(g, 9, 5, 17, 6, WHITE)
    rect(g, 9, 7, 14, 8, WHITE)
    rect(g, 6, 11, 26, 14, KEYCAP)
    rect(g, 6, 11, 26, 12, GREY)
    for lx in (4, 26):
        rect(g, lx, 17, lx + 2, 29, STEEL_SH)
        rect(g, lx - 2, 29, lx + 4, 31, KEY_DK)
    union_outline(g)
    return g


# ---- the corridor appliances and the meeting table ---------------------------------
# Drawn in the theme's appliance keys (pixel_painter::palette::appliance_overrides).
VEND_BODY, VEND_BODY_LT, VEND_BODY_SH = "Б", "Ъ", "ъ"
VEND_PANEL, VEND_PANEL_LT = "П", "п"
VEND_DRINKS = ("Ч", "Ш", "Щ", "Э")
VEND_TRIM, VEND_DARK = "Ф", "Ы"
PRN_BODY, PRN_BODY_SH = "Ю", "ю"
PRN_TOP, PRN_TOP_LT = "Я", "я"
PRN_GLASS = "Ё"
PRN_PAPER, PRN_PAPER_SH = "Й", "й"
PRN_TRAY = "Ц"
LED_ON = GREEN
# A vend, one loop per drink: the panel flashes as the pick's can leaves its
# shelf, falls past the shelf below, and waits in the tray until it is taken.
VEND_STEPS = 6  # rest, falling, three in the tray, taken
PRINT_STEPS = 8


def vend_can(g, x, y, drink):
    """A can 2 wide, 3 tall, its lid catching the light."""
    rect(g, x, y, x + 2, y + 3, drink)
    put(g, x, y, WHITE)


# The vending machine's glass, in canvas coordinates: 3 columns x 3 shelves of
# cans; drink `i` stands at column `i % 3`, shelf `i // 3` of the first four
# slots, and the rest of the shelves repeat the rotation.
VEND_GLASS = (2, 6, 11, 18)  # x0, y0, x1, y1
VEND_CAN_COLS = (3, 6, 9)
VEND_SHELVES = (7, 11, 15)
VEND_TRAY = (3, 20, 10, 22)  # x0, y0, x1, y1: the pickup recess
# The glass's glint: a short diagonal in the column and row no can stands in, so
# it reads the same in every frame.
VEND_GLINT = ((3, 6), (2, 7), (2, 8))


def vending_body():
    """The machine at rest: a lit brand panel over a glass front of cans on
    three shelves, a keypad and a coin plate on the shaded east side, the
    pickup flap below."""
    g = canvas(4 * S, 6 * S)
    rect(g, 1, 1, 15, 23, VEND_BODY)
    rect(g, 1, 1, 15, 2, VEND_BODY_LT)
    rect(g, 12, 2, 15, 23, VEND_BODY_SH)
    rect(g, 2, 2, 12, 5, VEND_PANEL)
    rect(g, 2, 2, 12, 3, VEND_PANEL_LT)
    x0, y0, x1, y1 = VEND_GLASS
    rect(g, x0, y0, x1, y1, VEND_DARK)
    for si, sy in enumerate(VEND_SHELVES):
        for ci, cx in enumerate(VEND_CAN_COLS):
            vend_can(g, cx, sy, VEND_DRINKS[(si * 3 + ci) % len(VEND_DRINKS)])
        rect(g, x0, sy + 3, x1, sy + 4, SLATE)  # the shelf under them
    for x, y in VEND_GLINT:
        put(g, x, y, VEND_BODY_LT)
    for k in range(3):  # the keypad
        put(g, 13, 7 + k * 2, KEYCAP)
    rect(g, 12, 13, 15, 17, VEND_TRIM)  # the coin plate, a slit through it
    rect(g, 13, 14, 14, 16, VEND_DARK)
    tx0, ty0, tx1, ty1 = VEND_TRAY
    rect(g, tx0 - 1, ty0 - 1, tx1 + 1, ty0, VEND_BODY_SH)
    rect(g, tx0, ty0, tx1, ty1, VEND_DARK)
    return g


def vending_machine():
    """A vending machine: at rest, then a vend per drink, the panel flashing
    as the pick's can leaves its shelf, falls in front of the shelf below and
    lands in the tray."""
    rest = vending_body()
    frames = [rest]
    for drink in range(len(VEND_DRINKS)):
        col, shelf = VEND_CAN_COLS[drink % 3], VEND_SHELVES[drink // 3]
        for step in range(VEND_STEPS):
            g = [row[:] for row in rest]
            if 1 <= step <= 4:  # its slot stands empty
                rect(g, col, shelf, col + 2, shelf + 3, VEND_DARK)
                rect(g, 2, 2, 12, 3, WHITE)  # the panel flashes while it vends
            if step == 1:  # falling past the shelf edge below it, tipped off the cans' grid
                vend_can(g, col - 1, shelf + 3, VEND_DRINKS[drink])
            if 2 <= step <= 4:
                tx0, ty0, _, _ = VEND_TRAY
                rect(g, tx0 + 2, ty0, tx0 + 5, ty0 + 2, VEND_DRINKS[drink])  # lying in the tray
                put(g, tx0 + 2, ty0, WHITE)
            frames.append(g)
    return [union_outlined(f) for f in frames]


def union_outlined(g):
    union_outline(g)
    return g


def vending_machine_1x():
    """The vending machine at 1x: its lit panel over two shelves of drinks,
    the coin plate, the dark pickup row; a vend darkens the pick's cell and
    shows its can in the coin plate's cell."""
    def body():
        g = canvas(4, 6)
        rect(g, 0, 0, 4, 6, VEND_BODY)
        rect(g, 0, 0, 4, 1, VEND_PANEL)
        for i, drink in enumerate(VEND_DRINKS):
            put(g, 1 + i % 2, 1 + i // 2, drink)
        rect(g, 3, 1, 4, 5, VEND_BODY_SH)
        put(g, 2, 4, VEND_TRIM)
        rect(g, 0, 5, 4, 6, VEND_DARK)
        return g
    rest = body()
    frames = [rest]
    for drink in range(len(VEND_DRINKS)):
        for step in range(VEND_STEPS):
            g = [row[:] for row in rest]
            if 1 <= step <= 4:
                put(g, 1 + drink % 2, 1 + drink // 2, VEND_DARK)
            if 2 <= step <= 4:
                put(g, 2, 4, VEND_DRINKS[drink])
            frames.append(g)
    return frames


PRN_GLASS_SPAN = (3, 14)  # the scanner glass's columns, half-open
PRN_SLOT_Y = 9
PRN_TRAY_Y = 12


def printer_body(scan=None, led=False, page=0):
    """The printer: a dark lid with its scanner glass and a status light, the
    light chassis with its paper slot, the output tray and its stack. `scan`
    lights the glass's column under the scan bar, `led` the status light,
    `page` the rows of a page slid out of the slot."""
    g = canvas(5 * S, 4 * S)
    rect(g, 1, 1, 19, 15, PRN_BODY)
    rect(g, 16, 6, 19, 15, PRN_BODY_SH)
    rect(g, 1, 1, 19, 6, PRN_TOP)
    rect(g, 1, 1, 19, 2, PRN_TOP_LT)
    gx0, gx1 = PRN_GLASS_SPAN
    rect(g, gx0, 3, gx1, 5, PRN_GLASS)
    if scan is not None:
        rect(g, scan, 3, scan + 1, 5, WHITE)
    put(g, 16, 3, LED_ON if led else KEY_DK)
    put(g, 17, 3, KEYCAP)
    rect(g, 4, PRN_SLOT_Y, 15, PRN_SLOT_Y + 1, SHADOW)
    rect(g, 1, PRN_TRAY_Y, 19, 15, PRN_TRAY)
    rect(g, 5, PRN_TRAY_Y, 14, 14, PRN_PAPER)
    rect(g, 5, 14, 14, 15, PRN_PAPER_SH)
    for r in range(page):
        rect(g, 6, PRN_SLOT_Y + 1 + r, 13, PRN_SLOT_Y + 2 + r, PRN_PAPER)
        if r % 2:
            rect(g, 7, PRN_SLOT_Y + 1 + r, 11, PRN_SLOT_Y + 2 + r, PRINT)
    return g


def printer():
    """A printer: at rest, then a print: the scan bar sweeps the glass as the
    page feeds out onto the stack, the status light blinking."""
    gx0, gx1 = PRN_GLASS_SPAN
    sweep = [gx0 + (gx1 - gx0 - 1) * i // 3 for i in range(4)]
    busy = [
        dict(scan=sweep[0], led=True),
        dict(scan=sweep[1], led=False, page=1),
        dict(scan=sweep[2], led=True, page=2),
        dict(scan=sweep[3], led=False, page=3),
        dict(led=True, page=3),
        dict(led=False, page=3),
        dict(led=True, page=3),
        dict(led=False),
    ]
    assert len(busy) == PRINT_STEPS
    return [union_outlined(printer_body(**f)) for f in [{}] + busy]


def printer_1x():
    """The printer at 1x: a dark lid lit along its west end over its glass
    strip, the light chassis shaded east between its tray-grey sides, the stack
    in the tray; a print sweeps a light across the glass as a page feeds out
    over the chassis, printed as it goes."""
    def body(scan=None, page=0):
        g = canvas(5, 4)
        rect(g, 0, 0, 5, 1, PRN_TOP)
        put(g, 0, 0, PRN_TOP_LT)
        rect(g, 1, 0, 4, 1, PRN_GLASS)
        rect(g, 0, 1, 5, 3, PRN_TRAY)
        rect(g, 1, 1, 4, 3, PRN_BODY)
        rect(g, 3, 1, 4, 3, PRN_BODY_SH)
        rect(g, 0, 3, 5, 4, PRN_TRAY)
        rect(g, 1, 3, 4, 4, PRN_PAPER)
        if scan is not None:
            put(g, scan, 0, WHITE)
        if page >= 1:  # the page's edge out of the slot
            rect(g, 1, 2, 4, 3, PRN_PAPER)
        if page >= 2:  # and the printed page above it
            rect(g, 1, 1, 4, 2, PRN_PAPER)
            put(g, 2, 1, PRINT)
        return g
    steps = [dict(scan=1), dict(scan=2, page=1), dict(scan=3, page=2), dict(page=2),
             dict(page=2), dict(page=2), dict(page=1), dict()]
    assert len(steps) == PRINT_STEPS
    return [body()] + [body(**f) for f in steps]


# `Furniture::MeetingTable`'s visual box.
TABLE_W, TABLE_H = 11, 5
# The props on the @4x table, (x0, y0, x1, y1) half-open: the grain keeps clear
# of each.
TABLE_LAPTOP = (5, 4, 15, 10)
TABLE_PAD = (19, 5, 30, 12)  # the notepad, its shaded edge and the pen
TABLE_MUG = (34, 5, 39, 9)


def meeting_table():
    """The meeting table: a wood top lit along its far edge with a board seam
    and grain, its near edge catching the light over the front face; a closed
    laptop, a notepad with a pen, and a mug, set well apart."""
    w, h = TABLE_W * S, TABLE_H * S
    g = canvas(w, h)
    rng = random.Random(11)
    front = h - 4
    rect(g, 1, 1, w - 1, front, WOOD)
    rect(g, 1, 1, w - 1, 2, WOOD_LT)
    rect(g, 1, front // 2, w - 1, front // 2 + 1, WOOD_SH)
    clear = [(x0 - 1, y0 - 1, x1 + 1, y1 + 1) for x0, y0, x1, y1 in (TABLE_LAPTOP, TABLE_PAD, TABLE_MUG)]
    for _ in range(6):
        y = rng.randrange(3, front - 2)
        sx = rng.randrange(4, w - 12)
        ex = sx + rng.randrange(5, 9)
        if y == front // 2 or any(y0 <= y < y1 and sx < x1 and ex > x0 for x0, y0, x1, y1 in clear):
            continue
        rect(g, sx, y, ex, y + 1, WOOD_SH)
    rect(g, 1, front, w - 1, h - 1, WOOD_SH)
    rect(g, 1, front, w - 1, front + 1, WOOD_HI)
    x0, y0, x1, y1 = TABLE_LAPTOP  # shut
    rect(g, x0, y0, x1, y1, BEZEL)
    rect(g, x0, y0, x1, y0 + 1, SLATE)
    put(g, (x0 + x1) // 2, (y0 + y1) // 2, GREY)
    x0, y0, x1, y1 = TABLE_PAD  # the pad, then its pen
    rect(g, x0, y0, x1 - 4, y1, OFFWHITE)
    rect(g, x1 - 4, y0 + 1, x1 - 3, y1, OFFWHITE_SH)
    for i in range(3):
        rect(g, x0 + 1, y0 + 2 + i * 2, x1 - 5 - (i % 2) * 2, y0 + 3 + i * 2, PRINT)
    rect(g, x1 - 2, y0 + 1, x1 - 1, y1 - 1, BLUE)
    desk_mug(g, TABLE_MUG[0], TABLE_MUG[1])
    union_outline(g)
    return g


def meeting_table_1x():
    """The meeting table at 1x: the lit far edge, the top, a notepad and a cup
    of coffee apart on it, the near edge in shade."""
    g = canvas(TABLE_W, TABLE_H)
    rect(g, 0, 0, TABLE_W, TABLE_H - 1, WOOD)
    rect(g, 0, 0, TABLE_W, 1, WOOD_LT)
    rect(g, 0, TABLE_H - 1, TABLE_W, TABLE_H, WOOD_SH)
    put(g, 3, 2, OFFWHITE)
    put(g, 8, 1, COFFEE)
    return g


# ---- the fixtures: the pantry's island and corner, the lounge, the meeting room, the wall -
# Recoloured from the theme (pixel_painter::palette::fixture_overrides).
TANK_WATER, TANK_WATER_DP, TANK_LINE = "Д", "д", "З"
TANK_FISH, TANK_FISH_SH, TANK_FISH_ALT, TANK_FISH_ALT_SH = "И", "и", "Л", "л"
TANK_PLANT, TANK_PLANT_SH = "Ь", "ь"
TRIM_DARK, TRIM_DARK_LT = "ж", "ν"
COOLER, COOLER_LT, COOLER_SH = "б", "в", "ё"
MAGAZINE, MAGAZINE_EDGE = "ы", "э"
COATS, COATS_SH = ("ч", "ш", "щ"), ("α", "β", "γ")
CLOCK_RIM, CLOCK_RIM_LT, CLOCK_FACE, CLOCK_FACE_SH, CLOCK_HAND = "ф", "η", "ц", "ε", "з"
# Un-themed: the cooler's bottle in the classic's own blue, the bin in the pack's nearest greys.
WATER, WATER_SH = "χ", "τ"
BIN, BIN_RIM, BIN_SH = "φ", "6", "7"
DOOR_FRAME, CHROME = "E", "K"


# `Furniture::KitchenIsland`'s visual box; its top two rows are the counter.
ISLAND_W, ISLAND_H = 20, 7
ISLAND_TOP_ROWS = 2


def kitchen_island():
    """The kitchen island: a lit wood counter overhanging its cabinets, a fruit
    bowl and a mug on it, four doors with their pulls, the toe-kick in shade."""
    w, h = ISLAND_W * S, ISLAND_H * S
    g = canvas(w, h)
    top = ISLAND_TOP_ROWS * S
    # the cabinets, a logical column in under the counter's overhang
    rect(g, S + 1, top, w - S - 1, h - 3, WOOD)
    rect(g, w - S - 4, top, w - S - 1, h - 3, WOOD_SH)  # the east face turned from the light
    rect(g, S + 1, top, w - S - 1, top + 1, WOOD_DK)  # the counter's shadow on the doors
    doors = 4
    span = (w - 2 * S - 2) // doors
    for i in range(doors):
        x0 = S + 1 + i * span
        rect(g, x0 + 2, top + 3, x0 + span - 2, h - 6, WOOD_SH)  # the recessed panel
        rect(g, x0 + 3, top + 4, x0 + span - 3, h - 7, WOOD)
        if i:
            rect(g, x0, top + 1, x0 + 1, h - 3, WOOD_DK)  # the seam between doors
        px = x0 + span - 4 if i % 2 == 0 else x0 + 3  # the pulls face the seam they share
        rect(g, px, top + 6, px + 1, top + 10, LAMP_HI)
    rect(g, S + 1, h - 5, w - S - 1, h - 3, SHADOW)  # the toe-kick
    # the counter: lit top, its far edge catching the light, its lip in shade
    rect(g, 1, 1, w - 1, top + 2, WOOD_LT)
    rect(g, 1, 1, w - 1, 2, WOOD_HI)
    rect(g, 1, top, w - 1, top + 2, WOOD)
    rect(g, 1, top + 1, w - 1, top + 2, WOOD_SH)
    for x, y in ((1, 1), (w - 2, 1), (1, top + 1), (w - 2, top + 1)):
        put(g, x, y, T)  # rounded corners
    rect(g, 9, 3, 21, 7, OFFWHITE)
    rect(g, 10, 7, 20, 8, OFFWHITE_SH)
    for x, y, k in ((10, 2, RED), (13, 2, GREEN), (16, 2, VERMILION), (12, 4, GOLD), (15, 4, RED)):
        rect(g, x, y, x + 3, y + 3, k)
        put(g, x, y, WHITE)
    # a mug, east, clear of the bowl
    desk_mug(g, w - 16, 3)
    union_outline(g)
    return g


def kitchen_island_1x():
    """The kitchen island at 1x: the counter lit along its far edge with the
    fruit and a mug on it, the cabinets under its overhang split into two doors
    with their pulls, the base in shade."""
    g = canvas(ISLAND_W, ISLAND_H)
    rect(g, 0, 0, ISLAND_W, 1, WOOD_HI)
    rect(g, 0, 1, ISLAND_W, ISLAND_TOP_ROWS, WOOD_LT)
    rect(g, 1, ISLAND_TOP_ROWS, ISLAND_W - 1, ISLAND_H - 1, WOOD)
    rect(g, 1, ISLAND_H - 1, ISLAND_W - 1, ISLAND_H, WOOD_SH)
    for x, y in ((0, 0), (ISLAND_W - 1, 0)):
        put(g, x, y, T)
    mid = ISLAND_W // 2
    rect(g, mid, ISLAND_TOP_ROWS, mid + 1, ISLAND_H - 1, WOOD_DK)
    put(g, mid - 2, 3, WOOD_DK)
    put(g, mid + 2, 3, WOOD_DK)
    put(g, 3, 0, RED)
    put(g, 4, 0, GOLD)
    put(g, ISLAND_W - 5, 0, MUG)
    return g


# `Furniture::LoungeSideTable`'s visual box.
SIDE_TABLE_W, SIDE_TABLE_H = 7, 4


def side_table():
    """The lounge side table: a lit wood top with a magazine lying on a second,
    its front edge in shade on two legs."""
    w, h = SIDE_TABLE_W * S, SIDE_TABLE_H * S
    g = canvas(w, h)
    rect(g, 1, 1, w - 1, 11, WOOD_LT)
    rect(g, 1, 1, w - 1, 2, WOOD_HI)
    rect(g, 1, 10, w - 1, 12, WOOD)
    rect(g, 1, 11, w - 1, 12, WOOD_SH)
    for x0 in (3, w - 6):
        rect(g, x0, 12, x0 + 3, h - 1, WOOD_DK)
        rect(g, x0, 12, x0 + 1, h - 1, WOOD_SH)
    # the magazines: the lower one askew, the upper with its title band
    rect(g, 10, 4, 22, 9, MAGAZINE_EDGE)
    rect(g, 7, 3, 19, 9, MAGAZINE)
    rect(g, 7, 8, 19, 9, MAGAZINE_EDGE)
    rect(g, 9, 4, 17, 5, WHITE)
    rect(g, 9, 6, 13, 7, OFFWHITE)
    union_outline(g)
    return g


def side_table_1x():
    """The side table at 1x: its lit top with a magazine on it, the front edge
    in shade."""
    g = canvas(SIDE_TABLE_W, SIDE_TABLE_H)
    rect(g, 0, 0, SIDE_TABLE_W, 1, WOOD_HI)
    rect(g, 0, 1, SIDE_TABLE_W, SIDE_TABLE_H - 1, WOOD_LT)
    rect(g, 0, SIDE_TABLE_H - 1, SIDE_TABLE_W, SIDE_TABLE_H, WOOD_SH)
    cx, cy = SIDE_TABLE_W // 2, SIDE_TABLE_H // 2
    rect(g, cx - 1, cy - 1, cx + 2, cy, MAGAZINE)
    rect(g, cx - 1, cy, cx + 2, cy + 1, MAGAZINE_EDGE)
    return g


# `PantryRoom::water_cooler_rect`'s size.
COOLER_W, COOLER_H = 3, 6
# The glug: a bubble climbs the bottle, then the water stills.
GLUG_STEPS = 5


def water_cooler():
    """A water cooler: the upturned bottle lit down its west side over the
    cabinet, its hot and cold taps over the drip tray; a bubble glugs up the
    bottle, then the water stills."""
    def body(bubble):
        w, h = COOLER_W * S, COOLER_H * S
        g = canvas(w, h)
        rect(g, 2, 1, w - 2, 10, WATER)  # the bottle, upside down
        put(g, 2, 1, T)
        put(g, w - 3, 1, T)
        rect(g, w - 4, 2, w - 2, 10, WATER_SH)
        rect(g, 3, 3, 4, 8, CYAN)  # its glint
        rect(g, 4, 10, w - 4, 12, WATER_SH)  # the neck, into the cabinet
        rect(g, 1, 11, w - 1, h - 1, COOLER)
        rect(g, 1, 11, w - 1, 12, COOLER_LT)
        rect(g, w - 3, 12, w - 1, h - 1, COOLER_SH)
        rect(g, 3, 14, w - 3, 19, SHADOW)  # the dispensing alcove
        put(g, 4, 14, RED)
        put(g, 4, 15, RED)
        put(g, w - 5, 14, BLUE)
        put(g, w - 5, 15, BLUE)
        rect(g, 3, 18, w - 3, 19, GREY)  # the drip tray
        if bubble is not None:
            bx, by = bubble
            rect(g, bx, by, bx + 2, by + 2, TANK_LINE)
        union_outline(g)
        return g
    rises = [(5, 7), (5, 3)]
    return [body(rises[i] if i < len(rises) else None) for i in range(GLUG_STEPS)]


def water_cooler_1x():
    """The water cooler at 1x: the bottle, glinting west, over the cabinet shaded
    east; a bubble glugs up the bottle, then the water stills."""
    def body(bubble_y):
        g = canvas(COOLER_W, COOLER_H)
        rect(g, 0, 0, COOLER_W, 2, WATER)
        put(g, 0, 0, CYAN)
        rect(g, 0, 2, COOLER_W, COOLER_H, COOLER)
        rect(g, COOLER_W - 1, 2, COOLER_W, COOLER_H, COOLER_SH)
        if bubble_y is not None:
            put(g, 1, bubble_y, TANK_LINE)
        return g
    rises = [1, 0]
    return [body(rises[i] if i < len(rises) else None) for i in range(GLUG_STEPS)]


# `PantryRoom::trash_bin_rect`'s size.
BIN_W, BIN_H = 4, 5


def trash_bin():
    """The pantry's trash bin: its rim lit, the bag's liner folded over it with a crumpled
    sheet on top, the ribbed body shaded east."""
    w, h = BIN_W * S, BIN_H * S
    g = canvas(w, h)
    rect(g, 2, 7, w - 2, h - 1, BIN)  # the body
    rect(g, 3, 15, w - 3, h - 1, BIN)
    rect(g, 2, 15, 3, h - 1, T)
    rect(g, w - 3, 15, w - 2, h - 1, T)
    rect(g, 2, 7, 4, 15, BIN_RIM)
    rect(g, w - 4, 7, w - 2, 15, BIN_SH)
    for x in range(5, w - 4, 3):
        rect(g, x, 8, x + 1, h - 2, BIN_SH)  # ribs
    rect(g, 1, 3, w - 1, 7, BIN_RIM)  # the rim
    rect(g, 3, 4, w - 3, 7, OFFWHITE)  # the liner inside it
    rect(g, 3, 6, w - 3, 7, OFFWHITE_SH)
    rect(g, 7, 1, 11, 5, OFFWHITE)  # a crumpled sheet
    put(g, 9, 2, OFFWHITE_SH)
    put(g, 8, 3, OFFWHITE_SH)
    put(g, 10, 4, OFFWHITE_SH)
    union_outline(g)
    return g


def trash_bin_1x():
    """The trash bin at 1x: the rim round the liner, the bag's fill, the body
    shaded east."""
    g = canvas(BIN_W, BIN_H)
    rect(g, 0, 0, BIN_W, 1, BIN_RIM)
    rect(g, 1, 0, BIN_W - 1, 1, OFFWHITE)
    rect(g, 0, 1, BIN_W, BIN_H, BIN)
    rect(g, 1, 1, BIN_W - 1, 2, OFFWHITE_SH)
    rect(g, BIN_W - 1, 2, BIN_W, BIN_H, BIN_SH)
    return g


# `Furniture::FishTank`'s visual box.
TANK_W, TANK_H = 14, 11
TANK_WATER_ROWS = (1, 8)  # half-open: the surface line, then water, the gravel last
TANK_CABINET_Y = 9
# The fish patrol: each swims its lane to the far wall and back, one column a step,
# the second TANK_FISH_LAG steps behind the first so the pair never mirror.
TANK_LANES = (3, 5)
TANK_SPAN = TANK_W - 5
TANK_STEPS = 2 * TANK_SPAN
TANK_FISH_LAG = 7
TANK_FISH_LEN = 3
# A bubble climbs from the gravel to the surface, then the next one starts.
TANK_BUBBLE_X = TANK_W - 3
TANK_BUBBLE_ROWS = (6, 5, 4, 3, 2, None)
TANK_PLANT_AT = ((2, 5), (2, 6), (2, 7), (3, 6))
assert TANK_STEPS % len(TANK_BUBBLE_ROWS) == 0


def tank_fish_at(step):
    """Where a fish starts its lane and which way it swims, `step` of its lap."""
    step %= TANK_STEPS
    if step < TANK_SPAN:
        return 1 + step, 1
    return 1 + TANK_STEPS - step, -1


def fish_tank():
    """An aquarium on its cabinet: two fish patrolling opposite lanes past the
    swaying weed, a bubble rising from the gravel, the surface rippling, the
    glass catching the light."""
    w, h = TANK_W * S, TANK_H * S
    top, bottom = TANK_WATER_ROWS[0] * S, TANK_WATER_ROWS[1] * S
    frames = []
    for step in range(TANK_STEPS):
        g = canvas(w, h)
        rect(g, 1, 1, w - 1, bottom + S, TRIM_DARK)  # the frame
        rect(g, 1, 1, w - 1, 2, TRIM_DARK_LT)
        rect(g, 4, top, w - 4, bottom - S, TANK_WATER)
        for y in range(top + 2 * S, bottom - S):  # the deep water, dithered in
            for x in range(4, w - 4):
                deep = y - (top + 2 * S) >= 2 * S or (x + y) % 2 == 0 and y - (top + 2 * S) >= S
                if deep:
                    put(g, x, y, TANK_WATER_DP)
        for x in range(4, w - 4):  # the surface, rippling
            put(g, x, top, TANK_LINE)
            if (x + step) % 6 < 2:
                put(g, x, top + 1, TANK_LINE)
        for x in range(4, w - 4):  # the gravel bed
            for y in range(bottom - S, bottom):
                put(g, x, y, (TAN, BROWN, GOLD, BROWN)[(x * 3 + y * 5) % 4])
        # the weed, its tips swaying with the water
        sway = (0, 1, 1, 0, -1, -1)[step % 6]
        for x0, y0, tall in ((7, bottom - S, 16), (w - 12, bottom - S, 10)):
            for i in range(tall):
                dx = sway if i > tall // 2 else 0
                put(g, x0 + dx, y0 - i, TANK_PLANT)
                put(g, x0 + dx + 1, y0 - i, TANK_PLANT_SH)
                if i % 4 == 2:
                    put(g, x0 + dx - 1, y0 - i, TANK_PLANT)
        for lane, lag, body, shade in ((TANK_LANES[0], 0, TANK_FISH, TANK_FISH_SH),
                                       (TANK_LANES[1], TANK_FISH_LAG, TANK_FISH_ALT, TANK_FISH_ALT_SH)):
            start, heading = tank_fish_at(step + lag)
            x0, y0 = start * S, lane * S
            fl = TANK_FISH_LEN * S - 2
            rect(g, x0 + 1, y0, x0 + fl, y0 + 3, body)
            rect(g, x0 + 2, y0 - 1, x0 + fl - 2, y0, body)
            rect(g, x0 + 1, y0 + 2, x0 + fl, y0 + 3, shade)
            tail = x0 if heading > 0 else x0 + fl
            rect(g, tail, y0 - 1, tail + 1, y0 + 4, shade)  # the tail fin, flicking
            if step % 2:
                put(g, tail, y0 - 1, TANK_WATER)
            head = x0 + fl - 2 if heading > 0 else x0 + 2
            put(g, head, y0, KEY_DK)
        row = TANK_BUBBLE_ROWS[step % len(TANK_BUBBLE_ROWS)]
        if row is not None:
            bx, by = TANK_BUBBLE_X * S, row * S + 1
            rect(g, bx, by, bx + 2, by + 2, TANK_LINE)
        for x, y in ((6, top + 3), (5, top + 4), (5, top + 5)):
            put(g, x, y, WHITE)  # the glass's glint
        rect(g, 1, TANK_CABINET_Y * S, w - 1, h - 1, WOOD)
        rect(g, 1, TANK_CABINET_Y * S, w - 1, TANK_CABINET_Y * S + 1, WOOD_HI)
        rect(g, 1, h - 3, w - 1, h - 1, WOOD_SH)
        mid = w // 2
        rect(g, mid, TANK_CABINET_Y * S + 1, mid + 1, h - 1, WOOD_DK)
        put(g, mid - 3, TANK_CABINET_Y * S + 3, LAMP_HI)
        put(g, mid + 3, TANK_CABINET_Y * S + 3, LAMP_HI)
        union_outline(g)
        frames.append(g)
    return frames


def fish_tank_1x():
    """The aquarium at 1x: the frame lit along its top, the surface line, two
    fish patrolling their lanes, a bubble rising, a weed sprig on the gravel,
    the cabinet under it."""
    frames = []
    for step in range(TANK_STEPS):
        g = canvas(TANK_W, TANK_H)
        rect(g, 0, 0, TANK_W, TANK_WATER_ROWS[1] + 1, TRIM_DARK)
        rect(g, 0, 0, TANK_W, 1, TRIM_DARK_LT)
        rect(g, 1, TANK_WATER_ROWS[0], TANK_W - 1, TANK_WATER_ROWS[1] - 1, TANK_WATER)
        rect(g, 1, TANK_WATER_ROWS[0], TANK_W - 1, TANK_WATER_ROWS[0] + 1, TANK_LINE)
        for x in range(1, TANK_W - 1):
            put(g, x, TANK_WATER_ROWS[1] - 1, BROWN if x % 2 == 0 else TAN)
        for lane, lag, body in ((TANK_LANES[0], 0, TANK_FISH), (TANK_LANES[1], TANK_FISH_LAG, TANK_FISH_ALT)):
            start, _ = tank_fish_at(step + lag)
            rect(g, start, lane, start + TANK_FISH_LEN, lane + 1, body)
        row = TANK_BUBBLE_ROWS[step % len(TANK_BUBBLE_ROWS)]
        if row is not None:
            put(g, TANK_BUBBLE_X, row, TANK_LINE)
        for x, y in TANK_PLANT_AT:  # last, so the fish swim behind it
            put(g, x, y, TANK_PLANT)
        rect(g, 0, TANK_CABINET_Y, TANK_W, TANK_H - 1, WOOD_LT)
        put(g, TANK_W // 2, TANK_CABINET_Y, WOOD_DK)
        rect(g, 0, TANK_H - 1, TANK_W, TANK_H, WOOD_SH)
        frames.append(g)
    return frames


# `coat_rack_rect_at`'s box around the pole, and where its coats hang: the
# layout's `COAT_HOOK_DX`, `COAT_W`, `COAT_RACK_BASE_DY`.
COAT_HOOK_DX, COAT_W, COAT_RACK_BASE_DY = 1, 2, 7
COAT_REACH = COAT_HOOK_DX + COAT_W - 1
RACK_W, RACK_H = 2 * COAT_REACH + 1, COAT_RACK_BASE_DY + 1


def coat_hooks():
    """Each coat's (x0, y0) in logical units: alternating sides down the pole."""
    return [(0 if i % 2 == 0 else COAT_REACH + COAT_HOOK_DX, 1 + i * 2) for i in range(len(COATS))]


def coat_rack():
    """A coat rack: a turned wood pole with its knob and foot, three coats hung
    on alternating pegs, each hanging from its shoulders and flaring to its hem,
    its collar turned down, its front edge closed, the side away from the light
    in shade."""
    w, h = RACK_W * S, RACK_H * S
    g = canvas(w, h)
    mid = COAT_REACH * S + S // 2
    rect(g, mid - 1, 3, mid + 1, h - 3, WOOD)  # the pole
    rect(g, mid - 1, 3, mid, h - 3, WOOD_LT)
    rect(g, mid - 2, 1, mid + 2, 4, WOOD_LT)  # its knob
    put(g, mid - 2, 1, WOOD_HI)
    rect(g, 3, h - 4, w - 3, h - 2, WOOD)  # its foot
    rect(g, 3, h - 4, w - 3, h - 3, WOOD_LT)
    for (lx, ly), coat, shade in zip(coat_hooks(), COATS, COATS_SH):
        west = lx == 0
        x0, y0 = lx * S, ly * S
        peg = x0 + COAT_W * S - 1 if west else x0
        rect(g, min(peg, mid), y0, max(peg, mid) + 1, y0 + 1, WOOD_DK)  # the peg
        # Hung from the peg's end: narrow at the shoulders, flaring to the hem.
        top, hem = y0 + 1, y0 + 2 * S + 1
        cx = peg - 1 if west else peg + 1
        for y in range(top, hem):
            half = 1 + (y - top) * 3 // (hem - top)
            rect(g, cx - half, y, cx + half + 1, y + 1, coat)
            put(g, cx + half if west else cx - half, y, shade)  # away from the light
        rect(g, cx - 1, top, cx + 2, top + 1, WHITE if coat != COATS[2] else OFFWHITE_SH)  # the collar
        for y in range(top + 1, hem - 1):
            put(g, cx, y, shade)  # the front edge
        half = 1 + (hem - 1 - top) * 3 // (hem - top)
        rect(g, cx - half, hem - 1, cx + half + 1, hem, shade)  # the hem
    union_outline(g)
    return g


def coat_rack_1x():
    """The coat rack at 1x: the pole with its knob and foot, three coats on
    alternating hooks, each lit at its shoulders and shaded at its hem."""
    g = canvas(RACK_W, RACK_H)
    rect(g, COAT_REACH, 0, COAT_REACH + 1, RACK_H, WOOD_SH)
    put(g, COAT_REACH, 0, WOOD_LT)
    rect(g, COAT_REACH - 1, RACK_H - 1, COAT_REACH + 2, RACK_H, WOOD)
    for (x0, y0), coat, shade in zip(coat_hooks(), COATS, COATS_SH):
        rect(g, x0, y0, x0 + COAT_W, y0 + 1, coat)
        rect(g, x0, y0 + 1, x0 + COAT_W, y0 + 2, shade)
    return g


# `MeetingRoom::notice_board_rect`'s size.
BOARD_W, BOARD_H = 8, 5


def notice_board():
    """A meeting room's notice board: cork in a wood frame lit along its top, a
    sticky note, an agenda and a card pinned to it."""
    w, h = BOARD_W * S, BOARD_H * S
    g = canvas(w, h)
    rect(g, 1, 1, w - 1, h - 1, WOOD)
    rect(g, 1, 1, w - 1, 2, WOOD_LT)
    rect(g, 1, h - 2, w - 1, h - 1, WOOD_SH)
    rect(g, 3, 3, w - 3, h - 3, CORK)
    for x, y in ((5, 14), (12, 4), (19, 15), (26, 6), (9, 9)):
        put(g, x, y, CORK_SH)
    rect(g, 4, 5, 9, 10, PINK)  # a sticky note
    rect(g, 5, 7, 8, 8, PRINT)
    rect(g, 12, 6, 19, 15, OFFWHITE)  # the agenda
    for i in range(3):
        rect(g, 13, 8 + i * 2, 18 - (i % 2) * 2, 9 + i * 2, PRINT)
    rect(g, 22, 5, 27, 10, GOLD)  # a card
    rect(g, 23, 7, 26, 8, PRINT)
    for x, y in ((6, 5), (15, 6), (24, 5)):
        put(g, x, y, RED)  # the pins
    union_outline(g)
    return g


def notice_board_1x():
    """The notice board at 1x: cork in a wood frame, a sticky note and a sheet
    pinned to it."""
    g = canvas(BOARD_W, BOARD_H)
    rect(g, 0, 0, BOARD_W, BOARD_H, WOOD)
    rect(g, 0, 0, BOARD_W, 1, WOOD_LT)
    rect(g, 1, 1, BOARD_W - 1, BOARD_H - 1, CORK)
    put(g, 2, 1, PINK)
    rect(g, 4, 2, 6, 3, OFFWHITE)
    return g


# `layout::CLOCK`. The dial only: the painter draws the hands from the time, which
# as art would be 720 frames.
CLOCK_W, CLOCK_H = 7, 7
# Art pixels from the dial's edge to its face: the outline and the rim.
CLOCK_RIM_PX = 3
CLOCK_1X = ("..RRR..", ".RFFFR.", "RFFFFFR", "RFFFFFR", "RFFFFFR", ".RFFFR.", "..RRR..")


def wall_clock():
    """The wall clock's dial: the rim lit on its upper west, the face shaded
    round its lower east, the hour ticks and the centre pin the hands turn on."""
    w, h = CLOCK_W * S, CLOCK_H * S
    g = canvas(w, h)
    c = (w / 2, h / 2)
    r_out, r_face = w / 2 - 1, w / 2 - CLOCK_RIM_PX
    for y in range(h):
        for x in range(w):
            dx, dy = x + 0.5 - c[0], y + 0.5 - c[1]
            d = math.hypot(dx, dy)
            if d > r_out:
                continue
            if d > r_face:
                put(g, x, y, CLOCK_RIM_LT if dx + dy < -r_face * 0.6 else CLOCK_RIM)
            elif d > r_face - 1.5 and dx + dy > r_face * 0.8:
                put(g, x, y, CLOCK_FACE_SH)
            else:
                put(g, x, y, CLOCK_FACE)
    for i in range(12):
        a = i * math.pi / 6
        major = i % 3 == 0
        for rr in ((r_face - 1.5, r_face - 3.5) if major else (r_face - 1.5,)):
            put(g, int(c[0] + rr * math.sin(a)), int(c[1] - rr * math.cos(a)), CLOCK_HAND)
    rect(g, int(c[0]) - 1, int(c[1]) - 1, int(c[0]) + 1, int(c[1]) + 1, CLOCK_HAND)
    union_outline(g)
    return g


def wall_clock_1x():
    """The wall clock's dial at 1x: the rim lit along its top, the face, the
    centre pin the hands turn on."""
    g = canvas(CLOCK_W, CLOCK_H)
    for y, row in enumerate(CLOCK_1X):
        for x, ch in enumerate(row):
            if ch != ".":
                put(g, x, y, CLOCK_RIM if ch == "R" else CLOCK_FACE)
    for x in range(2, 5):
        put(g, x, 0, CLOCK_RIM_LT)
    put(g, CLOCK_W // 2, CLOCK_H // 2, CLOCK_HAND)
    return g


# `Furniture::MeetingChair`'s visual box, drawn for a sitter facing east.
CHAIR_W, CHAIR_H = 7, 7
CHAIR_SEAT_ROWS = (1, 5)  # half-open
CHAIR_LEG_COLS = (1, 5)


def meeting_chair():
    """A head-of-table meeting chair in the sofas' fabric: its back to the
    west, the seat cushion lit along its far edge and shaded at its front, on
    four wood legs."""
    w, h = CHAIR_W * S, CHAIR_H * S
    g = canvas(w, h)
    y0, y1 = CHAIR_SEAT_ROWS[0] * S, CHAIR_SEAT_ROWS[1] * S
    for lx in CHAIR_LEG_COLS:
        rect(g, lx * S, y1, lx * S + 3, h - 1, WOOD_DK)
        rect(g, lx * S, y1, lx * S + 1, h - 1, WOOD_SH)
    rect(g, S, y0, w - 2, y1, FABRIC)  # the seat
    rect(g, S, y0, w - 2, y0 + 2, FABRIC_HI)
    rect(g, S, y1 - 3, w - 2, y1, FABRIC_SH)
    rect(g, S + 2, y0 + 3, w - 4, y1 - 4, FABRIC)  # its cushion's welt
    rect(g, S + 1, y0 + 2, S + 2, y1 - 3, FABRIC_SEAM)
    rect(g, 1, 1, S, y1, FABRIC)  # the back
    rect(g, 1, 1, S, 3, FABRIC_HI)
    rect(g, 1, 1, 2, y1, FABRIC_HI)
    rect(g, S - 1, 3, S, y1, FABRIC_SEAM)
    union_outline(g)
    return g


def meeting_chair_1x():
    """The meeting chair at 1x: its back to the west in the seat's fabric, the
    seat lit along its far edge, two legs showing."""
    g = canvas(CHAIR_W, CHAIR_H)
    rect(g, 0, 0, 1, CHAIR_SEAT_ROWS[1], FABRIC_SH)
    rect(g, 1, CHAIR_SEAT_ROWS[0], CHAIR_W - 1, CHAIR_SEAT_ROWS[1], FABRIC)
    rect(g, 1, CHAIR_SEAT_ROWS[0], CHAIR_W - 1, CHAIR_SEAT_ROWS[0] + 1, FABRIC_HI)
    for lx in CHAIR_LEG_COLS:
        rect(g, lx, CHAIR_SEAT_ROWS[1], lx + 1, CHAIR_H, WOOD_DK)
    return g


# The 1x `door.sprite`'s size, its panels' rows, and each frame's gap.
DOOR_W, DOOR_H = 16, 14
DOOR_PANEL_ROWS = (3, 13)  # half-open
DOOR_GAPS = (0, 4, 8)  # shut, half-open, open: the shaft columns showing


def elevator_door():
    """The elevator: a brushed-steel frame lit along its top and west, the
    floor display, and the chrome doors: shut, parting, open on a car lit
    from its ceiling, its handrail and floor."""
    w, h = DOOR_W * S, DOOR_H * S
    frames = []
    for gap in DOOR_GAPS:
        g = canvas(w, h)
        rect(g, 1, 1, w - 1, h - 1, DOOR_FRAME)
        rect(g, 1, 1, w - 1, 2, ALU)
        rect(g, 1, 1, 2, h - 1, ALU)
        rect(g, 4, 4, w - 4, 8, CYAN)  # the display
        rect(g, 4, 4, w - 4, 5, WHITE)
        for x in range(8, 13):  # the floor arrow
            put(g, x, 6 - min(x - 8, 12 - x) // 2, GOLD)
        rect(g, 16, 6, 18, 7, GOLD)
        rect(g, 20, 6, 22, 7, GOLD)
        p0, p1 = DOOR_PANEL_ROWS[0] * S, DOOR_PANEL_ROWS[1] * S
        x0, x1 = S, w - S
        mid = w // 2
        half = gap * S // 2
        if gap:  # the car, lit from its ceiling
            rect(g, mid - half, p0, mid + half, p1, LAMP)
            rect(g, mid - half, p0, mid + half, p0 + 2, BULB)
            rect(g, mid - half, p0 + 2, mid + half, p0 + 4, LAMP_HI)
            rect(g, mid - half, p0 + 22, mid + half, p0 + 23, GREY)  # the handrail
            rect(g, mid - half, p1 - 4, mid + half, p1, SLATE)
        for a, b in ((x0, mid - half), (mid + half, x1)):
            if b - a <= 0:
                continue
            rect(g, a, p0, b, p1, CHROME)
            rect(g, a, p0, a + 1, p1, WHITE)  # the lit edge of each leaf
            rect(g, b - 2, p0, b, p1, STEEL_SH)
            for x in range(a + 4, b - 3, 5):
                rect(g, x, p0 + 3, x + 1, p1 - 8, STEEL_SH)  # the brushing
            rect(g, a, p1 - 6, b, p1, STEEL_SH)  # the kick plate
        if not gap:
            rect(g, mid - 1, p0, mid + 1, p1, SHADOW)  # the doors' meeting line
        rect(g, 1, p1, w - 1, p1 + 1, ALU)  # the sill
        union_outline(g)
        frames.append(g)
    return frames


# ---- the city behind the windows: its buildings ------------------------------------
# Drawn only in the [city] materials: a painter colours each by its depth and the
# sky, so no drawing here carries a colour of its own. Each is drawn at `S`; its base
# is that drawing read at 1x, block by block (`city_base`).
CITY_FACADE, CITY_SHADE, CITY_ROOF, CITY_GLASS, CITY_MULLION, CITY_DETAIL, CITY_SIGN = (
    "+", "-", "~", "0", "|", "*", "=")
# pack.toml's `[city]` names these keys; `--check` fails where the two differ.
CITY = {
    "facade": CITY_FACADE, "shade": CITY_SHADE, "roof": CITY_ROOF, "glass": CITY_GLASS,
    "mullion": CITY_MULLION, "detail": CITY_DETAIL, "sign": CITY_SIGN,
}


def panes(g, x0, y0, x1, y1, w, h, px, py):
    """A grid of `w`x`h` windows at pitch `px`,`py` inside the rect."""
    for y in range(y0, y1 - h + 1, py):
        for x in range(x0, x1 - w + 1, px):
            rect(g, x, y, x + w, y + h, CITY_GLASS)


def shade_east(g):
    """The east third of each mass on each row turned from the light."""
    for row in g:
        x = 0
        while x < len(row):
            if row[x] == T:
                x += 1
                continue
            end = x
            while end < len(row) and row[end] != T:
                end += 1
            for i in range(x + (end - x) * 2 // 3, end):
                if row[i] == CITY_FACADE:
                    row[i] = CITY_SHADE
            x = end


def building_setback():
    """A slim tower stepping in twice toward a mast."""
    w, h = 6 * S, 16 * S
    g = canvas(w, h)
    for x0, y0 in ((0, 7 * S), (S, 4 * S), (2 * S, 2 * S)):
        rect(g, x0, y0, w - x0, h, CITY_FACADE)
        rect(g, x0, y0, w - x0, y0 + 1, CITY_ROOF)
    rect(g, w // 2 - 1, 0, w // 2 + 1, 2 * S, CITY_DETAIL)
    panes(g, 2, 7 * S + 2, w - 2, h - 2, 2, 2, 3, 4)
    panes(g, S + 2, 4 * S + 2, w - S - 2, 7 * S - 1, 2, 2, 3, 4)
    panes(g, 2 * S + 2, 2 * S + 2, w - 2 * S - 2, 4 * S - 1, 2, 2, 3, 4)
    shade_east(g)
    return g


def building_curtain():
    """A glass curtain wall: bands of glass between thin spandrels."""
    w, h = 6 * S, 15 * S
    g = canvas(w, h)
    rect(g, 0, S, w, h, CITY_FACADE)
    rect(g, 0, S, w, S + 2, CITY_ROOF)
    for y in range(S + 4, h - 1, 3):
        rect(g, 1, y, w - 1, y + 2, CITY_GLASS)
        for x in range(1, w - 1, 4):
            rect(g, x, y, x + 1, y + 2, CITY_MULLION)
    shade_east(g)
    return g


def building_walkup():
    """An old walk-up, its water tank on legs on the roof."""
    w, h = 5 * S, 10 * S
    g = canvas(w, h)
    rect(g, 0, 3 * S, w, h, CITY_FACADE)
    rect(g, 0, 3 * S, w, 3 * S + 2, CITY_ROOF)
    rect(g, 11, 2, 18, 8, CITY_DETAIL)
    rect(g, 11, 1, 18, 2, CITY_ROOF)
    for x in (12, 16):
        rect(g, x, 8, x + 1, 3 * S, CITY_DETAIL)
    panes(g, 2, 3 * S + 4, w - 2, h - 2, 3, 3, 5, 5)
    shade_east(g)
    return g


def building_dome():
    """A civic block under a dome, tall windows between its piers."""
    w, h = 7 * S, 11 * S
    g = canvas(w, h)
    base = 5 * S
    rect(g, 0, base, w, h, CITY_FACADE)
    rect(g, 0, base, w, base + 2, CITY_ROOF)
    for y in range(2 * S, base):
        for x in range(w):
            if (x + 0.5 - w / 2) ** 2 + ((y + 0.5 - base) * 1.1) ** 2 <= 9.5 ** 2:
                g[y][x] = CITY_ROOF if y < base - 7 else CITY_FACADE
    rect(g, w // 2 - 1, S + 2, w // 2 + 1, 2 * S + 3, CITY_DETAIL)
    for x in range(3, w - 3, 5):
        rect(g, x, base + 4, x + 2, h - 3, CITY_GLASS)
    shade_east(g)
    return g


def building_sign():
    """A wide mid-rise carrying a sign on its roof, lit after dark."""
    w, h = 7 * S, 9 * S
    g = canvas(w, h)
    rect(g, 0, 3 * S, w, h, CITY_FACADE)
    rect(g, 0, 3 * S, w, 3 * S + 2, CITY_ROOF)
    rect(g, S, S, w - S, 2 * S + 2, CITY_SIGN)
    rect(g, S, S, w - S, S + 1, CITY_MULLION)
    for x in (S + 3, w - S - 4):
        rect(g, x, 2 * S + 2, x + 1, 3 * S, CITY_DETAIL)
    panes(g, 2, 3 * S + 4, w - 2, h - 2, 4, 2, 6, 4)
    shade_east(g)
    return g


def building_spire():
    """A tower crowned by a stepped spire and a mast."""
    w, h = 4 * S, 18 * S
    g = canvas(w, h)
    rect(g, 0, 8 * S, w, h, CITY_FACADE)
    for i, y in enumerate(range(7 * S, 3 * S, -S)):  # tiers narrowing upward
        rect(g, 1 + i, y, w - 1 - i, y + S, CITY_FACADE)
        rect(g, 1 + i, y, w - 1 - i, y + 1, CITY_ROOF)
    rect(g, w // 2 - 1, 0, w // 2 + 1, 4 * S, CITY_DETAIL)  # down onto the top tier
    panes(g, 2, 8 * S + 3, w - 2, h - 2, 1, 3, 3, 5)
    shade_east(g)
    return g


def building_low():
    """A low block, its roof crowded with plant."""
    w, h = 6 * S, 6 * S
    g = canvas(w, h)
    rect(g, 0, 2 * S, w, h, CITY_FACADE)
    rect(g, 0, 2 * S, w, 2 * S + 2, CITY_ROOF)
    for x0, y0, x1 in ((3, 3, 8), (12, 4, 15), (18, 2, 20)):
        rect(g, x0, y0, x1, 2 * S, CITY_DETAIL)
    panes(g, 2, 2 * S + 4, w - 2, h - 2, 3, 2, 5, 4)
    shade_east(g)
    return g


def building_twin():
    """Twin towers joined by a skybridge, each crowned by a short mast."""
    w, h = 6 * S, 17 * S
    g = canvas(w, h)
    for x0 in (0, 4 * S):
        rect(g, x0, 2 * S, x0 + 2 * S, h, CITY_FACADE)
        rect(g, x0, 2 * S, x0 + 2 * S, 2 * S + 2, CITY_ROOF)
        rect(g, x0 + S - 1, 0, x0 + S + 1, 2 * S, CITY_DETAIL)
        panes(g, x0 + 1, 2 * S + 4, x0 + 2 * S - 1, h - 2, 2, 2, 3, 4)
    rect(g, 2 * S, 8 * S, 4 * S, 9 * S, CITY_FACADE)  # the skybridge
    rect(g, 2 * S, 8 * S + 1, 4 * S, 9 * S - 1, CITY_GLASS)
    shade_east(g)
    return g


def building_slab():
    """A wide slab tower in bands of glass, a lit crown along its top."""
    w, h = 8 * S, 15 * S
    g = canvas(w, h)
    rect(g, 0, S, w, h, CITY_FACADE)
    rect(g, 0, S, w, S + 2, CITY_ROOF)
    rect(g, 2, S + 3, w - 2, S + 5, CITY_SIGN)  # the crown's light strip
    for y in range(3 * S, h - 1, 4):
        rect(g, 1, y, w - 1, y + 2, CITY_GLASS)
        for x in range(5, w - 1, 6):
            rect(g, x, y, x + 1, y + 2, CITY_MULLION)
    shade_east(g)
    return g


def city_base(g):
    """A building drawn at `S`, read at 1x, `S`x`S` block by block: a thin part
    running through a block (a sign, then plant or a mast, then a roof line) wins it; a block
    less than half drawn is sky; one holding glass on an odd row and column is glass,
    the lit-dot grid a 1x city is read by (the scene's `skyline::block_window`);
    any other takes whichever of facade and shade it holds more of."""
    h, w = len(g) // S, len(g[0]) // S
    out = canvas(w, h)
    for by in range(h):
        for bx in range(w):
            cells = [g[by * S + y][bx * S + x] for y in range(S) for x in range(S)]
            thin = next((k for k in (CITY_SIGN, CITY_DETAIL, CITY_ROOF) if cells.count(k) >= S), None)
            if thin:
                out[by][bx] = thin
            elif (S * S - cells.count(T)) * 2 < S * S:
                continue
            elif CITY_GLASS in cells and bx % 2 == 1 and by % 2 == 1:
                out[by][bx] = CITY_GLASS
            else:
                out[by][bx] = max((CITY_FACADE, CITY_SHADE), key=cells.count)
    return out


# Each building, registered in pack.toml's `[buildings]` by hand with the planes it
# stands in (`every_embedded_sprite_is_a_frame_the_pack_loads` fails on one left out).
BUILDINGS = {
    "setback": building_setback,
    "curtain": building_curtain,
    "spire": building_spire,
    "twin": building_twin,
    "slab": building_slab,
    "walkup": building_walkup,
    "dome": building_dome,
    "sign": building_sign,
    "low": building_low,
}


# ---- output -----------------------------------------------------------------
PROVENANCE = "Generated by scripts/gen-art.py: edit the generator, not this file."
ENCODING = "utf-8"  # the keys include σ/ψ/Θ, and the locale's encoding need not be the file's


def render_sprite(header, frames, heads=()):
    """A .sprite file: the header as comments, then each frame, marked with its
    `heads` entry `(view, x, y)` where it has one."""
    text = inspect.cleandoc(header) + "\n" + PROVENANCE
    lines = [f"# {l}" if l else "#" for l in text.split("\n")]
    body = []
    for i, g in enumerate(frames):
        body.append(f"@frame {i}")
        if i < len(heads) and heads[i] is not None:
            view, x, y = heads[i]
            body.append(f"@mark head.{view} {x} {y}")
        body.extend(" ".join(r) for r in g)
    return "\n".join(lines + body) + "\n"


def hair_file(style, view, part):
    """The sprite a hairstyle's layer is written to, named as `[hairstyles]` lists it."""
    return f"hair_{style}_{view}_{part}@{S}x.sprite"


def orphans(pack, sprites):
    """The generated sprites on disk that `sprites` no longer draws."""
    return sorted(
        p.name
        for p in pack.glob("*.sprite")
        if p.name not in sprites and f"# {PROVENANCE}" in p.read_text(encoding=ENCODING).splitlines()
    )


def manifest_drift(pack):
    """Why pack.toml names other keys than the ones drawn in, or None."""
    manifest = pack / "pack.toml"
    if not manifest.is_file():
        return None
    toml = tomllib.loads(manifest.read_text(encoding=ENCODING))
    named = toml.get("city")
    if named is not None and named != CITY:
        return f"gen-art --check: pack.toml [city] {named} is not the keys drawn in, {CITY}"
    outline = toml.get("characters", {}).get("outline")
    if outline is not None and outline != SILHOUETTE:
        return f"gen-art --check: pack.toml [characters] outline {outline!r} is not the art's, {SILHOUETTE!r}"
    return None


def check(pack, sprites):
    """Why the committed pack is not what `sprites` draws, or None when it is."""
    drift = manifest_drift(pack)
    if drift:
        return drift
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
        (pack / "pack.toml").write_text('[characters]\noutline = "n"\n', encoding=ENCODING)
        assert check(pack, sprites) is not None, "an outline the art does not draw must fail"
        (pack / "pack.toml").write_text(f'[characters]\noutline = "{SILHOUETTE}"\n', encoding=ENCODING)
        assert check(pack, sprites) is None, "the art's own outline passes"
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
        "plant": (plant_bush.__doc__, [plant_bush()]),
        "plant_tall": (plant_tall.__doc__, [plant_tall()]),
        "plant_flower": (plant_flower.__doc__, [plant_flower()]),
        "plant_succulent": (plant_succulent.__doc__, [plant_succulent()]),
        "floor_lamp": (floor_lamp.__doc__, [floor_lamp()]),
        "filing_cabinet": (filing_cabinet.__doc__, [filing_cabinet()]),
        "bulletin_board": (bulletin_board.__doc__, [bulletin_board()]),
        "exit_sign": (exit_sign.__doc__, [exit_sign()]),
        "whiteboard": (whiteboard.__doc__, [whiteboard()]),
        "bookshelf": (bookshelf.__doc__, [bookshelf()]),
        "meeting_screen": (meeting_screen.__doc__, [meeting_screen()]),
        "pantry": (pantry.__doc__, [pantry()]),
        "pantry_small": (pantry_small.__doc__, [pantry_small()]),
        "snack_shelf": (snack_shelf.__doc__, [snack_shelf()]),
        "tv_stand": (tv_stand.__doc__, [tv_stand()]),
        "phone_booth": (phone_booth.__doc__, [phone_booth()]),
        "standing_desk": (standing_desk.__doc__, [standing_desk()]),
        "desk_chair": (desk_chair.__doc__, [desk_chair()]),
        "desk": (desk_south.__doc__, [desk_south()]),
        "desk_north": (desk_north.__doc__, [desk_north()]),
        "meeting_sofa": (meeting_sofa.__doc__, [meeting_sofa()]),
        "meeting_sofa_north": (meeting_sofa_north.__doc__, [meeting_sofa_north()]),
        "vending_machine": (vending_machine.__doc__, vending_machine()),
        "printer": (printer.__doc__, printer()),
        "meeting_table": (meeting_table.__doc__, [meeting_table()]),
        "kitchen_island": (kitchen_island.__doc__, [kitchen_island()]),
        "side_table": (side_table.__doc__, [side_table()]),
        "water_cooler": (water_cooler.__doc__, water_cooler()),
        "pantry_bin": (trash_bin.__doc__, [trash_bin()]),
        "fish_tank": (fish_tank.__doc__, fish_tank()),
        "coat_rack": (coat_rack.__doc__, [coat_rack()]),
        "notice_board": (notice_board.__doc__, [notice_board()]),
        "wall_clock": (wall_clock.__doc__, [wall_clock()]),
        "meeting_chair": (meeting_chair.__doc__, [meeting_chair()]),
        "door": (elevator_door.__doc__, elevator_door()),
    }
    classic = {
        "meeting_sofa_north": (meeting_sofa_north_1x.__doc__, [meeting_sofa_north_1x()]),
        "desk": (desk_south_1x.__doc__, [desk_south_1x()]),
        "desk_north": (desk_north_1x.__doc__, [desk_north_1x()]),
        "vending_machine": (vending_machine_1x.__doc__, vending_machine_1x()),
        "printer": (printer_1x.__doc__, printer_1x()),
        "meeting_table": (meeting_table_1x.__doc__, [meeting_table_1x()]),
        "kitchen_island": (kitchen_island_1x.__doc__, [kitchen_island_1x()]),
        "side_table": (side_table_1x.__doc__, [side_table_1x()]),
        "water_cooler": (water_cooler_1x.__doc__, water_cooler_1x()),
        "pantry_bin": (trash_bin_1x.__doc__, [trash_bin_1x()]),
        "fish_tank": (fish_tank_1x.__doc__, fish_tank_1x()),
        "coat_rack": (coat_rack_1x.__doc__, [coat_rack_1x()]),
        "notice_board": (notice_board_1x.__doc__, [notice_board_1x()]),
        "wall_clock": (wall_clock_1x.__doc__, [wall_clock_1x()]),
        "meeting_chair": (meeting_chair_1x.__doc__, [meeting_chair_1x()]),
    }
    sprites = {
        f"{base}@{S}x.sprite": render_sprite(header, frames)
        for base, (header, frames) in pieces.items()
    }
    for pose, (*_, header) in POSES.items():
        body, head = body_frame(pose)
        sprites[f"{pose}@{S}x.sprite"] = render_sprite(header + "\nBald: the pack's [hairstyles] dress it.", [body], [head])
    for style in HAIRSTYLES:
        for view, parts in hair_layers(style).items():
            for part, lyr in zip(("behind", "over"), parts):
                if lyr is not None and any(c != T for row in lyr for c in row):
                    sprites[hair_file(style, view, part)] = render_sprite(
                        f"The {style} hairstyle, {view} view: the layer {part} the body.",
                        [lyr],
                        [(view, HEAD_MARK[0], HEAD_MARK[1] + o)],
                    )
    sprites |= {f"{base}.sprite": render_sprite(header, frames) for base, (header, frames) in classic.items()}
    for name, draw in BUILDINGS.items():
        art = draw()
        sprites[f"building_{name}@{S}x.sprite"] = render_sprite(draw.__doc__, [art])
        sprites[f"building_{name}.sprite"] = render_sprite(
            draw.__doc__ + "\n\nIts base: the drawing read at 1x.", [city_base(art)]
        )
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
