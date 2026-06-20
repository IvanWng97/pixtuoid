#!/usr/bin/env python3
"""Self-test for sharp_edge_inventory.py — pins the three parsers so a regex
regression can't silently let the inventory drift from the CLAUDE.md edges (the
census would then mis-count citations). Pure, no I/O. Run:
`python3 scripts/sharp_edge_inventory_selftest.py` (exit 0 = pass)."""

import sys

import sharp_edge_inventory as m


def run() -> int:
    fails: list[str] = []

    def check(name: str, cond: bool) -> None:
        if not cond:
            fails.append(name)

    # count_sharp_edges: counts `- **` bullets ONLY inside the sharp-edges section.
    # A bold bullet before the section, a sub-bullet, a wrapped continuation line,
    # and a bullet in the NEXT section must all be excluded.
    md = (
        "# Guide\n\n"
        "- **not an edge** this bullet is above the section\n\n"
        "## Known sharp edges (don't be surprised by these)\n\n"
        "- **Edge one** the first edge\n"
        "  continuation line, still edge one\n"
        "  - **a sub-bullet** must not count\n"
        "- **Edge two** the second edge\n"
        "- **Edge three** the third\n\n"
        "## Where to look\n\n"
        "- **not an edge** this is in the next section\n"
    )
    check("counts exactly the 3 section bullets", m.count_sharp_edges(md) == 3)
    check("empty doc -> 0", m.count_sharp_edges("# x\n") == 0)
    # the alternate header spelling ("## Sharp edges") also matches.
    check(
        "alternate 'Sharp edges' header",
        m.count_sharp_edges("## Sharp edges\n- **a** x\n- **b** y\n") == 2,
    )

    # inventory_rows: data rows only (backtick slug + file key); header/separator skipped.
    inv = (
        "| slug | file | last cited | headline |\n"
        "|---|---|---|---|\n"
        "| `core-foo` | core | — | Foo |\n"
        "| `scene-bar` | scene | #2 | Bar |\n"
    )
    rows = m.inventory_rows(inv)
    check("parses 2 data rows", rows == [("core-foo", "core"), ("scene-bar", "scene")])
    check("skips header + separator", all(s not in ("slug", "---") for s, _ in rows))

    # ledger_citations: every [edge:<slug>] tag, nothing else.
    led = "row cites [edge:core-foo] and [edge:bin-max-desks-no-default]; not [issue:1] or `code`."
    cites = m.ledger_citations(led)
    check("finds both edge cites", cites == ["core-foo", "bin-max-desks-no-default"])
    check("no false cite", "issue" not in "".join(cites))

    if fails:
        print("sharp_edge_inventory selftest FAILED:")
        for f in fails:
            print(f"  ✗ {f}")
        return 1
    print("sharp_edge_inventory selftest: all assertions passed.")
    return 0


if __name__ == "__main__":
    sys.exit(run())
