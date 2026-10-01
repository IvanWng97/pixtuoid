"""Dress the locked torso in the rig's motion: one 32x24 key grid per frame.

compose.py TORSO_SPRITE ID_DIR OUT_DIR

Per frame: the rig's ID map read back to master pixels (a 4x4 majority), the
far legs and the tail under the torso's fur but over its outline, the locked
torso raised by the frame's bob, the near legs over all, then one outline.
"""
import pathlib
import sys

from PIL import Image

W, H, OVER = 32, 24, 4
IDS = {
    (255, 0, 0): "leg_near",
    (0, 255, 0): "leg_far",
    (255, 255, 0): "paw_near",
    (0, 255, 255): "paw_far",
    (0, 0, 255): "tail",
    (255, 0, 255): "tail_tip",
}
SPECIES = {
    "cat": ({"leg_near": "t", "paw_near": "w", "leg_far": "ţ", "paw_far": "ţ", "tail": "t", "tail_tip": "u"}, "Ţ"),
    "dog": ({"leg_near": "x", "paw_near": "Ƭ", "leg_far": "ƭ", "paw_far": "ƭ", "tail": "x", "tail_tip": "x"}, "Ƭ"),
}
KEY, LIT = SPECIES["cat"]
UNDER = {"leg_far", "paw_far", "tail", "tail_tip"}


def rows(path):
    return [l.split() for l in open(path, encoding="utf-8").read().splitlines() if l and not l.startswith(("@", "#"))]


def parts(path):
    im = Image.open(path).convert("RGBA")
    out = [[None] * W for _ in range(H)]
    for y in range(H):
        for x in range(W):
            votes = {}
            for j in range(OVER):
                for i in range(OVER):
                    r, g, b, a = im.getpixel((x * OVER + i, y * OVER + j))
                    if a < 128:
                        continue
                    k = IDS.get((round(r / 255) * 255, round(g / 255) * 255, round(b / 255) * 255))
                    if k:
                        votes[k] = votes.get(k, 0) + 1
            if votes and max(votes.values()) * 2 >= OVER * OVER:
                out[y][x] = max(votes, key=votes.get)
    return out


def compose(torso, ids, bob):
    g = [["."] * W for _ in range(H)]
    for y in range(H):
        for x in range(W):
            if ids[y][x] in UNDER:
                g[y][x] = KEY[ids[y][x]]
    # the tail lit along its upper-west edge, as the locked tail was
    for y in range(H):
        for x in range(W):
            if ids[y][x] == "tail" and (x == 0 or ids[y][x - 1] is None):
                g[y][x] = LIT
    lift = round(-bob)
    for y in range(H):
        for x in range(W):
            k = torso[y][x]
            ty = y - lift
            if k == "." or not 0 <= ty < H:
                continue
            if k == "κ" and g[ty][x] != ".":
                continue  # a far leg or the tail shows over the torso's outline
            g[ty][x] = k
    for y in range(H):
        for x in range(W):
            if ids[y][x] in ("leg_near", "paw_near"):
                g[y][x] = KEY[ids[y][x]]
    fill = {(y, x) for y in range(H) for x in range(W) if g[y][x] not in ".κ"}
    for y, x in fill:
        for dy, dx in ((-1, 0), (1, 0), (0, -1), (0, 1)):
            yy, xx = y + dy, x + dx
            if 0 <= yy < H and 0 <= xx < W and g[yy][xx] == ".":
                g[yy][xx] = "κ"
    return g


def main():
    global KEY, LIT
    species = sys.argv[1]
    KEY, LIT = SPECIES[species]
    torso_path, id_dir, out = sys.argv[2], pathlib.Path(sys.argv[3]), pathlib.Path(sys.argv[4])
    out.mkdir(parents=True, exist_ok=True)
    torso = rows(torso_path)
    if species == "cat":
        # the back's line over the rump, where the stub tail rose
        for x in range(8, 16):
            torso[9][x] = "κ"
    frames = sorted(id_dir.glob("id_*.png"))
    for i, f in enumerate(frames):
        bob = float((id_dir / f"bob_{i:02d}.txt").read_text())
        g = compose(torso, parts(f), bob)
        (out / f"{species}_walk_{i}.sprite").write_text(
            "@frame 0\n" + "\n".join(" ".join(r) for r in g) + "\n", encoding="utf-8"
        )
    print("composed", len(frames))


main()
