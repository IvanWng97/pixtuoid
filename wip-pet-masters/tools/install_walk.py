"""Install an N-frame walk into the scratch render worktree's pack.

install_walk.py PACK_DIR ANIM FRAMES_DIR FRAME_MS

Writes <anim>_<i>@4x.sprite from FRAMES_DIR (grounded together, as gen-art
grounds), <anim>_<i>.sprite read at 1x by gen-art's creature reading (no
fixes), and points pack.toml's <anim> and <anim>@4x at them.
"""
import pathlib
import re
import sys

pack, anim, src, ms = pathlib.Path(sys.argv[1]), sys.argv[2], pathlib.Path(sys.argv[3]), int(sys.argv[4])
base_dir = pathlib.Path(sys.argv[5]) if len(sys.argv) > 5 else None
S = 4
FOLD = {"ţ": "t", "Ţ": "t", "ƭ": "x", "Ƭ": "x"}


def rows(p):
    return [l.split() for l in p.read_text(encoding="utf-8").splitlines() if l and not l.startswith(("@", "#"))]


def grounded(frames):
    blank = min(next((i for i, r in enumerate(reversed(g)) if any(c != "." for c in r)), 0) for g in frames)
    return [[["."] * len(g[0])] * blank + g[: len(g) - blank] for g in frames]


def read(g):
    h, w = len(g) // S, len(g[0]) // S
    out = []
    for by in range(h):
        r = []
        for bx in range(w):
            cells = [g[by * S + y][bx * S + x] for y in range(S) for x in range(S)]
            drawn = [k for k in cells if k != "."]
            inside = [FOLD.get(k, k) for k in drawn if k != "κ"]
            if len(drawn) * 8 < len(cells) * 3 or not inside:
                r.append(".")
            elif cells.count("e") >= 3:
                r.append("e")
            else:
                r.append(max(dict.fromkeys(inside), key=inside.count))
        out.append(r)
    return out


def write(path, g):
    path.write_text("@frame 0\n" + "\n".join(" ".join(r) for r in g) + "\n", encoding="utf-8")


frames = grounded([rows(p) for p in sorted(src.glob(f"{anim}_*.sprite"), key=lambda p: int(p.stem.rsplit("_", 1)[1]))])
for i, g in enumerate(frames):
    write(pack / f"{anim}_{i}@4x.sprite", g)
    base = base_dir / f"{anim}_{i}.sprite" if base_dir else None
    write(pack / f"{anim}_{i}.sprite", rows(base) if base else read(g))
toml = (pack / "pack.toml").read_text(encoding="utf-8")
RAMPS_NEEDED = {"ţ": ("t", -2), "Ţ": ("t", 2), "ƭ": ("x", -2), "Ƭ": ("x", 2), "ȼ": ("o", -2), "Ȼ": ("o", 2), "ǩ": ("A", -2), "Ǩ": ("A", 2)}
for k, (of, lv) in RAMPS_NEEDED.items():
    if f'"{k}" = {{ of' not in toml:
        toml = toml.replace("[ramps]\n", f'[ramps]\n"{k}" = {{ of = "{of}", level = {lv} }}\n', 1)
if f'[animations."{anim}@4x"]' not in toml:
    toml = toml.replace(f"[animations.{anim}]", f'[animations."{anim}@4x"]\nframes   = []\nframe_ms = {ms}\n\n[animations.{anim}]', 1)
for key in (anim, f'"{anim}@4x"'):
    suffix = "@4x" if key.startswith('"') else ""
    names = ", ".join(f'"{anim}_{i}{suffix}.sprite"' for i in range(len(frames)))
    toml, n = re.subn(
        rf"(\[animations\.{re.escape(key)}\]\nframes\s*=\s*)\[[^\]]*\](\nframe_ms\s*=\s*)\d+",
        rf"\g<1>[{names}]\g<2>{ms}",
        toml,
    )
    assert n == 1, key
(pack / "pack.toml").write_text(toml, encoding="utf-8")
print("installed", len(frames))
