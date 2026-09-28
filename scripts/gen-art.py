#!/usr/bin/env python3
"""Author the bundled pack's generated sprite art: the cutaway profile's `@8x`
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
cutaway_snapshot -- <out.png> --scale 8` for the `@8x` art, `cargo run --release
--example snapshot -- --crop-furniture <piece> <out.png>` for the 1x.
"""

import argparse
import difflib
import math
import pathlib
import random
import sys
import tempfile

S = 8  # authoring density

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


# A seated figure's torso ends at the seat: the base's row below it holds only a
# hand, so a taller (typing) canvas grows no shirt.
SEAT = 10 * S
SHIRT_KEYS = {SHIRT, SHIRT_SH, SHIRT_LT, WHITE, WHITE_SH}


def canvas(w, h):
    return [[T] * w for _ in range(h)]


def put(g, x, y, k):
    if 0 <= y < len(g) and 0 <= x < len(g[0]):
        g[y][x] = k


def rect(g, x0, y0, x1, y1, k):
    for y in range(max(0, y0), min(len(g), y1)):
        for x in range(max(0, x0), min(len(g[0]), x1)):
            g[y][x] = k


def lit(nx, ny, light=(-0.55, -0.8)):
    """Lambert-ish term for a sphere normal (nx, ny), light from the windows."""
    nx, ny = max(-1.0, min(1.0, nx)), max(-1.0, min(1.0, ny))
    nz = math.sqrt(max(0.0, 1 - nx * nx - ny * ny))
    lx, ly, lz = light[0], light[1], 0.35
    n = math.sqrt(lx * lx + ly * ly + lz * lz)
    return (nx * lx + ny * ly + nz * lz) / n


def below_seat_bare(g):
    """Clear any shirt below the seat line."""
    for y in range(SEAT, len(g)):
        for x in range(len(g[0])):
            if g[y][x] in SHIRT_KEYS:
                g[y][x] = T


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


