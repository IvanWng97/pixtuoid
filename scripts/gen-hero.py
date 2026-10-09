#!/usr/bin/env python3
"""Render the README's hero: the office's neon sign, with the cat asleep on it and a coworker holding a coffee.

The figures are the cutaway's own @4x art, dressed and outlined the way
`crates/pixtuoid-scene/src/character/hair.rs`'s `dress` does it. The colours
come from the theme and pack copies that `readme_chart_palette.rs` pins. The
image is static, so it is committed under docs/images, and `--check` holds it
to its inputs.

Usage: `gen-hero.py OUT_DIR` writes `hero-{light,dark}.svg`; `--check OUT_DIR`
fails when either differs from a fresh render; `--selftest` runs the tests.
"""

from __future__ import annotations

import pathlib
import sys
import tomllib
import traceback

from readme_pixels import BASELINE, PACK_DIR, THEMES, Frame, _run, animation, load_pack, sprite_path, sprite_size, text_path, text_width

Marks = dict[str, tuple[int, int]]

WORD, WORD_PX = "pixtuoid", 6
# One sign pixel: the sign's frame, its padding and the air round the picture.
PX = 4
# The @4x art's pixels, doubled so the figures stand level with the sign.
SPRITE_PX = 2
BODY, STYLE, VIEW = "holding_coffee@4x.sprite", "crop@4x", "front"
CAT = "cat_sleep@4x"
# Sign pixels of board round the word, across and down.
SIGN_PAD = (6, 4)
# Sign pixels between the sign and the coworker, and round the whole picture.
GAP, MARGIN = 6, 2
HALO_OPACITY = 0.22
N4 = ((1, 0), (-1, 0), (0, 1), (0, -1))
MANIFEST = tomllib.loads((PACK_DIR / "pack.toml").read_text(encoding="utf-8"))


def read_sprite(text: str) -> tuple[Frame, Marks]:
    rows, marks = [], {}
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("@mark"):
            _, name, x, y = line.split()
            marks[name] = (int(x), int(y))
        elif line and not line.startswith(("#", "@")):
            rows.append(line.split())
    return rows, marks


def _opaque_top(frame: Frame) -> int | None:
    return next((y for y, row in enumerate(frame) if any(k != "." for k in row)), None)


def dress(body: tuple[Frame, Marks], behind: tuple[Frame, Marks] | None, over: tuple[Frame, Marks] | None, view: str, line: str) -> Frame:
    """hair.rs's `dress`: the layers laid mark on mark, one `line` round the union, a gap it closes to one pixel filled."""
    frame, marks = body
    head = marks[f"head.{view}"]

    def laid(layer: tuple[Frame, Marks]) -> tuple[Frame, int, int]:
        lf, lm = layer
        mx, my = lm[f"head.{view}"]
        return lf, head[0] - mx, head[1] - my

    hair = [laid(layer) for layer in (behind, over) if layer]
    tops = [dy + top for lf, _, dy in hair if (top := _opaque_top(lf)) is not None]
    if (top := _opaque_top(frame)) is not None:
        tops.append(top)
    rise = max(-(min(tops) - 1), 0) if tops else 0
    w, h = len(frame[0]), len(frame) + rise
    px = [["."] * w for _ in range(h)]
    order = hair[:1] if behind else []
    for lf, dx, dy in [*order, (frame, 0, 0), *(hair[-1:] if over else [])]:
        for y, row in enumerate(lf):
            for x, key in enumerate(row):
                tx, ty = x + dx, y + dy + rise
                if key != "." and 0 <= tx < w and 0 <= ty < h:
                    px[ty][tx] = key

    def opaque(x: int, y: int) -> bool:
        return 0 <= x < w and 0 <= y < h and px[y][x] != "."

    for cond in (any, all):
        cells = [(x, y) for y in range(h) for x in range(w) if not opaque(x, y) and cond(opaque(x + dx, y + dy) for dx, dy in N4)]
        for x, y in cells:
            px[y][x] = line
    return px


def _figure() -> Frame:
    style = MANIFEST["hairstyles"][STYLE][VIEW]
    load = lambda name: read_sprite((PACK_DIR / name).read_text(encoding="utf-8"))  # noqa: E731
    return dress(load(BODY), load(style["behind"]) if "behind" in style else None, load(style["over"]) if "over" in style else None, VIEW, MANIFEST["characters"]["outline"])


