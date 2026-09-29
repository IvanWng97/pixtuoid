#!/usr/bin/env python3
"""Author the bundled pack's generated sprite art: the cutaway profile's `@Nx`
pieces (N = `S`), and the classic profile's 1x pieces drawn from the same
layout.

Draws in PALETTE-KEY space, so the output is .sprite text and the recolor keys
(H/B/S/P and their [ramps] shades) survive per-agent recoloring. The .sprite
files it writes are committed; edit this, not them, and rerun:

    just gen-art

`--check` writes nothing and fails if a sprite it draws is missing or differs
from the committed one, or if a sprite carrying the provenance line is no longer
drawn here (a write deletes those).

It resolves no colour itself: the engine owns the palette and its ramps, so look
at the result through the real renderer: `cargo run --release --example
cutaway_snapshot -- <out.png>` for the `@Nx` art, `cargo run --release --example
snapshot -- --crop-furniture desk <out.png>` for the 1x desk.
"""

import argparse
import difflib
import inspect
import math
import pathlib
import random
import sys
import tempfile

S = 4  # the cutaway art's density

# ---- palette keys ---------------------------------------------------------
# recolor bases and their shades (bases in [palette], shades in [ramps])
HAIR, HAIR_SH, HAIR_LT, HAIR_HI = "H", "h", "Y", "Z"
SKIN, SKIN_SH, SKIN_DK = "S", "s", "i"
SHIRT, SHIRT_SH, SHIRT_LT, SHIRT_OUT = "B", "v", "W", "U"
PANTS, PANTS_SH, PANTS_LT = "P", "p", "("
# fixed colours, named by role
EYE = "e"
SHOE, SHOE_HI = ";", ":"
WOOD, WOOD_SH, WOOD_LT, WOOD_HI, WOOD_DK, POOL = "D", "d", "O", "σ", "ψ", "ω"
BEZEL, SLATE, SHADOW = "M", "3", "4"
# The desk monitor's screen: the cutaway relights these keys, so only desk art
# draws them.
GLASS, GLASS_TXT = "j", "J"
KEY_DK, KEYCAP, GREY = "k", "φ", "6"
LAMP, LAMP_HI, BULB = "7", "Θ", "9"
OFFWHITE, OFFWHITE_SH, PRINT = "¤", "!", "$"
MUG, MUG_SH = "V", "%"
CHAIR, CHAIR_RIM, CHAIR_BASE = "T", "I", "/"
# The @Nx art's one outline, round each piece's whole silhouette whatever its
# material.
SILHOUETTE = "κ"
COFFEE = "ρ"
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


# ---- the characters ----------------------------------------------------------
# A chibi figure: a big head over a small body, one near-black line round the
# whole silhouette. Every pose is a bald BODY, the face or the back of the head
# but never a scalp, that a hairstyle's layers dress per view: `behind` under
# the body, `over` on top. The union is outlined last, so no line runs between
# hair and face.
FIG_W = 8 * S
FIG_CX = (FIG_W - 1) / 2
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


def paste(dst, src, dx=0, dy=0):
    """`src`'s opaque pixels onto `dst`, moved `dx` columns and `dy` rows."""
    for y in range(len(src)):
        for x in range(len(src[0])):
            if src[y][x] != T and 0 <= y + dy < len(dst) and 0 <= x + dx < len(dst[0]):
                dst[y + dy][x + dx] = src[y][x]


# ---- hairstyles ----------------------------------------------------------------
# Each style draws four views on its own layer canvas, the tallest pose's rows
# plus HAIR_HEADROOM above for the hair rising over the head: `front` and `side` as
# (behind, over) pairs, `back` and `crown` as one layer over the body. Every
# coordinate below is on the pose grid; `o` shifts it onto the layer.
HAIR_HEADROOM = 6
LAYER_H = STANDING_ROWS * S + HAIR_HEADROOM
o = HAIR_HEADROOM
# The skull in profile, and the head seen from above as it lies on the arms.
SIDE_CX, SIDE_CY = 14.0, 12.5
CROWN_CX, CROWN_CY, CROWN_R = 15.5, 16.5, 10.0


def layer():
    return canvas(FIG_W, LAYER_H)


def front_fringe(tips):
    f = layer()
    fringe(f, tips, FACE_TOP - 4, o)
    return rim_light(f, lit_top=False)


def side_locks(locks, top=8):
    f = layer()
    fringe_locks(f, locks, top, o)
    return rim_light(f, lit_top=False)


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


# Hairstyles by name; every bundled character is baked in `BAKED_STYLE`.
HAIRSTYLES = {"mop": style_mop}


# ---- bodies ------------------------------------------------------------------------
# Drawn on the pose grid, shifted down `dy` rows onto a dressing canvas.
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
    """Typing seen from behind, where the chair back and its arm pads hide the
    hands: the reaching arm's elbow juts out past the chair, swapping sides
    each frame as the hands do."""
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
# The hairstyle every baked character wears until the renderer dresses each
# agent in its own.
BAKED_STYLE = "mop"
# The ears a style leaves bare, per view.
EARS = {"front": ears_front, "back": ears_back, "side": ear_side}
# Where a face-down head lies, as an offset from the crown's rest.
CROWN_SHIFT = {"seated_sleeping": (0, 0), "seated_sleeping_alt": (1, 1)}


def shade_by_skin(g):
    """Hair touching skin sits in the fringe's shadow."""
    for y in range(len(g)):
        for x in range(1, FIG_W - 1):
            if g[y][x] in (HAIR, HAIR_LT) and any(
                    0 <= y + ddy < len(g) and g[y + ddy][x + ddx] in SKIN_KEYS
                    for ddx, ddy in ((1, 0), (-1, 0), (0, 1))):
                g[y][x] = HAIR_SH


def dressed(pose, style):
    """`pose` dressed in `style`, the hair's headroom cut off: the frame the
    bundled pack bakes until the renderer composes hair itself."""
    rows, view, body, _ = POSES[pose]
    look = HAIRSTYLES[style]()
    h = rows * S
    g = canvas(FIG_W, h + o)
    if view in ("front", "side"):
        behind, over = look[view]
    else:
        behind, over = None, look[view]
    if behind is not None:
        paste(g, behind)
    # The head, then the body in front of it: a collar over the neck, a raised
    # mug and its steam over the chin.
    if view == "front":
        face_front(g, o)
    elif view == "back":
        nape(g, o)
    elif view == "side":
        side_face(g, o)
    if look["ears"] and view in EARS:
        EARS[view](g, o)
    body(g, o)
    paste(g, over, *CROWN_SHIFT.get(pose, (0, 0)))
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


# ---- output -----------------------------------------------------------------
PROVENANCE = "Generated by scripts/gen-art.py: edit the generator, not this file."
ENCODING = "utf-8"  # the keys include σ/ψ/Θ, and the locale's encoding need not be the file's


def render_sprite(header, frames):
    text = inspect.cleandoc(header) + "\n" + PROVENANCE
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
        **{pose: (POSES[pose][3], [dressed(pose, BAKED_STYLE)]) for pose in POSES},
        "desk_chair": (desk_chair.__doc__, [desk_chair()]),
        "desk": (desk_south.__doc__, [desk_south()]),
        "desk_north": (desk_north.__doc__, [desk_north()]),
    }
    classic = {
        "desk": (desk_south_1x.__doc__, [desk_south_1x()]),
        "desk_north": (desk_north_1x.__doc__, [desk_north_1x()]),
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
