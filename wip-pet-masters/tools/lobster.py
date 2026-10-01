"""The OpenClaw lobster, chibi, top-down at 4x (56x48): a big round shell,
eyes on stalks, two big claws, a segmented tail fanning out, antennae, three
small legs a side. lobster_rest: the claws snapping in turn, the antennae
swaying. lobster_walk: the legs stepping in turn, the claws and tail swaying.
Every frame placed on the grid; shapes are hard-edged spans."""
import math
import pathlib

HERE = pathlib.Path(__file__).parent
OUT = HERE / "rest"
OUT.mkdir(exist_ok=True)
W, H = 56, 48
SHELL, SHELL_SH, SHELL_LT, SEAM = "o", "ȼ", "Ȼ", "N"
CLAW, CLAW_SH, CLAW_LT = "A", "ǩ", "Ǩ"
GLOSS, FEELER, EYE, SHINE = "Q", "a", "e", "w"


def blank():
    return [["."] * W for _ in range(H)]


def disc(g, cx, cy, rx, ry, key, lit=None, shade=None):
    """A hard-edged ellipse, lit up-west and shaded down-east when asked."""
    for y in range(H):
        for x in range(W):
            dx, dy = (x + 0.5 - cx) / rx, (y + 0.5 - cy) / ry
            d = dx * dx + dy * dy
            if d <= 1.0:
                k = key
                if lit and dx + dy < -0.9 and d > 0.25:
                    k = lit
                elif shade and dx + dy > 0.8 and d > 0.35:
                    k = shade
                g[y][x] = k


def line(g, a, b, key, width=1):
    (ax, ay), (bx, by) = a, b
    n = int(max(abs(bx - ax), abs(by - ay)) * 2) + 1
    for i in range(n + 1):
        t = i / n
        x, y = ax + (bx - ax) * t, ay + (by - ay) * t
        for ox in range(width):
            for oy in range(width):
                xx, yy = int(x) + ox, int(y) + oy
                if 0 <= xx < W and 0 <= yy < H:
                    g[yy][xx] = key


def outline(g):
    fill = {(y, x) for y in range(H) for x in range(W) if g[y][x] not in ".κ"}
    for y, x in fill:
        for dy, dx in ((-1, 0), (1, 0), (0, -1), (0, 1)):
            yy, xx = y + dy, x + dx
            if 0 <= yy < H and 0 <= xx < W and g[yy][xx] == ".":
                g[yy][xx] = "κ"


def claw(g, side, open_, lift):
    """One claw: an arm from the shell out and up to a big pincer whose two
    fingers part by `open_` pixels; `lift` raises it."""
    s = -1 if side == "west" else 1
    cx, cy = 28 + s * 17, 13 - lift
    line(g, (28 + s * 7, 22), (cx - s * 2, cy + 6), CLAW_SH, width=3)
    disc(g, cx, cy + 3, 6.0, 6.0, CLAW, CLAW_LT, CLAW_SH)
    # the two fingers, a gap between them that opens and shuts
    for f, off in ((-1, -open_ / 2), (1, open_ / 2)):
        fx = cx + f * 3 + off
        disc(g, fx, cy - 4, 2.2, 4.0, CLAW, CLAW_LT if f < 0 else None, CLAW_SH if f > 0 else None)
    for y in range(int(cy - 8), int(cy - 1)):
        for x in range(int(cx - open_ / 2), int(cx + open_ / 2 + 1)):
            if 0 <= y < H and 0 <= x < W and open_ > 0:
                g[y][x] = "."


def lobster(open_w, open_e, sway, legs, lift_w=0, lift_e=0, tail=0):
    g = blank()
    # antennae, long and swaying, from the head up and out
    for s in (-1, 1):
        base = (28 + s * 3, 12)
        tip = (28 + s * (14 + sway * s), 1)
        mid = (28 + s * 8, 4)
        line(g, base, mid, FEELER)
        line(g, mid, tip, FEELER)
    # three small legs a side, stepping in turn
    for s in (-1, 1):
        for i, y in enumerate((24, 28, 32)):
            step = legs[(i + (s > 0)) % 2]
            line(g, (28 + s * 6, y), (28 + s * 12, y + 2 + step), SEAM, width=2)
    # the claws, their arms tucked under the shell
    claw(g, "west", open_w, lift_w)
    claw(g, "east", open_e, lift_e)
    # the tail: segments narrowing, then the fan
    for i, (y, rx) in enumerate(((33, 6.0), (37, 5.2), (40.5, 4.5))):
        disc(g, 28 + tail * (i > 0), y, rx, 2.6, SHELL, SHELL_LT, SHELL_SH)
    for fx in (-4, 0, 4):
        disc(g, 28 + tail + fx, 45, 2.5, 2.6, SHELL, None, SHELL_SH)
    # the shell, big and round, glossed up-west, seamed across
    disc(g, 28, 20, 8.5, 11, SHELL, SHELL_LT, SHELL_SH)
    disc(g, 24.5, 15.5, 2.0, 2.0, GLOSS)
    for x in range(22, 35):
        if g[27][x] == SHELL:
            g[27][x] = SEAM
    # eyes on stalks, big and dark, each with its catch-light
    for s in (-1, 1):
        line(g, (28 + s * 3, 12), (28 + s * 5, 9), SEAM, width=2)
        disc(g, 28 + s * 5.5, 8.5, 2.2, 2.2, EYE)
        g[7][int(28 + s * 5.5) - 1] = SHINE
    outline(g)
    return g


def emit(name, g):
    (OUT / f"{name}.sprite").write_text("@frame 0\n" + "\n".join(" ".join(r) for r in g) + "\n", encoding="utf-8")


# rest: the west claw opens and snaps, then the east; the antennae sway
REST = [
    dict(open_w=0, open_e=0, sway=0, legs=(0, 0)),
    dict(open_w=3, open_e=0, sway=1, legs=(0, 0), lift_w=1),
    dict(open_w=0, open_e=3, sway=0, legs=(0, 0), lift_e=1),
    dict(open_w=1, open_e=1, sway=-1, legs=(0, 0)),
]
for i, kw in enumerate(REST):
    emit(f"lobster_rest_{i}", lobster(**kw))
# walk: legs step in turn, the claws bob, the tail sways
WALK = [
    dict(open_w=1, open_e=1, sway=0, legs=(0, 1), lift_w=1, tail=0),
    dict(open_w=1, open_e=1, sway=1, legs=(1, 0), lift_e=1, tail=1),
    dict(open_w=1, open_e=1, sway=0, legs=(0, 1), lift_w=1, tail=0),
    dict(open_w=1, open_e=1, sway=-1, legs=(1, 0), lift_e=1, tail=-1),
]
for i, kw in enumerate(WALK):
    emit(f"lobster_walk_{i}", lobster(**kw))
print("ok lobster")
