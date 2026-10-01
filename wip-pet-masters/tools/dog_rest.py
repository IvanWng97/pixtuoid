"""The dog's rest poses with their loops, hand-placed (4 frames or fewer).
Drawn facing east with the walk's head (dog C), then turned west as a resting
pet shows. dog_sit: sitting, the chest breathing, the stub tail wagging on the
floor. dog_sleep: lying with its chin on its paws, eye shut, breathing.
"""
import pathlib

HERE = pathlib.Path(__file__).parent
OUT = HERE / "rest"
OUT.mkdir(exist_ok=True)

# the walk's head (dogC.py's HEAD), 15 wide
HEAD = [
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


def emit(name, g):
    g = [list(reversed(r)) for r in g]  # shown facing west
    (OUT / f"{name}.sprite").write_text("@frame 0\n" + "\n".join(" ".join(r) for r in g) + "\n", encoding="utf-8")


def blank(w, h):
    return [["."] * w for _ in range(h)]


def put(g, r0, c0, rows):
    for dr, row in enumerate(rows):
        for dc, k in enumerate(row):
            if k != ".":
                g[r0 + dr][c0 + dc] = k


def spans(g, rows):
    for r, segs in rows.items():
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


# ---- sit (24x24) -------------------------------------------------------------
def sit(breath, wag):
    g = blank(24, 24)
    # the haunch down on the floor, round at the back
    spans(g, {
        13: [(6, 11, "Ƭ")], 14: [(5, 13, "x")], 15: [(4, 14, "x")], 16: [(4, 14, "x")],
        17: [(4, 14, "x")], 18: [(4, 14, "x")], 19: [(4, 13, "ƭ")], 20: [(5, 12, "ƭ")],
    })
    # the far hind paw, and the chest held up under the head
    spans(g, {21: [(6, 9, "ƭ")]})
    chest = 19 if breath else 18
    for r in range(10, 20):
        spans(g, {r: [(11, chest, "x")]})
    # the front legs, straight, near over far
    spans(g, {r: [(13, 15, "ƭ")] for r in range(17, 21)})
    spans(g, {21: [(13, 16, "ƭ")]})
    spans(g, {r: [(16, 18, "x")] for r in range(17, 21)})
    spans(g, {21: [(16, 19, "Ƭ")]})
    # the stub tail on the floor behind, wagging up
    tail = [(19, 1), (19, 2), (19, 3)] if not wag else [(18, 1), (18, 2), (19, 3)]
    for r, c in tail:
        g[r][c] = "x"
        g[r - 1][c] = "Ƭ"
    put(g, 1, 8, HEAD)
    outline(g)
    return g


for i, (b, w) in enumerate(((False, False), (True, True), (True, False), (False, True))):
    emit(f"dog_sit_{i}", sit(b, w))


# ---- sleep (24x16) -------------------------------------------------------------
SLEEP_HEAD = [row[:] for row in HEAD]
SLEEP_HEAD = [list(r) for r in SLEEP_HEAD]
for r, c in ((3, 9), (3, 10), (4, 9), (4, 10), (5, 9), (5, 10)):
    SLEEP_HEAD[r][c] = "x"
for c in (9, 10, 11):
    SLEEP_HEAD[5][c] = "u"  # a shut eye's lid line
SLEEP_HEAD[7][12] = "x"  # the tongue in


def sleep(breath):
    g = blank(24, 16)
    top = 6 if breath else 7
    spans(g, {top: [(3, 13, "Ƭ")]})
    for r in range(top + 1, 14):
        spans(g, {r: [(2, 15, "x")]})
    spans(g, {13: [(2, 15, "ƭ")], 14: [(3, 14, "ƭ")]})
    # the front paws stretched out, the chin on them
    spans(g, {14: [(14, 22, "x")], 15: [(14, 22, "Ƭ")]})
    # the tail curled along the back of the body
    for c in (0, 1):
        g[12][c] = "x"
        g[11][c] = "Ƭ"
    put(g, 6, 8, SLEEP_HEAD)
    outline(g)
    return g


for i, b in enumerate((False, True, True, False)):
    emit(f"dog_sleep_{i}", sleep(b))
print("ok dog rest")