# ---- the figure, back view ---------------------------------------------------
def figure_back(height, elbows, hands=()):
    """A seated figure from behind. `elbows` lists (x, y) elbow points: the arms
    show as sleeves falling from the shoulders to the elbow. `hands` pairs with
    them where a typist's hand shows past the seat, as the base's do."""
    w = 8 * S
    g = canvas(w, height)
    cx = 31.5
    # torso: neck -> shoulders -> straight back, down to the seat
    top = 44
    for y in range(top, min(height, SEAT)):
        t = min(1.0, (y - top) / 14)
        half = 9 + (23.5 - 9) * math.sin(t * math.pi / 2)
        for x in range(w):
            if abs(x + 0.5 - cx) <= half:
                dx = (x + 0.5 - cx) / 26
                v = lit(dx * 0.8, -0.7 + 1.1 * t)
                g[y][x] = SHIRT_LT if (v > 0.6 and y < top + 9) else SHIRT if v > 0.12 else SHIRT_SH
    # sleeves from the shoulders to the elbows, parted from the back by a crease
    for ex, ey in elbows:
        sx = 10 if ex < cx else 53
        capsule(g, [(sx, 51), (ex, ey)], 4.2, SHIRT)
        capsule(g, [(sx + (1 if ex < cx else -1), 55), (ex, ey)], 2.2, SHIRT_SH)
    for (ex, ey), (hx, hy) in zip(elbows, hands):  # forearms down to the hands
        capsule(g, [(ex, ey), (hx, hy - 4)], 3.4, SHIRT)
    below_seat_bare(g)
    for hx, hy in hands:
        hand(g, hx, hy)
    for y in range(54, min(height, SEAT)):
        put(g, 32, y, SHIRT_SH)
    for i in range(7):
        put(g, 21 + i // 2, 62 + i, SHIRT_SH)
        put(g, 42 - i // 2, 62 + i, SHIRT_SH)
    # neck
    for y in range(36, 50):
        for x in range(26, 38):
            g[y][x] = SKIN_DK if y < 40 else SKIN_SH if y < 44 else SKIN
    # collar
    for x in range(23, 41):
        dy = int(abs(x + 0.5 - cx) / 4)
        put(g, x, 47 - dy, WHITE)
        put(g, x, 48 - dy, WHITE_SH)
    # ears, half behind the hair
    for ex in (11.5, 51.5):
        for y in range(22, 36):
            for x in range(w):
                if ((x + 0.5 - ex) / 2.7) ** 2 + ((y + 0.5 - 29) / 4.6) ** 2 <= 1:
                    g[y][x] = SKIN_SH if (x < cx) == (x < ex) else SKIN
    # hair: bumpy curl silhouette + sparse lit curl clumps
    hcx, hcy, hrx, hry = cx, 20.5, 21.0, 21.5
    rng = random.Random(11)
    bumps = [(rng.uniform(0, 2 * math.pi), rng.uniform(0.025, 0.06)) for _ in range(3)]

    def edge(theta):
        return 1.0 + sum(a * math.sin(k * theta + p) for (p, a), k in zip(bumps, (9, 13, 17)))

    for y in range(height):
        for x in range(w):
            dx, dy = x + 0.5 - hcx, y + 0.5 - hcy
            if math.hypot(dx / hrx, dy / hry) > edge(math.atan2(dy, dx)):
                continue
            v = lit(dx / hrx, dy / hry)
            g[y][x] = HAIR if v > 0.12 else HAIR_SH
    for _ in range(54):
        ang = rng.uniform(0, 2 * math.pi)
        rad = math.sqrt(rng.uniform(0, 1)) * 0.86
        ccx = hcx + math.cos(ang) * rad * hrx
        ccy = hcy + math.sin(ang) * rad * hry
        v = lit((ccx - hcx) / hrx, (ccy - hcy) / hry)
        if v < -0.1 and rng.random() < 0.65:
            continue
        cr = rng.uniform(1.6, 2.7)
        for a in range(-100, 20, 12):
            tt = math.radians(a - 45)
            px, py = int(ccx + math.cos(tt) * cr), int(ccy + math.sin(tt) * cr)
            if 0 <= py < height and 0 <= px < w and g[py][px] in (HAIR, HAIR_SH):
                g[py][px] = HAIR_HI if (v > 0.55 and a < -50) else HAIR_LT
    outline(g, {HAIR, HAIR_SH, HAIR_LT, HAIR_HI}, HAIR_OUT)
    outline(g, {SHIRT, SHIRT_SH, SHIRT_LT, WHITE, WHITE_SH}, SHIRT_OUT)
    outline(g, {SKIN, SKIN_SH, SKIN_DK}, OUTLINE)
    return g


# ---- the task chair from behind --------------------------------------------------
def chair():
    w, h = 8 * S, 5 * S
    g = canvas(w, h)
    x0, x1, y0, y1, r = 15, 49, 0, 25, 7
    for y in range(y0, y1):
        for x in range(x0, x1):
            ccx = min(max(x + 0.5, x0 + r), x1 - r)
            ccy = min(max(y + 0.5, y0 + r), y1 - r)
            if (x + 0.5 - ccx) ** 2 + (y + 0.5 - ccy) ** 2 <= r * r:
                g[y][x] = CHAIR
    for y in range(y0, y1):
        for x in range(x0, x1):
            if g[y][x] != CHAIR:
                continue
            up = y == 0 or g[y - 1][x] == T
            left = g[y][x - 1] == T
            if up or (left and y < 12):
                g[y][x] = CHAIR_RIM
    for x in (26, 38):  # two stitched channels
        for y in range(5, y1 - 4):
            if g[y][x] == CHAIR:
                g[y][x] = SHADOW
    for xa in (8, 50):  # armrests
        for y in range(15, 22):
            for x in range(xa, xa + 6):
                g[y][x] = CHAIR_RIM if y == 15 else CHAIR_BASE
    rect(g, 18, 25, 46, 28, CHAIR_BASE)  # seat edge
    rect(g, 30, 28, 34, 32, SHADOW)  # gas stem
    for dx, dy in ((-15, 5), (15, 5), (-8, 7), (8, 7), (0, 7)):  # the five-star base
        capsule(g, [(32, 32.5), (32 + dx + 0.5, 32.5 + dy)], 1.1, CHAIR_BASE)
        rect(g, 32 + dx - 1, 32 + dy, 32 + dx + 2, 32 + dy + 2, KEY_DK)
    despeckle(g, {CHAIR_BASE, KEY_DK})
    outline(g, {CHAIR, CHAIR_RIM, CHAIR_BASE}, KEY_DK)
    fill_pinholes(g)
    return g


# ---- the raised back-turned desk -----------------------------------------------
# ---- the desk: one layout, drawn at 1x (classic) and at `S` (cutaway) ------
# Logical units, shared by both drawings so the `@8x` art is always exactly `S`
# times the 1x it redraws (validate-pack rejects any other size).
#
# The width is the desk's `FurnitureDef` visual width, pinned by
# `desk_sprite_width_tracks_the_footprint_overhang`.
DESK_W = 14
# The viewer-facing desk. Its row 0 is the monitor rising a row above the
# desk's back edge (the painter blits it at `desk.y - 1`).
DESK_H = 9
# The back-turned desk: two rows taller, all of them above, so the occupant
# (who y-sorts in FRONT of the desk) leaves the upper screen row clear. Its
# lower screen row sits inside the wood, flanked by it: the owner picked a
# monitor standing on its desk over one floating clear of every head.
DESK_NORTH_H = 11
DESK_LEG_W = 2
# The monitor's columns, and its rows from the sprite's top: casing 0-1, glass
# 2-3, chin 4 — where the classic painter's screen glow lands
# (`pixel_painter::effects`'s `SCREEN_*`), so the art keeps each role there.
DESK_MONITOR_X0, DESK_MONITOR_X1 = 3, 11


def desk_1x(h, monitor_top):
    """The classic desk's wood: a lit back edge, a bright front lip, and legs
    dark on their inner side, with open floor between them so the carpet and
    anyone walking behind the desk show through. Both desks share these bottom
    three rows. The props (lamp, mug, paper tower) are the classic painter's
    live overlays, so the art draws none of them."""
    g = canvas(DESK_W, h)
    top, lip = monitor_top + 1 if monitor_top else 1, h - 3
    rect(g, 0, top, DESK_W, lip, WOOD)
    rect(g, 0, top, DESK_W, top + 1, WOOD_LT)
    rect(g, 0, lip, DESK_W, lip + 1, WOOD_HI)
    for x0, inner in ((0, DESK_LEG_W - 1), (DESK_W - DESK_LEG_W, DESK_W - DESK_LEG_W)):
        rect(g, x0, lip + 1, x0 + DESK_LEG_W, h, WOOD_SH)
        rect(g, inner, lip + 1, inner + 1, h, WOOD_DK)
    return g


def desk_south_1x():
    """The viewer-facing desk at 1x: the monitor's back, lit along its top
    edge, on a stand throwing a shadow either side. Its glass rows stay casing:
    a viewer-facing seat never shows a screen, though the glow still lights
    them where a pack ships no `desk_north` and this art stands in for it."""
    g = desk_1x(DESK_H, 0)
    rect(g, DESK_MONITOR_X0, 0, DESK_MONITOR_X1, 4, BEZEL)
    rect(g, DESK_MONITOR_X0, 0, DESK_MONITOR_X1, 1, SLATE)
    rect(g, DESK_MONITOR_X0 + 1, 4, DESK_MONITOR_X1 - 1, 5, BEZEL)
    for x in (DESK_MONITOR_X0 + 1, DESK_MONITOR_X1 - 2):
        put(g, x, 4, SHADOW)
    return g


def desk_north_1x():
    """The back-turned desk at 1x: the raised monitor shows its glass, dim
    lines of text on it, and a keyboard sits in front of the stand."""
    g = desk_1x(DESK_NORTH_H, 2)
    rect(g, DESK_MONITOR_X0, 0, DESK_MONITOR_X1, 4, BEZEL)
    rect(g, DESK_MONITOR_X0, 0, DESK_MONITOR_X1, 1, SLATE)
    rect(g, DESK_MONITOR_X0 + 1, 2, DESK_MONITOR_X1 - 1, 4, GLASS)
    for x, y in ((5, 2), (6, 2), (8, 2), (6, 3), (7, 3)):
        put(g, x, y, GLASS_TXT)
    rect(g, DESK_MONITOR_X0 + 1, 4, DESK_MONITOR_X1 - 1, 5, BEZEL)
    for x in (DESK_MONITOR_X0 + 1, DESK_MONITOR_X1 - 2):
        put(g, x, 4, SHADOW)
    rect(g, DESK_MONITOR_X0 + 1, 5, DESK_MONITOR_X1 - 1, 6, GREY)
    for x in (DESK_MONITOR_X0 + 1, DESK_MONITOR_X1 - 2):
        put(g, x, 5, KEY_DK)
    return g


def desk_north():
    w, h = DESK_W * S, DESK_NORTH_H * S
    g = canvas(w, h)
    rng = random.Random(3)
    # top surface, boards running left-right: the base's rows 3 to 7
    rect(g, 0, 24, w, 64, WOOD)
    for y in range(24, 64):
        if (y - 24) % 12 == 0:
            rect(g, 0, y, w, y + 1, WOOD_SH)
    for _ in range(26):
        y = rng.randrange(25, 63)
        if (y - 24) % 12 == 0:
            continue
        x0 = rng.randrange(0, w - 8)
        rect(g, x0, y, min(w, x0 + rng.randrange(8, 26)), y + 1,
             WOOD_SH if rng.random() < 0.6 else WOOD_LT)
    rect(g, 0, 24, w, 25, WOOD_LT)
    # the lamp's warm pool on the east wing
    for y in range(26, 64):
        for x in range(64, w):
            d = math.hypot((x - 100) / 30, (y - 44) / 18)
            if d < 1 and g[y][x] == WOOD:
                g[y][x] = POOL if d < 0.55 else WOOD_LT
    # front lip and legs: the base's rows 8 to 10
    rect(g, 0, 64, w, 72, WOOD)
    rect(g, 0, 64, w, 66, WOOD_HI)
    rect(g, 0, 70, w, 72, WOOD_SH)
    rect(g, 0, 72, 16, 88, WOOD_SH)
    rect(g, 96, 72, w, 88, WOOD_SH)
    rect(g, 14, 72, 16, 88, WOOD_DK)
    rect(g, 96, 72, 98, 88, WOOD_DK)
    # monitor on a stand, raised above the desk's back edge
    rect(g, 24, 1, 88, 31, BEZEL)
    rect(g, 24, 1, 88, 2, SLATE)
    rect(g, 24, 30, 88, 31, SHADOW)
    rect(g, 27, 4, 85, 28, GLASS)
    for i, (x0, ln) in enumerate(((31, 22), (31, 30), (35, 18), (35, 26), (31, 12), (35, 32), (31, 20))):
        rect(g, x0, 7 + i * 3, x0 + ln, 8 + i * 3, GLASS_TXT)
    rect(g, 52, 31, 60, 36, SHADOW)
    rect(g, 44, 35, 68, 38, BEZEL)
    rect(g, 44, 35, 68, 36, SLATE)
    # keyboard and mouse
    rect(g, 32, 40, 80, 47, KEY_DK)
    for ky in (41, 43, 45):
        for kx in range(33, 79, 3):
            rect(g, kx, ky, kx + 2, ky + 1, KEYCAP)
    rect(g, 32, 40, 80, 41, GREY)
    rect(g, 84, 41, 89, 47, KEYCAP)
    rect(g, 84, 41, 89, 42, GREY)
    # desk lamp on the east wing: weighted base, jointed arm, cone shade
    rect(g, 95, 46, 109, 51, LAMP)
    rect(g, 95, 46, 109, 47, LAMP_HI)
    for t in range(16):
        rect(g, 102 - t // 5, 45 - t, 104 - t // 5, 46 - t, LAMP_HI if t % 5 else LAMP)
    for t in range(8):
        rect(g, 99 - t, 29 - t // 3, 101 - t, 31 - t // 3, LAMP_HI)
    rect(g, 98, 27, 102, 31, LAMP)
    for i in range(7):
        rect(g, 86 - i // 2, 20 + i, 96 + i // 2, 21 + i, LAMP if i else LAMP_HI)
    rect(g, 85, 27, 97, 28, BULB)
    for y in range(28, 34):
        half = 3 + (y - 28)
        for x in range(91 - half, 91 + half):
            if 0 <= x < w and g[y][x] in (WOOD, WOOD_LT, WOOD_SH, POOL):
                g[y][x] = POOL
    # paper stack and mug on the west wing
    rect(g, 5, 33, 22, 50, OFFWHITE_SH)
    rect(g, 4, 32, 21, 48, OFFWHITE)
    for i in range(5):
        rect(g, 6, 35 + i * 3, 18 - (i % 2) * 4, 36 + i * 3, PRINT)
    rect(g, 8, 52, 17, 61, MUG)
    rect(g, 8, 52, 17, 54, MUG_SH)
    rect(g, 17, 55, 19, 59, MUG_SH)
    outline(g, {BEZEL, SLATE, GLASS, GLASS_TXT}, SHADOW)
    return g


# ---- the figure, front view -----------------------------------------------------
def figure_front(height, hands):
    """A seated figure facing the viewer: face, curls framing it, shirt."""
    w = 8 * S
    g = canvas(w, height)
    cx = 31.5
    # torso (drawn first; the head overlaps its top), down to the seat
    top = 44
    for y in range(top, min(height, SEAT)):
        t = min(1.0, (y - top) / 14)
        half = 9 + (30.5 - 9) * math.sin(t * math.pi / 2)
        for x in range(w):
            if abs(x + 0.5 - cx) <= half:
                dx = (x + 0.5 - cx) / 30
                v = lit(dx * 0.8, -0.7 + 1.1 * t)
                g[y][x] = SHIRT_LT if (v > 0.6 and y < top + 9) else SHIRT if v > 0.12 else SHIRT_SH
    for y in range(56, min(height, SEAT)):  # arms part from the chest by a crease
        put(g, 12 if y < 70 else 13, y, SHIRT_SH)
        put(g, 51 if y < 70 else 50, y, SHIRT_SH)
    for i in range(6):  # a V-neck placket
        put(g, 29 + i // 3, 50 + i, SHIRT_SH)
        put(g, 34 - i // 3, 50 + i, SHIRT_SH)
    # neck, shaded under the chin
    for y in range(38, 50):
        for x in range(26, 38):
            g[y][x] = SKIN_DK if y < 42 else SKIN_SH if y < 45 else SKIN
    for x in range(23, 41):  # collar points
        dy = int(abs(x + 0.5 - cx) / 4)
        if abs(x + 0.5 - cx) > 3:
            put(g, x, 46 + dy, WHITE)
            put(g, x, 47 + dy, WHITE_SH)
    # ears
    for ex in (12.0, 51.0):
        for y in range(18, 32):
            for x in range(w):
                if ((x + 0.5 - ex) / 2.7) ** 2 + ((y + 0.5 - 25) / 4.3) ** 2 <= 1:
                    g[y][x] = SKIN_SH if x > cx else SKIN
    # hair mass behind the face
    hcx, hcy, hrx, hry = cx, 17.0, 21.0, 18.0
    rng = random.Random(23)
    bumps = [(rng.uniform(0, 2 * math.pi), rng.uniform(0.025, 0.06)) for _ in range(3)]

    def edge(theta):
        return 1.0 + sum(a * math.sin(k * theta + p) for (p, a), k in zip(bumps, (9, 13, 17)))

    for y in range(height):
        for x in range(w):
            dx, dy = x + 0.5 - hcx, y + 0.5 - hcy
            if math.hypot(dx / hrx, dy / hry) > edge(math.atan2(dy, dx)):
                continue
            g[y][x] = HAIR if lit(dx / hrx, dy / hry) > 0.12 else HAIR_SH
    # face: a rounded oval, lit from the upper left
    fcx, fcy, frx, fry = cx, 26.0, 15.0, 15.5
    for y in range(height):
        for x in range(w):
            nx, ny = (x + 0.5 - fcx) / frx, (y + 0.5 - fcy) / fry
            if nx * nx + ny * ny > 1 or y < 14:
                continue
            v = lit(nx, ny)
            g[y][x] = SKIN if v > -0.12 else SKIN_SH if v > -0.5 else SKIN_DK
    # fringe: curls falling over the brow, lit on their tops
    for i in range(9):
        bx = 17 + i * 3.6
        by = 14 + 2.4 * math.sin(i * 1.7)
        for y in range(int(by) - 3, int(by) + 4):
            for x in range(int(bx) - 3, int(bx) + 4):
                if ((x + 0.5 - bx) / 3.1) ** 2 + ((y + 0.5 - by) / 3.2) ** 2 <= 1:
                    put(g, x, y, HAIR_LT if y < by - 1 else HAIR)
    for _ in range(40):  # sparse curl highlights on the crown
        ang = rng.uniform(math.pi * 1.05, math.pi * 1.95)
        rad = math.sqrt(rng.uniform(0.2, 1)) * 0.85
        ccx = hcx + math.cos(ang) * rad * hrx
        ccy = hcy + math.sin(ang) * rad * hry
        v = lit((ccx - hcx) / hrx, (ccy - hcy) / hry)
        cr = rng.uniform(1.6, 2.5)
        for a in range(-100, 20, 12):
            tt = math.radians(a - 45)
            px, py = int(ccx + math.cos(tt) * cr), int(ccy + math.sin(tt) * cr)
            if 0 <= py < height and 0 <= px < w and g[py][px] in (HAIR, HAIR_SH):
                g[py][px] = HAIR_HI if (v > 0.55 and a < -50) else HAIR_LT
    # features: brows, eyes with a catchlight, nose shade, mouth, cheeks
    for bx0 in (20, 36):
        rect(g, bx0, 20, bx0 + 7, 21, HAIR_SH)
    for ex0 in (22, 37):
        rect(g, ex0, 23, ex0 + 4, 28, EYE)
        put(g, ex0 + 1, 24, BRIGHT)
    rect(g, 31, 29, 33, 32, SKIN_SH)
    rect(g, 29, 34, 35, 35, MOUTH)
    rect(g, 30, 35, 34, 36, SKIN_SH)
    for kx in (19, 40):
        rect(g, kx, 30, kx + 5, 32, BLUSH)
    # hands at the sides (typing alternates them like the base frames)
    for hx, hy in hands:
        for y in range(hy - 6, hy + 5):
            for x in range(hx - 5, hx + 5):
                if ((x + 0.5 - hx) / 4.6) ** 2 + ((y + 0.5 - hy) / 5.2) ** 2 <= 1:
                    cuff = y < min(hy - 2, SEAT)
                    put(g, x, y, SHIRT_SH if cuff else SKIN if (x + 0.5) < hx + 1 else SKIN_SH)
    outline(g, {HAIR, HAIR_SH, HAIR_LT, HAIR_HI}, HAIR_OUT)
    outline(g, {SHIRT, SHIRT_SH, SHIRT_LT, WHITE, WHITE_SH}, SHIRT_OUT)
    outline(g, {SKIN, SKIN_SH, SKIN_DK}, OUTLINE)
    return g


# ---- the viewer-facing desk: the monitor's BACK --------------------------------
def desk_south():
    w, h = DESK_W * S, DESK_H * S
    g = canvas(w, h)
    rng = random.Random(5)
    # top surface, boards left-right: the base's rows 1 to 5
    rect(g, 0, 8, w, 48, WOOD)
    for y in range(8, 48):
        if (y - 8) % 12 == 0:
            rect(g, 0, y, w, y + 1, WOOD_SH)
    for _ in range(22):
        y = rng.randrange(9, 47)
        if (y - 8) % 12 == 0:
            continue
        x0 = rng.randrange(0, w - 8)
        rect(g, x0, y, min(w, x0 + rng.randrange(8, 24)), y + 1,
             WOOD_SH if rng.random() < 0.6 else WOOD_LT)
    rect(g, 0, 8, w, 9, WOOD_LT)
    # lamp pool on the west wing
    for y in range(10, 48):
        for x in range(0, 48):
            d = math.hypot((x - 12) / 28, (y - 26) / 17)
            if d < 1 and g[y][x] == WOOD:
                g[y][x] = POOL if d < 0.55 else WOOD_LT
    # front lip and legs: the base's rows 6 to 8
    rect(g, 0, 48, w, 56, WOOD)
    rect(g, 0, 48, w, 50, WOOD_HI)
    rect(g, 0, 54, w, 56, WOOD_SH)
    rect(g, 0, 56, 16, 72, WOOD_SH)
    rect(g, 96, 56, w, 72, WOOD_SH)
    rect(g, 14, 56, 16, 72, WOOD_DK)
    rect(g, 96, 56, 98, 72, WOOD_DK)
    # the monitor's back: casing, a vent grille and a badge, on a stand
    rect(g, 24, 0, 88, 30, BEZEL)
    rect(g, 24, 0, 88, 1, SLATE)
    rect(g, 24, 1, 25, 30, SLATE)
    rect(g, 24, 28, 88, 30, SHADOW)
    for vy in range(6, 14, 2):
        rect(g, 34, vy, 78, vy + 1, SHADOW)
    rect(g, 52, 18, 60, 22, SLATE)
    rect(g, 52, 30, 60, 36, SHADOW)
    rect(g, 42, 35, 70, 39, BEZEL)
    rect(g, 42, 35, 70, 36, SLATE)
    # desk lamp on the west wing, shade toward the occupant's side
    rect(g, 3, 32, 17, 37, LAMP)
    rect(g, 3, 32, 17, 33, LAMP_HI)
    for t in range(16):
        rect(g, 10 + t // 5, 31 - t, 12 + t // 5, 32 - t, LAMP_HI if t % 5 else LAMP)
    rect(g, 11, 12, 15, 16, LAMP)
    for i in range(7):
        rect(g, 12 - i // 2, 6 + i, 22 + i // 2, 7 + i, LAMP if i else LAMP_HI)
    rect(g, 11, 13, 23, 14, BULB)
    # a paper stack and a mug on the east wing
    rect(g, 92, 18, 109, 34, OFFWHITE_SH)
    rect(g, 91, 17, 108, 32, OFFWHITE)
    for i in range(4):
        rect(g, 93, 20 + i * 3, 104 - (i % 2) * 4, 21 + i * 3, PRINT)
    rect(g, 94, 37, 103, 46, MUG)
    rect(g, 94, 37, 103, 39, MUG_SH)
    rect(g, 103, 40, 105, 44, MUG_SH)
    outline(g, {BEZEL, SLATE}, SHADOW)
    return g


# ---- standing and walking: their own body, arms free of the torso -----------------
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


def hand(g, cx, cy):
    for y in range(cy - 4, cy + 4):
        for x in range(cx - 4, cx + 4):
            if ((x + 0.5 - cx) / 3.8) ** 2 + ((y + 0.5 - cy) / 3.6) ** 2 <= 1:
                put(g, x, y, SKIN if x + 0.5 < cx + 1 else SKIN_SH)


def standing(view, stride=0, arms=None, props=()):
    """A standing figure: head from the seated figure, then a body of its own."""
    w, h = 8 * S, 12 * S
    # the head (and collar) from the seated figure; its torso rows are replaced below
    fig = figure_front if view == "front" else figure_back
    head = fig(9 * S, [])
    g = canvas(w, h)
    cx = 31.5
    # legs first, so the torso hem overlaps them
    ly_top, ly_bot = 68, 89
    lx = {0: (24.5, 39.5), -1: (20.5, 41.5), 1: (22.5, 43.5)}[stride]
    lift = {0: (0, 0), -1: (0, -7), 1: (-7, 0)}[stride]
    for (fx, lf) in zip(lx, lift):
        capsule(g, [(fx, ly_top), (fx, ly_bot + lf)], 6.4, PANTS)
    for y in range(ly_top, h):  # shade the east side of each leg
        for x in range(w):
            if g[y][x] == PANTS and (x + 0.5) > (lx[0] + 3 if x < cx else lx[1] + 3):
                g[y][x] = PANTS_SH
    if stride == 0:  # the inseam of legs together
        rect(g, 31, 72, 33, ly_bot - 4, PANTS_SH)
    for (fx, lf) in zip(lx, lift):  # shoes, toes toward the viewer
        fy = ly_bot + lf + 2
        for y in range(fy - 4, fy + 4):
            for x in range(int(fx) - 8, int(fx) + 8):
                if ((x + 0.5 - fx) / 7.5) ** 2 + ((y + 0.5 - fy) / 3.8) ** 2 <= 1:
                    put(g, x, y, SHOE_HI if (y < fy - 1 and view == "front") else SHOE)
    # torso: shoulders -> waist, a belt at the hem
    top, hem = 44, 70
    for y in range(top, hem):
        t = (y - top) / (hem - top)
        half = 10 + 11 * math.sin(min(1.0, t * 3.2) * math.pi / 2) - 4 * max(0.0, t - 0.4)
        for x in range(w):
            if abs(x + 0.5 - cx) <= half:
                v = lit((x + 0.5 - cx) / 24, -0.7 + 1.2 * t)
                g[y][x] = SHIRT_LT if (v > 0.6 and y < top + 8) else SHIRT if v > 0.1 else SHIRT_SH
    rect(g, 17, hem - 2, 47, hem, PANTS_OUT)
    if view == "front":
        rect(g, 30, hem - 2, 34, hem, SLATE)  # buckle
    # arms: sleeves from the shoulders, hands at the ends
    # a walker's arms swing against their legs: the forward hand hangs lower
    swing = {0: (67, 67), -1: (63, 70), 1: (70, 63)}[stride]
    arms = arms or [((12, 50), (9, 60), (8, swing[0])), ((52, 50), (55, 60), (56, swing[1]))]
    for pts in arms:
        capsule(g, list(pts), 3.6, SHIRT)
        capsule(g, list(pts)[1:], 3.0, SHIRT_SH)
    for p_ in props:
        p_(g)
    for pts in arms:
        hand(g, *pts[-1])
    # paste the head and collar over the body
    for y in range(0, 50):
        for x in range(w):
            k = head[y][x]
            if k != T and not (y >= top and k in (SHIRT, SHIRT_SH, SHIRT_LT, SHIRT_OUT)):
                g[y][x] = k
    despeckle(g, {SHIRT_LT})
    outline(g, {SHIRT, SHIRT_SH, SHIRT_LT, WHITE, WHITE_SH}, SHIRT_OUT)
    outline(g, {SKIN, SKIN_SH, SKIN_DK}, OUTLINE)
    outline(g, {PANTS, PANTS_SH, PANTS_LT, SLATE}, PANTS_OUT)
    outline(g, {SHOE, SHOE_HI}, KEY_DK)
    fill_pinholes(g)
    return g


def mug(cx, cy):
    def draw(g):
        rect(g, cx - 6, cy - 7, cx + 6, cy + 7, MUG)
        rect(g, cx + 2, cy - 7, cx + 6, cy + 7, MUG_SH)
        rect(g, cx - 5, cy - 6, cx + 5, cy - 4, CYAN)
        rect(g, cx + 6, cy - 3, cx + 9, cy + 3, MUG_SH)
        rect(g, cx + 6, cy - 1, cx + 7, cy + 1, T)
        outline(g, {MUG, MUG_SH, CYAN}, SHADOW)
    return draw


# ---- asleep at the desk, in profile, on the couch -----------------------------------
def hair_dome(g, cx, cy, rx, ry, seed, clumps=40):
    """A head of curls seen from above or behind, lit from the windows."""
    h, w = len(g), len(g[0])
    rng = random.Random(seed)
    bumps = [(rng.uniform(0, 2 * math.pi), rng.uniform(0.025, 0.06)) for _ in range(3)]

    def edge(theta):
        return 1.0 + sum(a * math.sin(k * theta + p) for (p, a), k in zip(bumps, (9, 13, 17)))

    for y in range(h):
        for x in range(w):
            dx, dy = x + 0.5 - cx, y + 0.5 - cy
            if math.hypot(dx / rx, dy / ry) > edge(math.atan2(dy, dx)):
                continue
            g[y][x] = HAIR if lit(dx / rx, dy / ry) > 0.12 else HAIR_SH
    for _ in range(clumps):
        ang = rng.uniform(0, 2 * math.pi)
        rad = math.sqrt(rng.uniform(0, 1)) * 0.86
        ccx, ccy = cx + math.cos(ang) * rad * rx, cy + math.sin(ang) * rad * ry
        v = lit((ccx - cx) / rx, (ccy - cy) / ry)
        if v < -0.1 and rng.random() < 0.65:
            continue
        cr = rng.uniform(1.5, 2.5)
        for a in range(-100, 20, 12):
            tt = math.radians(a - 45)
            px, py = int(ccx + math.cos(tt) * cr), int(ccy + math.sin(tt) * cr)
            if 0 <= py < h and 0 <= px < w and g[py][px] in (HAIR, HAIR_SH):
                g[py][px] = HAIR_HI if (v > 0.55 and a < -50) else HAIR_LT


def back_body(g, top, cx, half_max, tilt=0.0):
    """Shoulders and back from behind, optionally leaning (`tilt` px per row)."""
    h, w = len(g), len(g[0])
    for y in range(top, h):
        t = min(1.0, (y - top) / 12)
        half = 10 + (half_max - 10) * math.sin(t * math.pi / 2)
        c = cx + tilt * (h - y)
        for x in range(w):
            if abs(x + 0.5 - c) <= half:
                v = lit((x + 0.5 - c) / (half_max + 4), -0.7 + 1.1 * t)
                g[y][x] = SHIRT_LT if (v > 0.6 and y < top + 7) else SHIRT if v > 0.12 else SHIRT_SH
        put(g, int(c), y, SHIRT_SH) if y > top + 6 else None


def asleep(slumped):
    """Face down on folded arms: the crown of the head over bare forearms crossed
    beneath it, a hand at each elbow."""
    w, h = 8 * S, 10 * S
    g = canvas(w, h)
    if not slumped:
        back_body(g, 38, 31.5, 30)
        capsule(g, [(7, 42), (16, 44)], 5.2, SHIRT)  # sleeves to the elbows
        capsule(g, [(57, 42), (48, 44)], 5.2, SHIRT)
        capsule(g, [(8, 45), (16, 46)], 3.0, SHIRT_SH)
        capsule(g, [(56, 45), (48, 46)], 3.0, SHIRT_SH)
        capsule(g, [(16, 43), (47, 47)], 3.2, SKIN)  # forearms crossed under the face
        capsule(g, [(48, 43), (17, 47)], 3.2, SKIN_SH)
        hand(g, 8, 36)
        hand(g, 56, 36)
        hair_dome(g, 31.5, 28, 17.5, 15, seed=41)
    else:
        back_body(g, 44, 30.5, 26, tilt=0.12)
        capsule(g, [(10, 50), (34, 48)], 5.0, SHIRT)
        capsule(g, [(12, 53), (32, 51)], 3.0, SHIRT_SH)
        hair_dome(g, 42, 34, 15.5, 14, seed=43)
    despeckle(g, {SHIRT_LT})
    outline(g, {HAIR, HAIR_SH, HAIR_LT, HAIR_HI}, HAIR_OUT)
    outline(g, {SHIRT, SHIRT_SH, SHIRT_LT}, SHIRT_OUT)
    outline(g, {SKIN, SKIN_SH, SKIN_DK}, OUTLINE)
    outline(g, {PANTS, PANTS_SH}, PANTS_OUT)
    return g


def profile():
    """Seated in profile, facing the way the base `side_seated` does (the seat code
    in `pixel_painter/seat.rs` owns which chair mirrors it)."""
    w, h = 8 * S, 10 * S
    g = canvas(w, h)
    cx = 33.5
    # a narrower torso: shoulder at the back, chest toward the east
    for y in range(44, 72):
        t = min(1.0, (y - 44) / 10)
        x0 = 16 - 2 * math.sin(t * math.pi / 2)
        x1 = 44 + 4 * math.sin(t * math.pi / 2)
        for x in range(w):
            if x0 <= x + 0.5 <= x1:
                v = lit((x + 0.5 - 30) / 20, -0.7 + 1.1 * t)
                g[y][x] = SHIRT_LT if (v > 0.6 and y < 52) else SHIRT if v > 0.1 else SHIRT_SH
    capsule(g, [(30, 50), (34, 60), (48, 62)], 4.0, SHIRT)  # forward arm
    capsule(g, [(33, 58), (47, 63)], 2.6, SHIRT_SH)
    hand(g, 53, 62)
    rect(g, 16, 72, 52, 80, PANTS)
    rect(g, 16, 72, 52, 74, PANTS_LT)
    rect(g, 44, 74, 52, 80, PANTS_SH)
    # neck
    for y in range(36, 46):
        for x in range(28, 38):
            g[y][x] = SKIN_SH if x < 31 or y < 40 else SKIN
    # head: face toward the east, hair over the crown and the back
    fcx, fcy, frx, fry = cx, 24.0, 15.5, 16.0
    for y in range(h):
        for x in range(w):
            nx, ny = (x + 0.5 - fcx) / frx, (y + 0.5 - fcy) / fry
            if nx * nx + ny * ny <= 1:
                g[y][x] = SKIN if lit(nx, ny) > -0.2 else SKIN_SH
    rect(g, 48, 26, 51, 31, SKIN)  # nose
    put(g, 50, 30, SKIN_SH)
    hair_dome(g, 28, 18, 17, 16, seed=47, clumps=30)
    for y in range(h):  # the face stays clear of the hair east of the brow
        for x in range(w):
            nx, ny = (x + 0.5 - fcx) / frx, (y + 0.5 - fcy) / fry
            if nx * nx + ny * ny <= 1 and x > 36 and y > 17:
                g[y][x] = SKIN if lit(nx, ny) > -0.2 else SKIN_SH
    rect(g, 40, 20, 46, 21, HAIR_SH)  # brow
    rect(g, 42, 23, 45, 28, EYE)
    put(g, 43, 24, BRIGHT)
    rect(g, 44, 34, 48, 35, MOUTH)
    rect(g, 40, 29, 44, 31, BLUSH)
    outline(g, {HAIR, HAIR_SH, HAIR_LT, HAIR_HI}, HAIR_OUT)
    outline(g, {SHIRT, SHIRT_SH, SHIRT_LT}, SHIRT_OUT)
    outline(g, {SKIN, SKIN_SH, SKIN_DK}, OUTLINE)
    outline(g, {PANTS, PANTS_SH, PANTS_LT}, PANTS_OUT)
    return g


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


def plant(kind):
    if kind == "plant":  # a leafy bush in terracotta
        g = canvas(6 * S, 7 * S)
        pot(g, 24, 32, 50, 9, 7)
        for ang, ln, wd, side in ((-2.6, 22, 6, 1), (-2.1, 24, 6.5, 1), (-1.57, 26, 7, 1), (-1.05, 24, 6.5, -1),
                                   (-0.5, 21, 6, -1), (-2.9, 17, 5, 1), (-0.2, 17, 5, -1), (-1.8, 18, 5, -1), (-1.3, 19, 5, 1)):
            leaf(g, 24, 32, ang, ln, wd, side)
        outline(g, {LEAF, LEAF_SH, LEAF_HI, LEAF_DK}, LEAF_DK)
        outline(g, {POT, POT_SH, POT_HI, SOIL}, OUTLINE)
        return g
    if kind == "plant_tall":  # a tall fig in a dark pot
        g = canvas(6 * S, 10 * S)
        rect(g, 23, 14, 26, 58, BROWN)  # the trunk, up to the topmost leaf, meeting both sides' bases
        pot(g, 24, 58, 78, 12, 10, dark=True)
        rng = random.Random(9)
        for i in range(15):
            y = 6 + i * 3.4
            side = -1 if i % 2 else 1
            ang = (-1.57 + side * rng.uniform(0.5, 1.1))
            leaf(g, 24 + side * 2, y + 8, ang, rng.uniform(13, 17), rng.uniform(5, 6.5), -side)
        outline(g, {LEAF, LEAF_SH, LEAF_HI, LEAF_DK}, LEAF_DK)
        outline(g, {OUTLINE, SHADOW, SLATE, SOIL}, KEY_DK)
        return g
    if kind == "plant_flower":  # a bloom cluster over a small pot
        g = canvas(6 * S, 6 * S)
        pot(g, 24, 32, 47, 8, 6)
        rng = random.Random(5)
        blooms = [(24 + rng.uniform(-12, 12), 14 + rng.uniform(-9, 7)) for _ in range(9)]
        for fx, fy in blooms:  # a stem to every bloom
            capsule(g, [(24, 33), (fx, fy + 3)], 0.7, LEAF_DK)
        leaf(g, 24, 32, -2.3, 12, 4, 1)
        leaf(g, 24, 32, -0.9, 12, 4, -1)
        for i, (fx, fy) in enumerate(blooms):
            col = PETAL_R if i % 2 else PETAL_Y
            for y in range(int(fy) - 4, int(fy) + 5):
                for x in range(int(fx) - 4, int(fx) + 5):
                    if (x + 0.5 - fx) ** 2 + (y + 0.5 - fy) ** 2 <= 16:
                        put(g, x, y, PETAL_HI if (y < fy - 1 and x < fx) else col)
            rect(g, int(fx), int(fy), int(fx) + 2, int(fy) + 2, GOLD if col == PETAL_R else DARK_RED)
        outline(g, {PETAL_R, PETAL_Y, PETAL_HI, DARK_RED, LEAF, LEAF_SH, LEAF_HI, LEAF_DK}, OUTLINE)
        outline(g, {POT, POT_SH, POT_HI, SOIL}, OUTLINE)
        return g
    # plant_succulent: a rosette in a wide shallow pot
    g = canvas(5 * S, 4 * S)
    pot(g, 20, 17, 31, 15, 13)
    for i in range(10):
        ang = -math.pi + i * math.pi / 9
        leaf(g, 20, 19, ang, 14, 4.2, 1 if i < 5 else -1)
    for i in range(5):
        ang = -math.pi + 0.3 + i * 0.6
        leaf(g, 20, 19, ang, 8, 3, 1)
    outline(g, {LEAF, LEAF_SH, LEAF_HI, LEAF_DK}, LEAF_DK)
    outline(g, {POT, POT_SH, POT_HI, SOIL}, OUTLINE)
    return g


def whiteboard():
    """A mobile whiteboard with a sprint board sketched on it."""
    g = canvas(14 * S, 11 * S)
    rect(g, 0, 0, 112, 60, ALU)
    rect(g, 0, 0, 112, 2, GREY)
    rect(g, 0, 58, 112, 60, ALU_DK)
    rect(g, 4, 4, 108, 56, OFFWHITE)
    for y in range(4, 56):  # a faint gloss band across the board
        for x in range(4, 108):
            if 0 < (x - y * 1.6) % 160 < 10:
                g[y][x] = BRIGHT
    rect(g, 4, 52, 108, 56, OFFWHITE_SH)  # the board's lower edge in its own shadow
    rng = random.Random(17)
    for col, x0 in enumerate((10, 44, 78)):  # three columns of sticky notes
        rect(g, x0, 8, x0 + 26, 10, INK)
        for r in range(3):
            if rng.random() < 0.8:
                c = (GOLD, CYAN, BLUSH)[(col + r) % 3]
                rect(g, x0 + 2, 14 + r * 12, x0 + 18, 24 + r * 12, c)
                rect(g, x0 + 4, 17 + r * 12, x0 + 14, 18 + r * 12, PRINT)
                rect(g, x0 + 4, 20 + r * 12, x0 + 11, 21 + r * 12, PRINT)
    for x in (40, 74):  # column dividers
        rect(g, x, 8, x + 1, 50, INK)
    for t in range(40):  # a burndown line in red
        x = 80 + t * 0.6
        y = 42 - t * 0.35 + 3 * math.sin(t / 5)
        put(g, int(x), int(y), RED)
    rect(g, 20, 60, 92, 63, ALU_DK)  # marker tray
    rect(g, 30, 59, 36, 61, RED)
    rect(g, 40, 59, 46, 61, BLUE)
    # stand: two legs, a cross bar, wheels
    for lx in (18, 92):
        rect(g, lx, 63, lx + 3, 84, ALU_DK)
        rect(g, lx - 4, 83, lx + 7, 88, KEY_DK)
    rect(g, 18, 74, 95, 76, ALU_DK)
    outline(g, {ALU, GREY}, SHADOW)
    return g


def bookshelf():
    """A wooden bookshelf, three shelves of books, a drawer."""
    g = canvas(8 * S, 12 * S)
    rect(g, 0, 0, 64, 88, WOOD_SH)
    rect(g, 4, 4, 60, 84, WOOD_DK)  # the dark interior
    rng = random.Random(21)
    for shelf in range(3):
        y_floor = 28 + shelf * 20
        x = 6
        while x < 56:
            bw = rng.randrange(3, 7)
            bh = rng.randrange(12, 18)
            col = BOOKS[rng.randrange(len(BOOKS))]
            if rng.random() < 0.12 and x < 48:  # a book leaning on the next
                for t in range(bh):
                    rect(g, x + t // 4, y_floor - t, x + t // 4 + bw, y_floor - t + 1, col)
                x += bw + bh // 4 + 1
                continue
            rect(g, x, y_floor - bh, min(56, x + bw), y_floor, col)
            rect(g, x, y_floor - bh, x + 1, y_floor, WHITE if col in (BLUE, DARK_RED, FABRIC) else OUTLINE)
            rect(g, x, y_floor - bh + 3, min(56, x + bw), y_floor - bh + 4, PRINT)
            x += bw + (1 if rng.random() < 0.3 else 0)
        rect(g, 4, y_floor, 60, y_floor + 3, WOOD)  # the shelf board
        rect(g, 4, y_floor, 60, y_floor + 1, WOOD_LT)
    rect(g, 4, 68, 60, 84, WOOD)  # the drawer
    rect(g, 4, 68, 60, 69, WOOD_LT)
    rect(g, 28, 75, 36, 77, SLATE)
    rect(g, 0, 0, 64, 2, WOOD_LT)
    rect(g, 0, 0, 2, 88, WOOD_LT)
    rect(g, 2, 88, 62, 96, T)
    outline(g, {WOOD, WOOD_SH, WOOD_LT, WOOD_DK}, OUTLINE)
    return g


def meeting_sofa():
    """A three-seat sofa from above, back to the north."""
    g = canvas(20 * S, 7 * S)
    # backrest
    for y in range(2, 22):
        for x in range(4, 156):
            g[y][x] = FABRIC_HI if y < 8 else FABRIC if y < 18 else FABRIC_SH
    # arms
    for ax in (0, 140):
        rect(g, ax, 6, ax + 20, 54, FABRIC)
        rect(g, ax, 6, ax + 20, 12, FABRIC_HI)
        rect(g, ax + (16 if ax == 0 else 0), 12, ax + (20 if ax == 0 else 4), 54, FABRIC_SH)
    # three seat cushions, each lit on its front-left, seams between
    for i in range(3):
        x0 = 20 + i * 40
        rect(g, x0, 22, x0 + 40, 52, FABRIC)
        rect(g, x0 + 2, 24, x0 + 38, 30, FABRIC_HI)
        rect(g, x0, 46, x0 + 40, 52, FABRIC_SH)
        rect(g, x0, 22, x0 + 1, 52, FABRIC_SEAM)
        for bx in (x0 + 12, x0 + 28):  # button tufts
            rect(g, bx, 36, bx + 2, 38, FABRIC_SEAM)
    rect(g, 20, 21, 140, 22, FABRIC_SEAM)
    rect(g, 6, 54, 12, 56, KEY_DK)  # feet
    rect(g, 148, 54, 154, 56, KEY_DK)
    outline(g, {FABRIC, FABRIC_HI, FABRIC_SH, FABRIC_SEAM}, OUTLINE)
    return g


# ---- furniture: screens, pantry, pods and small fixtures --------------------------
CORK, CORK_SH, STEEL, STEEL_SH = "Δ", "π", "K", "ξ"


def slide_screen(g, x0, y0, x1, y1):
    """A lit slide sized to its screen: title bar, bullet lines, a bar chart."""
    h, w = y1 - y0, x1 - x0
    rect(g, x0, y0, x1, y1, DISPLAY)
    rect(g, x0 + 4, y0 + 4, x1 - 4, y0 + 4 + max(3, h // 10), CYAN)
    body = y0 + 8 + max(3, h // 10)
    for i in range(max(1, (y1 - 6 - body) // 8)):
        rect(g, x0 + 6, body + i * 8, x0 + 6 + w * (3 if i % 2 else 4) // 10, body + i * 8 + 2, GREY)
    bx = x0 + w // 2 + 4
    bw = max(3, w // 16)
    for i, frac in enumerate((0.35, 0.6, 0.45, 0.8)):
        hgt = int((y1 - 6 - body) * frac)
        rect(g, bx + i * (bw + 3), y1 - 6 - hgt, bx + i * (bw + 3) + bw, y1 - 6, BLUE if i % 2 else LEAF)
    rect(g, bx - 2, y1 - 6, x1 - 6, y1 - 5, GREY)

def meeting_screen():
    """A wall-mounted widescreen showing a slide."""
    g = canvas(14 * S, 12 * S)
    rect(g, 0, 6, 112, 82, SHADOW)
    rect(g, 0, 6, 112, 8, SLATE)
    slide_screen(g, 5, 11, 107, 77)
    for y in range(11, 77):  # a soft sheen across the glass
        for x in range(5, 107):
            if 0 <= (x + y * 0.8) % 140 < 6 and g[y][x] == DISPLAY:
                g[y][x] = DISPLAY_SHEEN
    rect(g, 34, 82, 78, 88, BEZEL)
    rect(g, 50, 88, 62, 96, SHADOW)
    outline(g, {SHADOW, SLATE, BEZEL}, KEY_DK)
    return g


def fridge(g, x0, y0, w, h):
    """A fridge `w` x `h`: every fitting sized by the door, so a mini fridge keeps
    its handles and magnets on its own doors."""
    seam = y0 + h * 2 // 5  # the freezer door's bottom
    shade = w // 8
    rect(g, x0, y0, x0 + w, y0 + h, OFFWHITE)
    rect(g, x0 + w - shade, y0, x0 + w, y0 + h, OFFWHITE_SH)  # the shaded side
    rect(g, x0, y0, x0 + w, y0 + 2, WHITE)
    rect(g, x0, seam, x0 + w, seam + 2, OFFWHITE_SH)
    hx = x0 + w - shade - 6
    for hy0, hy1 in ((y0 + 6, seam - 6), (seam + 8, seam + 8 + (y0 + h - seam) // 2)):
        rect(g, hx, hy0, hx + 3, hy1, STEEL)
    rect(g, x0 + w * 10 // 64, y0 + h - 22, x0 + w * 18 // 64, y0 + h - 14, CYAN)  # magnets
    rect(g, x0 + w * 22 // 64, y0 + h - 20, x0 + w * 28 // 64, y0 + h - 14, BLUE)


def counter(g, x0, x1, y_top, y_bot):
    rect(g, x0, y_top, x1, y_bot, WOOD)
    rect(g, x0, y_top, x1, y_top + 3, WOOD_HI)
    rect(g, x0, y_bot - 12, x1, y_bot, WOOD_SH)
    for cx in range(x0 + 16, x1 - 8, 24):  # cabinet doors and pulls
        rect(g, cx - 1, y_bot - 11, cx + 1, y_bot - 1, WOOD_DK)
        rect(g, cx - 6, y_bot - 8, cx - 3, y_bot - 7, STEEL)


def espresso(g, x0, y0):
    rect(g, x0, y0, x0 + 40, y0 + 30, STEEL)
    rect(g, x0, y0, x0 + 40, y0 + 3, WHITE)
    rect(g, x0 + 34, y0, x0 + 40, y0 + 30, STEEL_SH)
    rect(g, x0 + 6, y0 + 6, x0 + 30, y0 + 12, SHADOW)
    rect(g, x0 + 8, y0 + 7, x0 + 20, y0 + 11, CYAN)  # the display
    rect(g, x0 + 14, y0 + 16, x0 + 24, y0 + 20, SHADOW)  # group head
    rect(g, x0 + 15, y0 + 22, x0 + 23, y0 + 28, MUG)


def microwave(g, x0, y0, w):
    rect(g, x0, y0, x0 + w, y0 + 26, BEZEL)
    rect(g, x0, y0, x0 + w, y0 + 2, SLATE)
    rect(g, x0 + 4, y0 + 5, x0 + w - 16, y0 + 22, INK)  # the window
    rect(g, x0 + 6, y0 + 7, x0 + 16, y0 + 9, DISPLAY_SHEEN)
    rect(g, x0 + w - 12, y0 + 6, x0 + w - 4, y0 + 10, CYAN)
    for by in (13, 17):
        rect(g, x0 + w - 11, y0 + by, x0 + w - 5, y0 + by + 2, KEYCAP)


def bottle(g, x0, y0, col):
    rect(g, x0, y0 + 6, x0 + 6, y0 + 22, col)
    rect(g, x0 + 2, y0, x0 + 4, y0 + 6, col)
    rect(g, x0, y0 + 8, x0 + 1, y0 + 20, WHITE)


def pantry():
    """A fridge, a coffee counter, a microwave counter."""
    g = canvas(32 * S, 10 * S)
    fridge(g, 0, 0, 64, 72)
    counter(g, 80, 160, 28, 72)
    espresso(g, 96, 0)
    rect(g, 140, 18, 150, 28, MUG)
    rect(g, 140, 18, 150, 20, MUG_SH)
    counter(g, 176, 256, 28, 72)
    microwave(g, 184, 2, 48)
    bottle(g, 236, 6, GOLD)
    bottle(g, 246, 6, RED)
    rect(g, 0, 72, 256, 80, T)
    outline(g, {OFFWHITE, OFFWHITE_SH, WHITE, STEEL, STEEL_SH, WOOD, WOOD_HI, WOOD_SH, BEZEL, SLATE, INK}, OUTLINE)
    return g


def pantry_small():
    """A vending machine, a mini fridge, a coffee-and-microwave counter."""
    g = canvas(20 * S, 8 * S)
    rect(g, 0, 0, 24, 48, BEZEL)  # vending machine
    rect(g, 0, 0, 24, 2, SLATE)
    rect(g, 3, 4, 18, 40, INK)
    for r in range(4):
        for c in range(3):
            rect(g, 4 + c * 5, 6 + r * 8, 8 + c * 5, 11 + r * 8, (RED, GOLD, BLUE, LEAF)[(r + c) % 4])
    rect(g, 19, 10, 22, 16, CYAN)
    fridge(g, 32, 0, 24, 48)
    counter(g, 64, 152, 20, 56)
    espresso(g, 68, -4)
    microwave(g, 110, -2, 40)  # within the counter's end
    outline(g, {OFFWHITE, OFFWHITE_SH, WHITE, STEEL, STEEL_SH, WOOD, WOOD_HI, WOOD_SH, BEZEL, SLATE, INK}, OUTLINE)
    return g


def snack_shelf():
    """A wooden rack of colourful snack packets."""
    g = canvas(7 * S, 10 * S)
    rect(g, 0, 0, 56, 72, WOOD)
    rect(g, 4, 4, 52, 60, WOOD_DK)
    rng = random.Random(33)
    for shelf in range(3):
        yb = 20 + shelf * 18
        x = 6
        while x < 48:
            pw = rng.randrange(6, 10)
            ph = rng.randrange(9, 14)
            col = (GOLD, RED, BLUE, LEAF, CYAN, MUG, ORANGE)[rng.randrange(7)]
            rect(g, x, yb - ph, min(50, x + pw), yb, col)
            rect(g, x, yb - ph, min(50, x + pw), yb - ph + 2, WHITE)
            x += pw + 1
        rect(g, 4, yb, 52, yb + 3, WOOD_SH)
        rect(g, 4, yb, 52, yb + 1, WOOD_LT)
    rect(g, 0, 0, 56, 2, WOOD_LT)
    rect(g, 18, 62, 38, 70, WOOD_DK)
    rect(g, 26, 65, 30, 67, STEEL)
    outline(g, {WOOD, WOOD_LT, WOOD_SH, WOOD_DK}, OUTLINE)
    return g


def tv_stand():
    """A lit TV on a slim column and a wide base."""
    g = canvas(10 * S, 10 * S)
    rect(g, 0, 0, 80, 44, SHADOW)
    rect(g, 0, 0, 80, 2, SLATE)
    slide_screen(g, 5, 5, 75, 39)
    rect(g, 36, 44, 44, 64, BEZEL)
    rect(g, 16, 64, 64, 72, WOOD)
    rect(g, 16, 64, 64, 66, WOOD_LT)
    rect(g, 16, 72, 64, 80, WOOD_SH)
    outline(g, {SHADOW, SLATE, BEZEL, WOOD, WOOD_LT, WOOD_SH}, KEY_DK)
    return g


def phone_booth():
    """A glass privacy pod with an occupied LED on the roof."""
    g = canvas(6 * S, 12 * S)
    rect(g, 2, 0, 46, 94, BEZEL)
    rect(g, 2, 0, 46, 3, SLATE)
    rect(g, 18, 3, 30, 8, BULB)  # the LED
    rect(g, 6, 12, 42, 80, CYAN)
    for y in range(12, 80):  # reflections on the glass
        for x in range(6, 42):
            if (x - y * 0.6) % 26 < 3:
                g[y][x] = WHITE
    rect(g, 6, 40, 42, 44, BEZEL)  # the transom
    rect(g, 16, 50, 32, 74, SHADOW)  # the occupant's silhouette seen through it
    rect(g, 6, 80, 42, 92, WOOD)
    rect(g, 36, 84, 39, 88, STEEL)
    outline(g, {BEZEL, SLATE, CYAN, WHITE, WOOD}, OUTLINE)
    return g


def standing_desk():
    """A sit-stand desk with a lit laptop."""
    g = canvas(8 * S, 8 * S)
    rect(g, 0, 0, 64, 34, WOOD)
    rect(g, 0, 0, 64, 2, WOOD_HI)
    rect(g, 0, 30, 64, 34, WOOD_SH)
    rect(g, 12, 6, 52, 22, BEZEL)  # laptop lid, open
    rect(g, 14, 8, 50, 20, CYAN)
    rect(g, 16, 10, 36, 11, WHITE)
    rect(g, 16, 14, 30, 15, WHITE)
    rect(g, 10, 22, 54, 28, KEYCAP)  # keyboard deck
    rect(g, 10, 22, 54, 23, GREY)
    for lx in (6, 54):  # the telescoping legs
        rect(g, lx, 34, lx + 4, 58, STEEL_SH)
        rect(g, lx - 3, 58, lx + 7, 62, KEY_DK)
    outline(g, {WOOD, WOOD_HI, WOOD_SH, BEZEL}, OUTLINE)
    return g


def bulletin_board():
    """A corkboard of pinned notes."""
    g = canvas(10 * S, 6 * S)
    rect(g, 0, 0, 80, 48, WOOD)
    rect(g, 0, 0, 80, 2, WOOD_LT)
    rect(g, 4, 4, 76, 44, CORK)
    rng = random.Random(8)
    for y in range(4, 44):
        for x in range(4, 76):
            if rng.random() < 0.07:
                g[y][x] = CORK_SH
    for x0, y0, c in ((8, 8, RED), (26, 7, GOLD), (48, 9, BLUE), (60, 22, LEAF), (12, 26, GOLD), (34, 24, CYAN)):
        rect(g, x0, y0, x0 + 14, y0 + 13, c)
        rect(g, x0 + 2, y0 + 5, x0 + 11, y0 + 6, PRINT)
        rect(g, x0 + 2, y0 + 8, x0 + 9, y0 + 9, PRINT)
        rect(g, x0 + 6, y0 + 1, x0 + 8, y0 + 3, RED if c != RED else BLUE)  # the pin
    outline(g, {WOOD, WOOD_LT}, OUTLINE)
    return g


def exit_sign():
    """A lit EXIT sign: dark red letters on a warm-lit panel, in a red frame."""
    w, h = 5 * S, 3 * S
    g = canvas(w, h)
    rect(g, 0, 0, w, h, RED)
    rect(g, 3, 3, w - 3, h - 3, BULB)
    glyphs = {
        "E": ("111", "100", "110", "100", "111"),
        "X": ("101", "101", "010", "101", "101"),
        "I": ("1",) * 5,
        "T": ("111", "010", "010", "010", "010"),
    }
    # E X I T: five-row glyphs of 2x2 px cells, centred on the panel
    for x0, letter in zip((7, 15, 23, 27), "EXIT"):
        for r, row in enumerate(glyphs[letter]):
            for c, bit in enumerate(row):
                if bit == "1":
                    rect(g, x0 + c * 2, 7 + r * 2, x0 + c * 2 + 2, 9 + r * 2, DARK_RED)
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
        (pack / "gone@8x.sprite").write_text(f"# {PROVENANCE}\n", encoding=ENCODING)
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
    # typing: the hands alternate like the base frames (rows 9 and 10)
    typing = [
        figure_back(11 * S, [(7, 66), (56, 70)], [(4, 76), (52, 84)]),
        figure_back(11 * S, [(7, 70), (56, 66)], [(59, 76), (11, 84)]),
    ]
    seated = [figure_back(10 * S, [(7, 68), (56, 68)])]
    typing_front = [
        figure_front(11 * S, [(4, 76), (52, 84)]),
        figure_front(11 * S, [(59, 76), (11, 84)]),
    ]
    walk = [standing("front", -1), standing("front", 1)]
    walk_back = [standing("back", -1), standing("back", 1)]
    hold = standing(
        "front",
        0,
        arms=[((12, 50), (11, 60), (25, 64)), ((52, 50), (53, 60), (38, 64))],
        props=[mug(31, 62)],
    )
    coffee_walk = [
        standing(
            "front",
            stride,
            arms=[((12, 50), (9, 60), (8, 67)), ((52, 50), (54, 58), (44, 58))],
            props=[mug(40, 54)],
        )
        for stride in (-1, 1)
    ]
    pieces = {
        "meeting_screen": ("A wall-mounted meeting screen showing a slide: a title bar, bullets\nand a bar chart.", [meeting_screen()]),
        "pantry": ("The office pantry: a fridge, an espresso counter, a microwave counter\nwith bottles.", [pantry()]),
        "pantry_small": ("The compact pantry: a vending machine, a mini fridge, a\ncoffee-and-microwave counter.", [pantry_small()]),
        "snack_shelf": ("A snack rack.", [snack_shelf()]),
        "tv_stand": ("A lit TV on a stand.", [tv_stand()]),
        "phone_booth": ("A glass privacy pod, its occupied LED on the roof.", [phone_booth()]),
        "standing_desk": ("A sit-stand desk: a lit laptop on telescoping legs.", [standing_desk()]),
        "bulletin_board": ("A corkboard of pinned notes.", [bulletin_board()]),
        "exit_sign": ("A lit EXIT sign.", [exit_sign()]),
        "plant": ("A potted plant: broad leaves over a terracotta pot.", [plant("plant")]),
        "plant_tall": ("A tall office fig in its dark pot.", [plant("plant_tall")]),
        "plant_flower": ("Potted flowers.", [plant("plant_flower")]),
        "plant_succulent": ("A succulent rosette.", [plant("plant_succulent")]),
        "whiteboard": ("A mobile whiteboard: a sprint board sketched on it, a marker tray, a\nwheeled stand.", [whiteboard()]),
        "bookshelf": ("A bookshelf: three shelves of books, a drawer below.", [bookshelf()]),
        "meeting_sofa": ("A three-seat sofa: backrest to the north, tufted cushions, arms.", [meeting_sofa()]),
        "seated_sleeping": ("Asleep face-down on folded arms: bare forearms crossed under the\ncrown of the head, a hand at each elbow.", [asleep(False)]),
        "seated_sleeping_alt": ("Dozed off, slumped east.", [asleep(True)]),
        "side_seated": ("Seated in profile.", [profile()]),
        "back_couch": ("On the couch, facing the windows: the back-view figure with its hands\nin its lap, out of sight.", [figure_back(9 * S, [])]),
        "standing": ("A standing figure: the front-view head on a standing body.", [standing("front", 0)]),
        "walking_0": ("Walking, frame 0: left foot out.", [walk[0]]),
        "walking_1": ("Walking, frame 1: right foot out.", [walk[1]]),
        "walking_back_0": ("Walking away, frame 0.", [walk_back[0]]),
        "walking_back_1": ("Walking away, frame 1.", [walk_back[1]]),
        "holding_coffee": ("Standing with a mug in both hands.", [hold]),
        "walking_coffee_0": ("Walking with a mug, frame 0.", [coffee_walk[0]]),
        "walking_coffee_1": ("Walking with a mug, frame 1.", [coffee_walk[1]]),
        "typing_0": ("Front view, typing, frame 0: the west hand up, the east down.", [typing_front[0]]),
        "typing_1": ("Front view, typing, frame 1: the hands swap.", [typing_front[1]]),
        "seated": ("Front view at rest: both hands level.", [figure_front(10 * S, [(12, 76), (51, 76)])]),
        "desk": ("The viewer-facing desk: the monitor turns its BACK to us (casing, vents,\nbadge), lamp and papers on the wings.", [desk_south()]),
        "typing_back_0": ("Back view, typing, frame 0: the west elbow up, the east down.", [typing[0]]),
        "typing_back_1": ("Back view, typing, frame 1: the elbows swap.", [typing[1]]),
        "seated_back": ("Back view at rest: both elbows level.", seated),
        "desk_chair": ("The task chair from behind: a tufted four-column back, armrests and a\nfive-star base.", [chair()]),
        "desk_north": ("The back-turned desk: its raised monitor, with a desk lamp, papers and\na mug on the wings.", [desk_north()]),
    }
    classic = {
        "desk": ("The viewer-facing desk: the monitor's back on its stand, the wood lit\nalong its back edge and front lip.", [desk_south_1x()]),
        "desk_north": ("The back-turned desk: its raised monitor's glass with dim text, a\nkeyboard before the stand.", [desk_north_1x()]),
    }
    for base, (_, frames) in classic.items():
        if base in pieces:
            small, big = frames[0], pieces[base][1][0]
            assert (len(big[0]), len(big)) == (S * len(small[0]), S * len(small)), base
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
