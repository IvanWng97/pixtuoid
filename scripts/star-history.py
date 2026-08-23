#!/usr/bin/env python3
"""Render the README's star-history chart from GitHub's own stargazers data.

GitHub announced on 2026-06-30 that `stargazers` (the who-starred-when list)
would be limited to a repository's admins and collaborators —
https://github.blog/changelog/2026-06-30-upcoming-access-restrictions-to-public-api-endpoints-and-ui-views/
— and the cut-over blanked every hosted chart service (star-history.com
included) for every repo it does not own. The repo's own Actions token is
assumed to be a collaborator — the changelog names neither GraphQL nor Actions
tokens, so the first live run is the proof — and the chart is drawn here and
published to the `star-history` branch by `.github/workflows/star-history.yml`;
nothing leaves the repo.

Usage: `star-history.py OWNER/REPO OUT_DIR` with the token in `GH_TOKEN`
(`GITHUB_TOKEN` accepted). Writes `star-history-{light,dark}.svg`.
`--selftest` exercises the pure halves (bucketing, axes, rendering, paging)
with no network; exit 0 = pass.
"""

from __future__ import annotations

import datetime as dt
import json
import os
import pathlib
import re
import sys
import traceback
import xml.etree.ElementTree as ET
from collections.abc import Callable, Iterable
from typing import NamedTuple
from urllib import request

GRAPHQL_URL = "https://api.github.com/graphql"
# The API's hard page ceiling for `stargazers(first:)`.
PAGE_SIZE = 100
STARGAZERS_QUERY = """
query($owner: String!, $name: String!, $first: Int!, $after: String) {
  repository(owner: $owner, name: $name) {
    stargazerCount
    stargazers(first: $first, after: $after, orderBy: {field: STARRED_AT, direction: ASC}) {
      edges { starredAt }
      pageInfo { hasNextPage endCursor }
    }
  }
}
"""

Point = tuple[dt.date, int]


class Palette(NamedTuple):
    """The office roles the chart paints with; the README's `<picture>` picks light or dark."""

    wall: str
    trim: str
    carpet_light: str
    carpet_dark: str
    title: str
    text: str
    star: str


# Copied from the themes, not derived — the renderer is Python. The copy's guard
# is `crates/pixtuoid-scene/tests/readme_chart_palette.rs`, which pins every
# value to its theme by struct access, so a theme edit fails `just test`.
PALETTE_PATH = pathlib.Path(__file__).with_name("star-history-palette.json")
THEMES: dict[str, Palette] = {
    variant: Palette(**{f: row[f] for f in Palette._fields})
    for variant, row in json.loads(PALETTE_PATH.read_text(encoding="utf-8")).items()
}

WIDTH, HEIGHT = 800, 400
# One chart pixel — coarser than a label glyph needs, finer than the README banner's sprites.
PX = 4
TITLE_PX, LABEL_PX = 3, 2
MARGIN_LEFT, MARGIN_RIGHT, MARGIN_TOP, MARGIN_BOTTOM = 64, 32, 56, 44
PLOT_COLS = (WIDTH - MARGIN_LEFT - MARGIN_RIGHT) // PX
PLOT_ROWS = (HEIGHT - MARGIN_TOP - MARGIN_BOTTOM) // PX
Y_TICKS = 5
X_TICKS = 5
LABEL_GAP = 3 * PX
TITLE_Y = MARGIN_TOP - 10 * PX
STAMP_Y = MARGIN_TOP - 8 * PX
# The office carpet's two-tone checkerboard, tiled under the curve.
TILE = 2 * PX
# Forms are BASELINE rows tall on a GLYPH_H-row cell; the spare rows hold g j p q y's descenders.
GLYPH_W, GLYPH_H, BASELINE = 5, 9, 7
GLYPH_ADVANCE = GLYPH_W + 1

