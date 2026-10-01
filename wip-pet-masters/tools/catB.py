import pathlib, sys
OUT = pathlib.Path(sys.argv[1])
OUT.mkdir(exist_ok=True)
VARIANT = sys.argv[2]

# cat_walk, B style: authored facing west like the B sit, emitted facing east
# (the sim flips a walk sprite when it heads west).
def pad(rows, w):
    out = []
    for r in rows:
        r = list(r)
        assert len(r) <= w, "".join(r)
        out.append(r + ["."] * (w - len(r)))
    return out


WALK_BODY = [
    "................................",
    "................................",
    "....κ.....κ.....................",
    "...κuκ...κuκ....................",
    "...κu^κκκκ^uκ...................",
    "..κŢŢŢŢŢuŢŢŢκ...................",
    "..κttttuttutttκ.........κκ......",
    ".κttteettttttţκ........κuuκ.....",
    ".κttwettttettţκ........κtuκ.....",
    ".κttteettttettţκκκκκκκκκtκ......",
    ".κ^wwttttttttţtŢŢŢŢŢŢŢŢttκ......",
    ".κwuwwtttttttţttttttttttttκ.....",
    "..κwwwtttttţţttttttttttttţκ.....",
    "...κwwwttttttttttttttttttţţκ....",
    "....κwwttttttttttttttttttţţκ....",
    "....κwtttttttttttttttttţţţκ.....",
    ".....κţţţţţţţţţţţţţţţţţţţκ......",
    "......κκκκκκκκκκκκκκκκκκκ.......",
    "................................",
    "................................",
    "................................",
    "................................",
    "................................",
    "................................",
]


def leg(g, cols, fur, paw):
    """A 2-px leg from the belly down; `cols` gives its west column per row
    17..21, so a slant is a column shift."""
    for i, c in enumerate(cols):
        r = 17 + i
        g[r][c] = g[r][c + 1] = paw if r == 21 else fur
        for side in (c - 1, c + 2):
            if g[r][side] == ".":
                g[r][side] = "κ"
    c = cols[-1]
    g[22][c] = g[22][c + 1] = "κ"


# a chibi's body is short: three columns out of the middle, re-centred
SHORT_BODY = ["." + r[:17] + r[20:] + "." * 2 for r in WALK_BODY]


# The tail: two fur cells a row from the rump up, curving east then hooking
# back west, the top row its dark tip; `sway` leans the upper half east.
def tail_cells(sway):
    lean = [0, 0, 0, 0, sway, sway, sway, sway]
    base = [(9, 21), (8, 22), (7, 23), (6, 24), (5, 24), (4, 24), (3, 23), (2, 22)]
    return [(r, c + d) for (r, c), d in zip(base, lean)]


def tail(g, sway):
    # clear the stub the body template carries
    for r in range(0, 9):
        for c in range(17, 32):
            g[r][c] = "."
    g[9][22] = "."
    g[9][23] = "."
    cells = tail_cells(sway)
    for i, (r, c) in enumerate(cells):
        tip = i == len(cells) - 1
        g[r][c] = "u" if tip else "t"
        g[r][c + 1] = "u" if tip else "ţ"
    fur = {(r, c + k) for r, c in cells for k in (0, 1)}
    for r, c in fur:
        for dr, dc in ((-1, 0), (1, 0), (0, -1), (0, 1)):
            rr, cc = r + dr, c + dc
            if (rr, cc) not in fur and g[rr][cc] == ".":
                g[rr][cc] = "κ"
    # the body's top outline meets the tail's base
    g[9][20] = "κ"


def eyes_sized(g, rows):
    # the near eye 2 wide and `rows` tall where v1's broken one was, its
    # catch-light top-west once mirrored; the blush below and behind it
    g[8][5] = "t"
    for r in range(7, 7 + rows):
        for c in (6, 7):
            g[r][c] = "e"
    for r in range(7 + rows, 10):
        for c in (6, 7):
            g[r][c] = "t"
    g[7][7] = "w"
    g[10][8] = "^"


def eyes(g):
    # the near eye a round 3x3 with its catch-light top-west once mirrored,
    # where v1 had a broken 2-wide; a blush below and behind it
    g[8][5] = "t"
    for r in (7, 8, 9):
        for c in (5, 6, 7):
            g[r][c] = "e"
    g[7][7] = "w"
    g[10][8] = "^"


def walk(legs, sway=0):
    g = pad([r[:32] for r in SHORT_BODY], 32)
    if VARIANT == "torso":
        # the locked head and body alone, for the multi-frame walk to dress
        tail(g, sway)
        for r in range(0, 10):
            for c in range(17, 32):
                g[r][c] = "."
        g[9][20] = "κ"
        eyes_sized(g, 3)
        return [list(reversed(r)) for r in g]
    if VARIANT != "v1":
        tail(g, sway)
    if VARIANT == "eyes":
        eyes(g)
    if VARIANT == "e23":
        eyes_sized(g, 3)
    if VARIANT == "e22":
        eyes_sized(g, 2)
    for cols, fur, paw in legs:
        leg(g, cols, fur, paw)
    return [list(reversed(r)) for r in g]


def emit(name, g):
    (OUT / f"{name}.sprite").write_text(
        "@frame 0\n" + "\n".join(" ".join(r) for r in g) + "\n", encoding="utf-8"
    )


# far legs first, near legs over them

walk0 = walk([
    ([10, 10, 10, 10, 10], "ţ", "w"),
    ([18, 18, 18, 18, 18], "ţ", "ţ"),
    ([7, 7, 6, 6, 5], "t", "w"),
    ([20, 20, 21, 21, 22], "t", "w"),
])
walk1 = walk([
    ([9, 9, 9, 9, 9], "ţ", "w"),
    ([17, 17, 17, 17, 17], "ţ", "ţ"),
    ([7, 7, 7, 7, 7], "t", "w"),
    ([20, 20, 20, 20, 20], "t", "w"),
], sway=1)
emit("cat_walk_0", walk0)
emit("cat_walk_1", walk1)
