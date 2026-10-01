"""Alternative sits, a proper sitting silhouette: the haunch down on the floor,
the front legs straight and upright, each species' own tail. Facing west as a
resting pet shows. Hand-placed spans; 4-frame loops as before."""
import pathlib

HERE = pathlib.Path(__file__).parent
OUT = HERE / "sit_alt"
OUT.mkdir(exist_ok=True)


def rows(path):
    return [l.split() for l in path.read_text(encoding="utf-8").splitlines() if l and not l.startswith(("@", "#"))]


def blank(w, h):
    return [["."] * w for _ in range(h)]


def spans(g, rows_):
    for r, segs in rows_.items():
        for a, b, k in segs:
            for c in range(a, b + 1):
                g[r][c] = k


def outline(g):
    h, w = len(g), len(g[0])
    fill = {(y, x) for y in range(h) for x in range(w) if g[y][x] not in ".κ"}
    for y, x in fill:
        for dy, dx in ((-1, 0), (1, 0), (0, -1), (0, 1)):
            yy, xx = y + dy, x + dx
            if 0 <= yy < h and 0 <= xx < w and g[yy][xx] == ".":
                g[yy][xx] = "κ"


def emit(name, g):
    (OUT / f"{name}.sprite").write_text("@frame 0\n" + "\n".join(" ".join(r) for r in g) + "\n", encoding="utf-8")


# ---- the cat: B's head, upright chest, front legs, a round haunch, the tail
# wrapped along the floor to the front paws ------------------------------------
B = rows(HERE.parent / "cat_B.sprite")
CAT_HEAD = [r[:13] for r in B[1:12]]  # B's head without its tail
for r in (6, 7, 8):  # the locked eye, its catch-light top-east (faces west)
    for c in (5, 6):
        CAT_HEAD[r - 1][c] = "e"
CAT_HEAD[5][6] = "w"
CAT_HEAD[6][4] = "t"
CAT_HEAD[8][7] = "^"


def cat_sit(breath, flick):
    g = blank(24, 24)
    # the haunch, round and down on the floor at the back
    spans(g, {
        13: [(10, 15, "Ţ")], 14: [(9, 17, "t")], 15: [(8, 18, "t")], 16: [(8, 19, "t")],
        17: [(8, 19, "t")], 18: [(8, 19, "t")], 19: [(8, 19, "ţ")], 20: [(9, 18, "ţ")],
    })
    for r, c in ((14, 13), (15, 15), (16, 17)):  # the tabby's stripes round it
        g[r][c] = "ţ"
    # the chest, upright under the head, white down the front
    chest_w = 9 if breath else 8
    spans(g, {r: [(3, chest_w, "t")] for r in range(11, 17)})
    spans(g, {r: [(3, 5, "w")] for r in range(11, 16)})
    # the front legs, straight down to white paws
    spans(g, {r: [(6, 7, "ţ")] for r in range(17, 21)})
    spans(g, {21: [(6, 7, "ţ")]})
    spans(g, {r: [(3, 4, "t")] for r in range(17, 21)})
    spans(g, {21: [(2, 5, "w")]})
    # the tail along the floor round the side, its dark tip curling up by the
    # paws, an outline apart from the far paw so it reads at 1:1
    spans(g, {21: [(11, 19, "t")], 20: [(19, 20, "t")]})
    for r, c in (((21, 9), (21, 10), (20, 9), (20, 10)) if not flick else ((21, 10), (20, 9), (20, 10), (19, 9), (19, 10))):
        g[r][c] = "u"
    for r, row in enumerate(CAT_HEAD):
        for c, k in enumerate(row):
            if k != ".":
                g[r + 1][c] = k
    outline(g)
    g[21][8] = "κ"  # the line between the far paw and the tail's tip
    return g


for i, (b, f) in enumerate(((False, False), (True, False), (True, True), (False, True))):
    emit(f"cat_sit_{i}", cat_sit(b, f))

# ---- the dog: the walk's head, upright chest, thick front legs, the haunch
# down, the stub tail out behind on the floor -----------------------------------
DOG_HEAD = [
    "....ƬƬƬƬƬ......",
    "..ƬƬƬƬƬƬƬƬƬ....",
    ".ƬƬƬƬƬƬƬƬƬƬƬ...",
    "zzzzxxxxxwexx..",
    "zzzzzxxxxeexƬƬu",
    "zzzzzxxxxeexƬƬu",
    ".zzzzxx^xxxƬƬƬ.",
    "..zzzxxxxxxxR..",
    "....ƭƭƭƭƭƭƭ....",
]


def dog_sit(breath, wag):
    g = blank(24, 24)  # drawn facing east, turned at the end
    # the haunch down at the back, round
    spans(g, {
        13: [(3, 8, "Ƭ")], 14: [(2, 10, "x")], 15: [(1, 10, "x")], 16: [(1, 10, "x")],
        17: [(1, 10, "x")], 18: [(1, 10, "x")], 19: [(1, 10, "ƭ")], 20: [(2, 9, "ƭ")],
    })
    spans(g, {21: [(4, 9, "ƭ")]})  # the hind paw under it
    # the chest, upright and deep under the head
    front = 18 if breath else 17
    spans(g, {r: [(10, front, "x")] for r in range(10, 16)})
    spans(g, {16: [(12, front, "ƭ")]})
    # the front legs, thick and straight, clear of the haunch, the far one behind
    spans(g, {r: [(12, 14, "ƭ")] for r in range(17, 21)})
    spans(g, {21: [(12, 15, "ƭ")]})
    spans(g, {r: [(15, 17, "x")] for r in range(17, 21)})
    spans(g, {21: [(15, 18, "Ƭ")]})
    # the stub tail out behind, on the floor, wagging up
    for r, c in (((18, 0), (18, 1)) if not wag else ((17, 0), (18, 1))):
        g[r][c] = "x"
    for r, row in enumerate(DOG_HEAD):
        for c, k in enumerate(row):
            if k != ".":
                g[r + 2][c + 8] = k
    outline(g)
    return [list(reversed(r)) for r in g]


for i, (b, w) in enumerate(((False, False), (True, True), (True, False), (False, True))):
    emit(f"dog_sit_{i}", dog_sit(b, w))
print("ok sit alt")