def render_svg(theme: str) -> str:
    pal = THEMES[theme]
    pack = load_pack(PACK_DIR, (CAT,))
    guy = _figure()
    gw, gh = len(guy[0]) * SPRITE_PX, len(guy) * SPRITE_PX
    cw, ch = sprite_size(pack, CAT, SPRITE_PX)
    board_w = text_width(WORD, WORD_PX) + 2 * SIGN_PAD[0] * PX
    board_h = BASELINE * WORD_PX + 2 * SIGN_PAD[1] * PX
    # The cat sinks two of its own pixels into the sign's top frame.
    cat_above = ch - 2 * SPRITE_PX
    content_h = max(board_h + 2 * PX + cat_above, gh)
    width = 2 * MARGIN * PX + board_w + 2 * PX + GAP * PX + gw
    height = 2 * MARGIN * PX + content_h
    floor = height - MARGIN * PX
    bx, by = MARGIN * PX + PX, floor - PX - board_h
    tx, ty = bx + SIGN_PAD[0] * PX, by + SIGN_PAD[1] * PX
    halo = "".join(text_path(WORD, tx + dx * PX, ty + dy * PX, WORD_PX) for dx, dy in N4)
    cat, cat_css = animation(pack, CAT, bx + board_w - cw - 4 * PX, by - PX + 2 * SPRITE_PX, SPRITE_PX)
    css = "".join(
        [
            ".neon{animation:fl 6s steps(1,end) infinite}@keyframes fl{0%,89%,93%,100%{opacity:1}90%,92%{opacity:.35}}",
            "@media (prefers-reduced-motion:reduce){.neon{animation:none}}",
            *cat_css,
        ]
    )
    return "\n".join(
        [
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" width="{width}" height="{height}" shape-rendering="crispEdges" role="img" aria-labelledby="t">',
            f'<title id="t">{WORD}: a neon office sign, a cat asleep on it and a coworker holding a coffee</title>',
            f"<style>{css}</style>",
            f'<path fill="{pal.trim}" d="{_run(bx - PX, by - PX, board_w + 2 * PX, board_h + 2 * PX)}"/>',
            f'<path fill="{pal.neon_panel}" d="{_run(bx, by, board_w, board_h)}"/>',
            f'<g class="neon"><path fill="{pal.neon_brand}" fill-opacity="{HALO_OPACITY}" d="{halo}"/><path fill="{pal.neon_brand}" d="{text_path(WORD, tx, ty, WORD_PX)}"/></g>',
            cat,
            sprite_path(guy, bx + board_w + PX + GAP * PX, floor - gh, SPRITE_PX, pack.palette),
            "</svg>",
        ]
    ) + "\n"


def write_heroes(out_dir: pathlib.Path) -> list[pathlib.Path]:
    out_dir.mkdir(parents=True, exist_ok=True)
    paths = []
    for theme in THEMES:
        p = out_dir / f"hero-{theme}.svg"
        p.write_text(render_svg(theme), encoding="utf-8")
        paths.append(p)
    return paths


def stale(out_dir: pathlib.Path) -> list[str]:
    """The committed heroes that differ from a fresh render."""
    return [f"hero-{t}.svg" for t in THEMES if not (out_dir / f"hero-{t}.svg").is_file() or (out_dir / f"hero-{t}.svg").read_text(encoding="utf-8") != render_svg(t)]


FAILS: list[str] = []


def check(cond: bool, msg: str) -> None:
    if not cond:
        FAILS.append(msg)


def _grid(*rows: str) -> Frame:
    return [list(r) for r in rows]


def test_read_sprite_keeps_marks_and_rows() -> None:
    frame, marks = read_sprite("# c\n@frame 0\n@mark head.front 2 1\n. a .\nb . c\n")
    check(frame == [[".", "a", "."], ["b", ".", "c"]] and marks == {"head.front": (2, 1)}, f"got {frame}, {marks}")


