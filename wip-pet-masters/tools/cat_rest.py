"""The cat's rest poses with their loops, hand-placed (4 frames or fewer).

cat_sit: B's sit (the walk's own head), facing west as a sitting pet shows;
a breathing back and, once a loop, a tail flick.
cat_sleep: curled up, eyes shut, the tail wrapped round the front; breathing.
"""
import pathlib

HERE = pathlib.Path(__file__).parent
OUT = HERE / "rest"
OUT.mkdir(exist_ok=True)


def rows(path):
    return [l.split() for l in path.read_text(encoding="utf-8").splitlines() if l and not l.startswith(("@", "#"))]


def emit(name, g):
    (OUT / f"{name}.sprite").write_text("@frame 0\n" + "\n".join(" ".join(r) for r in g) + "\n", encoding="utf-8")


def copy(g):
    return [r[:] for r in g]


def outline(g):
    h, w = len(g), len(g[0])
    fill = {(y, x) for y in range(h) for x in range(w) if g[y][x] not in ".κ"}
    for y, x in fill:
        for dy, dx in ((-1, 0), (1, 0), (0, -1), (0, 1)):
            yy, xx = y + dy, x + dx
            if 0 <= yy < h and 0 <= xx < w and g[yy][xx] == ".":
                g[yy][xx] = "κ"


# ---- sit ------------------------------------------------------------------
SIT = rows(HERE.parent / "cat_B.sprite")
# the locked eye: 2x3 with its catch-light top-east (the sprite faces west, so
# it sits where the walk's lands once the walk turns west), and the blush
SIT[7][4] = "t"
for r in (6, 7, 8):
    for c in (5, 6):
        SIT[r][c] = "e"
SIT[6][6] = "w"
SIT[9][7] = "^"


def breathe(g):
    """The back one pixel fuller: each row's east outline steps out a column."""
    g = copy(g)
    for r in range(13, 19):
        east = max(c for c, k in enumerate(g[r]) if k == "κ")
        g[r][east] = "ţ"
        if east + 1 < len(g[r]) and g[r][east + 1] == ".":
            g[r][east + 1] = "κ"
    return g


def flick(g):
    """The tail's hooked tip pulled a column toward the body."""
    g = copy(g)
    for r in (4, 5, 6):
        tip = g[r][19:24]
        g[r][18:23] = tip
        g[r][23] = "."
    return g


sit0 = SIT
sit1 = breathe(SIT)
sit2 = flick(breathe(SIT))
sit3 = flick(SIT)
for i, g in enumerate((sit0, sit1, sit2, sit3)):
    emit(f"cat_sit_{i}", g)

# ---- sleep ----------------------------------------------------------------
W, H = 24, 16
BODY = {  # the loaf behind the head, rounded
    6: (13, 20), 7: (11, 21), 8: (10, 22), 9: (10, 22), 10: (10, 22),
    11: (10, 22), 12: (10, 22), 13: (11, 21), 14: (13, 19),
}


def sleep_frame(breath):
    g = [["."] * W for _ in range(H)]
    for r, (a, b) in BODY.items():
        for c in range(a, b + 1):
            g[r][c] = "Ţ" if r == 6 else ("ţ" if r >= 13 else "t")
    if breath:  # the back rises with the breath
        for c in range(14, 20):
            g[5][c] = "Ţ"
        for c in range(13, 21):
            g[6][c] = "t"
    for c in (12, 15, 18):  # the tabby's stripes over the back
        g[7][c] = "ţ"
    # the head, B's own, lowered to rest on the front; its eyes shut
    head = [r[:15] for r in SIT[1:12]]
    for r, c in ((5, 5), (5, 6), (6, 5), (6, 6), (7, 5), (7, 6), (6, 10), (7, 10), (7, 11)):
        head[r][c] = "t"
    head[5][6] = "t"
    for c in (4, 5, 6):
        head[7][c] = "u"  # a shut eye's lid line
    head[7][10] = "u"
    for r, row in enumerate(head):
        for c, k in enumerate(row):
            if k not in ".κ":
                g[r + 3][c] = k
    # the tail round the front, its dark tip by the nose
    for c in range(9, 22):
        g[14][c] = "t" if c > 10 else "u"
    g[13][21] = "t"
    outline(g)
    return g


for i, breath in enumerate((False, True, True, False)):
    emit(f"cat_sleep_{i}", sleep_frame(breath))
print("ok cat rest")