# Classic dot-matrix forms; `*` is the star. Drawn as rectangles so the chart
# needs no font: an SVG inside a GitHub `<img>` can't fetch a webfont, and a
# system font wouldn't be pixel art.
_FORMS: dict[str, tuple[str, ...]] = {
    "0": (".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###."),
    "1": ("..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###."),
    "2": (".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####"),
    "3": ("#####", "...#.", "..#..", "...#.", "....#", "#...#", ".###."),
    "4": ("...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#."),
    "5": ("#####", "#....", "####.", "....#", "....#", "#...#", ".###."),
    "6": ("..##.", ".#...", "#....", "####.", "#...#", "#...#", ".###."),
    "7": ("#####", "....#", "...#.", "..#..", ".#...", ".#...", ".#..."),
    "8": (".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###."),
    "9": (".###.", "#...#", "#...#", ".####", "....#", "...#.", ".##.."),
    "A": (".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"),
    "B": ("####.", "#...#", "#...#", "####.", "#...#", "#...#", "####."),
    "C": (".###.", "#...#", "#....", "#....", "#....", "#...#", ".###."),
    "D": ("###..", "#..#.", "#...#", "#...#", "#...#", "#..#.", "###.."),
    "E": ("#####", "#....", "#....", "####.", "#....", "#....", "#####"),
    "F": ("#####", "#....", "#....", "####.", "#....", "#....", "#...."),
    "G": (".###.", "#...#", "#....", "#.###", "#...#", "#...#", ".####"),
    "H": ("#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"),
    "I": (".###.", "..#..", "..#..", "..#..", "..#..", "..#..", ".###."),
    "J": ("..###", "...#.", "...#.", "...#.", "...#.", "#..#.", ".##.."),
    "K": ("#...#", "#..#.", "#.#..", "##...", "#.#..", "#..#.", "#...#"),
    "L": ("#....", "#....", "#....", "#....", "#....", "#....", "#####"),
    "M": ("#...#", "##.##", "#.#.#", "#.#.#", "#...#", "#...#", "#...#"),
    "N": ("#...#", "#...#", "##..#", "#.#.#", "#..##", "#...#", "#...#"),
    "O": (".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."),
    "P": ("####.", "#...#", "#...#", "####.", "#....", "#....", "#...."),
    "Q": (".###.", "#...#", "#...#", "#...#", "#.#.#", "#..#.", ".##.#"),
    "R": ("####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#"),
    "S": (".####", "#....", "#....", ".###.", "....#", "....#", "####."),
    "T": ("#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#.."),
    "U": ("#...#", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."),
    "V": ("#...#", "#...#", "#...#", "#...#", "#...#", ".#.#.", "..#.."),
    "W": ("#...#", "#...#", "#...#", "#.#.#", "#.#.#", "#.#.#", ".#.#."),
    "X": ("#...#", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "#...#"),
    "Y": ("#...#", "#...#", "#...#", ".#.#.", "..#..", "..#..", "..#.."),
    "Z": ("#####", "....#", "...#.", "..#..", ".#...", "#....", "#####"),
    "a": (".....", ".....", ".###.", "....#", ".####", "#...#", ".####"),
    "b": ("#....", "#....", "#.##.", "##..#", "#...#", "#...#", "####."),
    "c": (".....", ".....", ".###.", "#....", "#....", "#...#", ".###."),
    "d": ("....#", "....#", ".##.#", "#..##", "#...#", "#...#", ".####"),
    "e": (".....", ".....", ".###.", "#...#", "#####", "#....", ".###."),
    "f": ("..##.", ".#..#", ".#...", "###..", ".#...", ".#...", ".#..."),
    "g": (".....", ".....", ".####", "#...#", "#...#", "#...#", ".####", "....#", ".###."),
    "h": ("#....", "#....", "#.##.", "##..#", "#...#", "#...#", "#...#"),
    "i": ("..#..", ".....", ".##..", "..#..", "..#..", "..#..", ".###."),
    "j": ("...#.", ".....", "..##.", "...#.", "...#.", "...#.", "...#.", "#..#.", ".##.."),
    "k": ("#....", "#....", "#..#.", "#.#..", "##...", "#.#..", "#..#."),
    "l": (".##..", "..#..", "..#..", "..#..", "..#..", "..#..", ".###."),
    "m": (".....", ".....", "##.#.", "#.#.#", "#.#.#", "#...#", "#...#"),
    "n": (".....", ".....", "#.##.", "##..#", "#...#", "#...#", "#...#"),
    "o": (".....", ".....", ".###.", "#...#", "#...#", "#...#", ".###."),
    "p": (".....", ".....", "####.", "#...#", "#...#", "#...#", "####.", "#....", "#...."),
    "q": (".....", ".....", ".####", "#...#", "#...#", "#...#", ".####", "....#", "....#"),
    "r": (".....", ".....", "#.##.", "##..#", "#....", "#....", "#...."),
    "s": (".....", ".....", ".####", "#....", ".###.", "....#", "####."),
    "t": (".#...", ".#...", "###..", ".#...", ".#...", ".#..#", "..##."),
    "u": (".....", ".....", "#...#", "#...#", "#...#", "#..##", ".##.#"),
    "v": (".....", ".....", "#...#", "#...#", "#...#", ".#.#.", "..#.."),
    "w": (".....", ".....", "#...#", "#...#", "#.#.#", "#.#.#", ".#.#."),
    "x": (".....", ".....", "#...#", ".#.#.", "..#..", ".#.#.", "#...#"),
    "y": (".....", ".....", "#...#", "#...#", "#...#", "#...#", ".####", "....#", ".###."),
    "z": (".....", ".....", "#####", "...#.", "..#..", ".#...", "#####"),
    " ": (".....", ".....", ".....", ".....", ".....", ".....", "....."),
    "/": ("....#", "....#", "...#.", "..#..", ".#...", "#....", "#...."),
    "-": (".....", ".....", ".....", "#####", ".....", ".....", "....."),
    "_": (".....", ".....", ".....", ".....", ".....", ".....", "#####"),
    ".": (".....", ".....", ".....", ".....", ".....", ".##..", ".##.."),
    "*": ("..#..", "#.#.#", ".###.", "#####", ".###.", "#.#.#", "..#.."),
}
GLYPHS: dict[str, tuple[str, ...]] = {ch: rows + ("." * GLYPH_W,) * (GLYPH_H - len(rows)) for ch, rows in _FORMS.items()}


def cumulative_by_day(dates: Iterable[dt.date]) -> list[Point]:
    """Running star total, one point per distinct day, ascending."""
    series: list[Point] = []
    total = 0
    for day in sorted(dates):
        total += 1
        if series and series[-1][0] == day:
            series[-1] = (day, total)
        else:
            series.append((day, total))
    return series


def extend_to(series: list[Point], today: dt.date) -> list[Point]:
    """Carry the last total to `today` so the right edge reads as-of-render, not as-of-last-star."""
    if series and series[-1][0] < today:
        return [*series, (today, series[-1][1])]
    return series


def nice_step(top: int) -> int:
    """Smallest 1/2/5×10ⁿ step that fits `top` into the Y tick budget."""
    step = 1
    while top // step + 1 > Y_TICKS + 1:
        mant = step / 10 ** (len(str(step)) - 1)
        step = step * 2 if mant == 1 else int(step * 2.5) if mant == 2 else step * 2
    return step


def compact(n: int) -> str:
    """`1.5k`/`20k`/`1M` — the left margin fits four label glyphs, not a raw 5-digit count."""
    for unit, size in (("M", 1_000_000), ("k", 1_000)):
        if n >= size:
            return f"{n / size:.1f}".removesuffix(".0") + unit
    return str(n)


def _run(x: int, y: int, w: int, h: int) -> str:
    return f"M{x} {y}h{w}v{h}h-{w}z"


def text_width(s: str, px: int) -> int:
    return (len(s) * GLYPH_ADVANCE - 1) * px


def text_path(s: str, x: int, y: int, px: int) -> str:
    """Path data lighting `s` top-left at (x, y), `px` real pixels per glyph pixel; one run per horizontal stroke."""
    runs = []
    for i, ch in enumerate(s):
        rows = GLYPHS.get(ch)
        if rows is None:
            raise ValueError(f"no glyph for {ch!r} in {s!r}")
        gx = x + i * GLYPH_ADVANCE * px
        for r, row in enumerate(rows):
            for m in re.finditer(r"#+", row):
                runs.append(_run(gx + m.start() * px, y + r * px, (m.end() - m.start()) * px, px))
    return "".join(runs)


def x_labels(first: dt.date, last: dt.date) -> list[tuple[int, str]]:
    """Date labels as (x, text): as many as fit without touching, edge ones flush with the plot, one per day."""
    span_days = max((last - first).days, 1)
    fmt = "%b %d" if span_days < 365 else "%b %Y"
    w = text_width(last.strftime(fmt), LABEL_PX)
    plot_w = PLOT_COLS * PX
    # Edge labels are flush, interior ones centred: the first gap is pitch − 1.5·w.
    ticks = min(X_TICKS, plot_w // (w + w // 2 + LABEL_GAP))
    placed: list[tuple[int, str]] = []
    for i in range(ticks + 1):
        day = first + dt.timedelta(days=round(span_days * i / ticks))
        if day > last or (placed and placed[-1][1] == day.strftime(fmt)):
            continue
        col = MARGIN_LEFT + round((PLOT_COLS - 1) * (day - first).days / span_days) * PX
        x = col if i == 0 else col + PX - w if i == ticks else col + (PX - w) // 2
        placed.append((max(MARGIN_LEFT, min(x, MARGIN_LEFT + plot_w - w)), day.strftime(fmt)))
    return placed


def render_svg(repo: str, series: list[Point], theme: str, today: dt.date) -> str:
    pal = THEMES[theme]
    pts = extend_to(series, today) or [(today, 0)]
    first, last = pts[0][0], pts[-1][0]
    span_days = max((last - first).days, 1)
    top = pts[-1][1]
    step = nice_step(max(top, 1))
    y_max = max(step * ((top // step) + 1), step)
    plot_x, plot_y = MARGIN_LEFT, MARGIN_TOP
    plot_w, plot_h = PLOT_COLS * PX, PLOT_ROWS * PX
    base_y = plot_y + plot_h

    def col_x(day: dt.date) -> int:
        return plot_x + round((PLOT_COLS - 1) * (day - first).days / span_days) * PX

    def rows_of(count: int) -> int:
        return round(PLOT_ROWS * count / y_max)

    # Stepped, not interpolated: a column's height is a count that was true on its day.
    heights = []
    k = 0
    for c in range(PLOT_COLS):
        day = first + dt.timedelta(days=round(span_days * c / (PLOT_COLS - 1)))
        while k + 1 < len(pts) and pts[k + 1][0] <= day:
            k += 1
        heights.append(rows_of(pts[k][1]) if pts[k][0] <= day else 0)
    fill = "".join(_run(plot_x + c * PX, base_y - h * PX, PX, h * PX) for c, h in enumerate(heights) if h)
    cap = "".join(_run(plot_x + c * PX, base_y - h * PX, PX, PX) for c, h in enumerate(heights) if h)
    grid = "".join(
        _run(plot_x + c * PX, base_y - rows_of(count) * PX - PX, PX, PX)
        for count in range(step, y_max + 1, step)
        for c in range(0, PLOT_COLS, 2)
    )
    axis = [_run(plot_x - PX, plot_y, PX, plot_h), _run(plot_x - PX, base_y, plot_w + PX, PX)]
    for count in range(0, y_max + 1, step):
        label = compact(count)
        axis.append(text_path(label, plot_x - LABEL_GAP - text_width(label, LABEL_PX), base_y - rows_of(count) * PX - BASELINE * LABEL_PX // 2, LABEL_PX))
    for lx, label in x_labels(first, last):
        axis.append(text_path(label, lx, base_y + LABEL_GAP, LABEL_PX))
    stamp = f"updated {today.isoformat()}"
    axis.append(text_path(stamp, WIDTH - MARGIN_RIGHT - text_width(stamp, LABEL_PX), STAMP_Y, LABEL_PX))
    count_label = f"* {top}"
    cw = text_width(count_label, LABEL_PX)
    cx = max(col_x(last) + PX - cw, plot_x)
    cy = max(base_y - rows_of(top) * PX - (BASELINE + 2) * LABEL_PX, plot_y)
    frame = "".join(
        [_run(0, 0, WIDTH, PX), _run(0, HEIGHT - PX, WIDTH, PX), _run(0, 0, PX, HEIGHT), _run(WIDTH - PX, 0, PX, HEIGHT)]
    )
    return "\n".join(
        [
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {WIDTH} {HEIGHT}" width="{WIDTH}" height="{HEIGHT}" shape-rendering="crispEdges" role="img" aria-labelledby="t">',
            f'<title id="t">{repo} star history: {top} stars as of {today.isoformat()}</title>',
            f'<pattern id="carpet" width="{2 * TILE}" height="{2 * TILE}" patternUnits="userSpaceOnUse">'
            f'<rect width="{2 * TILE}" height="{2 * TILE}" fill="{pal.carpet_dark}"/>'
            f'<rect width="{TILE}" height="{TILE}" fill="{pal.carpet_light}"/>'
            f'<rect x="{TILE}" y="{TILE}" width="{TILE}" height="{TILE}" fill="{pal.carpet_light}"/></pattern>',
            f'<rect width="{WIDTH}" height="{HEIGHT}" fill="{pal.wall}"/>',
            f'<path fill="{pal.trim}" d="{frame}{grid}"/>',
            f'<path fill="url(#carpet)" d="{fill}"/>',
            f'<path fill="{pal.star}" d="{cap}"/>',
            f'<path fill="{pal.text}" d="{"".join(axis)}"/>',
            f'<path fill="{pal.title}" d="{text_path(repo, MARGIN_LEFT, TITLE_Y, TITLE_PX)}"/>',
            f'<path fill="{pal.star}" d="{text_path(count_label, cx, cy, LABEL_PX)}"/>',
            "</svg>",
        ]
    ) + "\n"


def write_charts(repo: str, series: list[Point], out_dir: pathlib.Path, today: dt.date) -> list[pathlib.Path]:
    out_dir.mkdir(parents=True, exist_ok=True)
    paths = []
    for theme in THEMES:
        p = out_dir / f"star-history-{theme}.svg"
        p.write_text(render_svg(repo, series, theme, today), encoding="utf-8")
        paths.append(p)
    return paths


def _graphql_post(query: str, variables: dict, token: str) -> dict:
    body = json.dumps({"query": query, "variables": variables}).encode()
    req = request.Request(
        GRAPHQL_URL,
        data=body,
        headers={"Authorization": f"bearer {token}", "Content-Type": "application/json", "User-Agent": "pixtuoid-star-history"},
    )
    with request.urlopen(req, timeout=30) as resp:
        return json.load(resp)


def fetch_star_dates(repo: str, token: str, post: Callable[[str, dict, str], dict] = _graphql_post) -> list[dt.date]:
    owner, name = repo.split("/", 1)
    dates: list[dt.date] = []
    after: str | None = None
    while True:
        reply = post(STARGAZERS_QUERY, {"owner": owner, "name": name, "first": PAGE_SIZE, "after": after}, token)
        if reply.get("errors"):
            raise RuntimeError(f"GraphQL errors for {repo}: {json.dumps(reply['errors'])}")
        repository = reply["data"]["repository"]
        page = repository["stargazers"]
        dates.extend(dt.datetime.fromisoformat(e["starredAt"].replace("Z", "+00:00")).date() for e in page["edges"])
        if page["pageInfo"]["hasNextPage"]:
            after = page["pageInfo"]["endCursor"]
            continue
        # The announced deprecation shape is an EMPTY list, not an error; a blank
        # chart would replace the README's, so fewer rows than the public count reds the run.
        if len(dates) < repository["stargazerCount"]:
            raise RuntimeError(f"{repo}: stargazers returned {len(dates)} rows for stargazerCount {repository['stargazerCount']}")
        return dates


FAILS: list[str] = []


def check(cond: bool, msg: str) -> None:
    if not cond:
        FAILS.append(msg)


def _d(s: str) -> dt.date:
    return dt.date.fromisoformat(s)


def test_cumulative_by_day_counts_one_point_per_day_in_order() -> None:
    dates = [_d("2026-05-24"), _d("2026-05-26"), _d("2026-05-24"), _d("2026-05-25")]
    check(
        cumulative_by_day(dates)
        == [(_d("2026-05-24"), 2), (_d("2026-05-25"), 3), (_d("2026-05-26"), 4)],
        "cumulative_by_day must sort, bucket per day, and accumulate",
    )
    check(cumulative_by_day([]) == [], "cumulative_by_day of nothing is nothing")


def test_extend_to_holds_the_last_count_through_today() -> None:
    series = [(_d("2026-05-24"), 2), (_d("2026-05-26"), 4)]
    got = extend_to(series, _d("2026-06-01"))
    check(got[-1] == (_d("2026-06-01"), 4), f"extend_to must end at today with the last count, got {got[-1]}")
    check(got[:-1] == series, "extend_to must not alter the recorded points")
    check(extend_to(series, _d("2026-05-26")) == series, "extend_to is a no-op when today is the last point")


def test_compact_keeps_axis_labels_to_four_glyphs() -> None:
    cases = {0: "0", 500: "500", 1000: "1k", 1500: "1.5k", 20000: "20k", 200000: "200k", 1000000: "1M", 1500000: "1.5M"}
    for n, want in cases.items():
        check(compact(n) == want, f"compact({n}) = {compact(n)!r}, want {want!r}")
        check(len(compact(n)) <= 4, f"compact({n}) must fit the left margin")


def test_nice_step_yields_at_most_the_tick_budget() -> None:
    for top in (1, 7, 452, 1000, 12345):
        step = nice_step(top)
        ticks = top // step + 1
        check(1 <= ticks <= Y_TICKS + 1, f"nice_step({top})={step} gives {ticks} ticks, budget {Y_TICKS}")
        mant = step / 10 ** (len(str(step)) - 1)
        check(mant in (1, 2, 5), f"nice_step({top})={step} is not a 1/2/5 step")


SVG_NS = "{http://www.w3.org/2000/svg}"
# One `_run` rectangle; every path here is built from them.
RUN_RE = re.compile(r"M(-?\d+) (-?\d+)h(\d+)v(\d+)h-\d+z")


def test_glyphs_fill_the_cell() -> None:
    for ch, rows in GLYPHS.items():
        check(len(rows) == GLYPH_H and all(len(r) == GLYPH_W for r in rows), f"glyph {ch!r} is not {GLYPH_W}x{GLYPH_H}")
        check(all(set(r) <= {"#", "."} for r in rows), f"glyph {ch!r} has a stray character")


def test_descenders_drop_below_the_shared_baseline() -> None:
    def lowest_lit(ch: str) -> int:
        return max(r for r, row in enumerate(GLYPHS[ch]) if "#" in row)

    base = lowest_lit("x")
    check(all(lowest_lit(ch) == base for ch in "aAx0"), "x-height letters, capitals and digits share one baseline")
    check(all(lowest_lit(ch) > base for ch in "gjpqy"), "g j p q y must descend below that baseline, not sit on it like ρ")


def test_text_path_paints_every_glyph_pixel_and_nothing_else() -> None:
    runs = RUN_RE.findall(text_path("1", 10, 20, 2))
    lit = {(rx + dx, ry + dy) for rx, ry, w, h in (map(int, r) for r in runs) for dx in range(0, w, 2) for dy in range(0, h, 2)}
    want = {(10 + c * 2, 20 + r * 2) for r, row in enumerate(GLYPHS["1"]) for c, ch in enumerate(row) if ch == "#"}
    check(lit == want, f"text_path('1') must light exactly the glyph's pixels at scale 2; diff {lit ^ want}")
    check(text_width("ab", 3) == (2 * GLYPH_ADVANCE - 1) * 3, "text_width is advance per glyph minus the trailing gap, times scale")


def test_text_path_rejects_a_character_without_a_glyph() -> None:
    try:
        text_path("é", 0, 0, 1)
    except ValueError as e:
        check("é" in str(e), f"the offending character must be named: {e}")
    else:
        FAILS.append("text_path must refuse a character it cannot draw")


def test_themes_cover_both_readme_variants() -> None:
    check(set(THEMES) == {"light", "dark"}, f"the README's <picture> needs exactly light and dark, got {sorted(THEMES)}")


def _render_ok(svg: str, label: str) -> ET.Element | None:
    try:
        return ET.fromstring(svg)
    except ET.ParseError as e:
        FAILS.append(f"{label}: render_svg is not well-formed XML: {e}")
        return None


def test_x_labels_never_touch_and_stay_on_the_canvas() -> None:
    today = _d("2026-08-23")
    for span in (0, 1, 2, 30, 364, 365, 2405):
        placed = x_labels(today - dt.timedelta(days=span), today)
        boxes = [(x, x + text_width(label, LABEL_PX)) for x, label in placed]
        check(len(placed) >= 2 or span == 0, f"span {span}: both ends need a date, got {placed}")
        check(all(0 <= a and b <= WIDTH for a, b in boxes), f"span {span}: a label leaves the canvas: {boxes}")
        check(all(boxes[i + 1][0] - boxes[i][1] >= LABEL_GAP for i in range(len(boxes) - 1)), f"span {span}: labels need LABEL_GAP of air: {placed}")
        check(len({label for _, label in placed}) == len(placed), f"span {span}: a date is labelled twice: {placed}")


def test_render_is_well_formed_and_carries_the_facts() -> None:
    series = [(_d("2026-05-24"), 2), (_d("2026-06-14"), 300), (_d("2026-08-02"), 452)]
    for theme in THEMES:
        svg = render_svg("IvanWng97/pixtuoid", series, theme, _d("2026-08-23"))
        root = _render_ok(svg, theme)
        if root is None:
            continue
        title = root.findtext(f"{SVG_NS}title") or ""
        check("IvanWng97/pixtuoid" in title and "452" in title and "2026-08-23" in title, f"{theme}: <title> must carry repo, count and date; got {title!r}")
        check(THEMES[theme].star in svg and THEMES[theme].wall in svg, f"{theme}: the office palette must be painted")
        check(root.get("shape-rendering") == "crispEdges", f"{theme}: pixels must not be anti-aliased")
    light = render_svg("IvanWng97/pixtuoid", series, "light", _d("2026-08-23"))
    check(light == render_svg("IvanWng97/pixtuoid", series, "light", _d("2026-08-23")), "render must be deterministic")


def test_render_paints_only_inside_the_canvas() -> None:
    cases = (
        [(_d("2026-05-24"), 1)],
        [(_d("2020-01-01"), 1), (_d("2026-08-02"), 12345)],
        [(_d("2026-08-23"), 1234)],
        [(_d("2026-08-22"), 1), (_d("2026-08-23"), 2)],
        [],
    )
    for series in cases:
        svg = render_svg("IvanWng97/pixtuoid_with-a.long-name", series, "light", _d("2026-08-23"))
        runs = [tuple(map(int, r)) for r in RUN_RE.findall(svg)]
        check(bool(runs), "the chart must be painted as rectangle runs")
        outside = [r for r in runs if r[0] < 0 or r[1] < 0 or r[0] + r[2] > WIDTH or r[1] + r[3] > HEIGHT]
        check(not outside, f"{series[:1]}…: runs leave the {WIDTH}x{HEIGHT} canvas: {outside[:3]}")


def test_render_handles_a_repo_with_no_stars() -> None:
    svg = render_svg("o/r", [], "light", _d("2026-08-23"))
    root = _render_ok(svg, "empty series")
    if root is not None:
        check("0 stars" in (root.findtext(f"{SVG_NS}title") or ""), "empty series must still state a zero count")


def test_fetch_follows_cursors_until_the_last_page() -> None:
    pages = [
        {"data": {"repository": {"stargazerCount": 3, "stargazers": {
            "edges": [{"starredAt": "2026-05-24T04:01:48Z"}, {"starredAt": "2026-05-24T07:07:52Z"}],
            "pageInfo": {"hasNextPage": True, "endCursor": "c1"},
        }}}},
        {"data": {"repository": {"stargazerCount": 3, "stargazers": {
            "edges": [{"starredAt": "2026-05-25T00:00:00Z"}],
            "pageInfo": {"hasNextPage": False, "endCursor": "c2"},
        }}}},
    ]
    seen: list[dict] = []

    def post(query: str, variables: dict, token: str) -> dict:
        seen.append(variables)
        return pages[len(seen) - 1]

    got = fetch_star_dates("IvanWng97/pixtuoid", "t", post=post)
    check(got == [_d("2026-05-24"), _d("2026-05-24"), _d("2026-05-25")], f"fetch must flatten pages to dates, got {got}")
    check([v.get("after") for v in seen] == [None, "c1"], f"fetch must thread the cursor, sent {seen}")
    check(seen[0]["owner"] == "IvanWng97" and seen[0]["name"] == "pixtuoid", "fetch must split owner/name")


def test_fetch_refuses_a_withheld_list() -> None:
    def post(query: str, variables: dict, token: str) -> dict:
        return {"data": {"repository": {"stargazerCount": 452, "stargazers": {"edges": [], "pageInfo": {"hasNextPage": False, "endCursor": None}}}}}

    try:
        fetch_star_dates("o/r", "t", post=post)
    except RuntimeError as e:
        check("452" in str(e) and "0" in str(e), f"the message must show count vs rows: {e}")
    else:
        FAILS.append("an empty list under a non-zero stargazerCount is the deprecation shape — it must raise, not publish a blank chart")


def test_fetch_raises_on_graphql_errors() -> None:
    def post(query: str, variables: dict, token: str) -> dict:
        return {"errors": [{"type": "NOT_FOUND", "message": "Could not resolve to a Repository"}]}

    try:
        fetch_star_dates("o/r", "t", post=post)
    except RuntimeError as e:
        check("NOT_FOUND" in str(e), f"the GraphQL error type must reach the message: {e}")
    else:
        FAILS.append("fetch must raise on a GraphQL errors array")


def test_write_charts_emits_one_file_per_theme() -> None:
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        out = pathlib.Path(tmp) / "nested"
        paths = write_charts("o/r", [(_d("2026-05-24"), 1)], out, _d("2026-08-23"))
        check(sorted(p.name for p in paths) == sorted(f"star-history-{t}.svg" for t in THEMES), f"got {paths}")
        check(all(p.exists() and p.stat().st_size > 0 for p in paths), "every chart must be written non-empty")


def selftest() -> int:
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
            except Exception:  # noqa: BLE001 — a crashing test is a failing test
                FAILS.append(f"{name} crashed:\n{traceback.format_exc()}")
    for f in FAILS:
        print(f"FAIL: {f}", file=sys.stderr)
    print(f"star-history selftest: {'FAIL' if FAILS else 'ok'}")
    return 1 if FAILS else 0


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return selftest()
    if len(argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    repo, out_dir = argv
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if not token:
        print("error: GH_TOKEN (or GITHUB_TOKEN) is unset", file=sys.stderr)
        return 2
    dates = fetch_star_dates(repo, token)
    series = cumulative_by_day(dates)
    today = dt.datetime.now(dt.timezone.utc).date()
    for p in write_charts(repo, series, pathlib.Path(out_dir), today):
        print(p)
    print(f"{repo}: {len(dates)} stars", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