def test_dress_lays_hair_mark_on_mark_and_rises_for_it() -> None:
    body = (_grid("....", ".SS.", ".SS."), {"head.front": (1, 1)})
    over = (_grid("HH", ".."), {"head.front": (0, 1)})
    got = dress(body, None, over, "front", "k")
    # The hair's mark (0, 1) lands on the body's (1, 1): its pixels sit on body row 0,
    # whose top opaque row is now 0, so the line needs one row of rise above it.
    check(len(got) == 4 and len(got[0]) == 4, f"the frame keeps the body's width and grows one row: {got}")
    check(got[1][1:3] == ["H", "H"], f"hair lands on the head mark: {got}")
    check(got[0][1:3] == ["k", "k"], f"the line runs over the hair: {got}")


def test_dress_cuts_hair_past_the_sides() -> None:
    body = (_grid("..", "SS"), {"head.front": (0, 1)})
    over = (_grid("HHHH"), {"head.front": (1, 0)})
    got = dress(body, None, over, "front", "k")
    check(all(len(row) == 2 for row in got), f"a layer past the body's sides is cut off: {got}")


def test_dress_rings_the_union_and_fills_one_pixel_holes() -> None:
    body = (_grid(".....", ".S.S.", ".SSS.", "....."), {"head.front": (2, 1)})
    got = dress(body, None, None, "front", "k")
    check(got[1][2] == "k", f"a gap closed on four sides takes the line: {got}")
    check(got[0][1] == "k" and got[0][0] == ".", f"the ring is four-neighbour, not diagonal: {got}")
    check(got[2][2] == "S", f"the body itself is untouched: {got}")
    corners = dress((_grid("S.S", "...", "S.S"), {"head.front": (0, 0)}), None, None, "front", "k")
    check(corners[2][1] == "k", f"the cell the ring encloses on four sides takes the line: {corners}")


def test_every_hero_pixel_has_a_colour() -> None:
    for theme in THEMES:
        svg = render_svg(theme)
        check('fill="None"' not in svg and "fill=\"\"" not in svg, f"{theme}: a pixel without a colour")


def test_render_is_well_formed_animated_and_deterministic() -> None:
    import xml.etree.ElementTree as ET

    for theme in THEMES:
        svg = render_svg(theme)
        root = ET.fromstring(svg)
        title = root.find("{http://www.w3.org/2000/svg}title")
        check(title is not None and "pixtuoid" in (title.text or ""), f"{theme}: the title must name pixtuoid")
        check('class="neon"' in svg and 'class="cat_sleep@4x"' not in svg and "prefers-reduced-motion" in svg, f"{theme}: the sign flickers and motion can be reduced")
        check(svg == render_svg(theme), f"{theme}: render must be deterministic")


def test_render_paints_inside_the_canvas() -> None:
    import re

    for theme in THEMES:
        svg = render_svg(theme)
        w, h = (int(v) for v in re.search(r'viewBox="0 0 (\d+) (\d+)"', svg).groups())
        runs = [tuple(map(int, r)) for r in re.findall(r"M(-?\d+) (-?\d+)h(\d+)v(\d+)h-\d+z", svg)]
        outside = [r for r in runs if r[0] < 0 or r[1] < 0 or r[0] + r[2] > w or r[1] + r[3] > h]
        check(bool(runs) and not outside, f"{theme}: runs leave the {w}x{h} canvas: {outside[:3]}")


def selftest() -> int:
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
            except Exception:  # noqa: BLE001 — a crashing test is a failing test
                FAILS.append(f"{name} crashed:\n{traceback.format_exc()}")
    for f in FAILS:
        print(f"FAIL: {f}", file=sys.stderr)
    print(f"gen-hero selftest: {'FAIL' if FAILS else 'ok'}")
    return 1 if FAILS else 0


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return selftest()
    check_mode = "--check" in argv
    args = [a for a in argv if a != "--check"]
    if len(args) != 1:
        print(__doc__, file=sys.stderr)
        return 2
    out_dir = pathlib.Path(args[0])
    if check_mode:
        bad = stale(out_dir)
        for name in bad:
            print(f"error: {out_dir / name} is stale; run `just gen-hero`", file=sys.stderr)
        return 1 if bad else 0
    for p in write_heroes(out_dir):
        print(p)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
