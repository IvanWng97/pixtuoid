#!/usr/bin/env python3
"""Guide citations whose symbol no longer exists in code.

The guides name code constantly — `SHARP-EDGES.md` alone cites ~390 symbols —
and nothing checks that the code still has them. `gen-guides-check` verifies the
generated index blocks match their siblings, which is a different claim: both
sides can agree while both describe a function that was deleted.

REPORTS, never gates. Some entries name a symbol ON PURPOSE because it must NOT
exist ("there is no `occludes_behind` field any more", "don't hoist a
`paint_glass_strip` helper"), and no rule separates those from rot — so the
sentence is printed with the name and a reader decides in one pass. What this
CANNOT see is the worse class: a symbol that still exists, described with
behaviour it lost.

Usage: guide-drift.py [--selftest]
"""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys
import tempfile

GUIDES = ("SHARP-EDGES.md", "WHERE-TO-LOOK.md", "LAYOUT.md", "CLAUDE.md")
CODE_DIRS = ("crates", "scripts", "site/src", "integrations")
# 5+ chars: `home`, `open` and friends are prose as often as they are symbols.
CITATION = re.compile(r"`([a-z_][a-z0-9_]{4,})(?:\(\))?`")


def cited(root: pathlib.Path) -> dict[str, list[tuple[str, str]]]:
    """Symbol → [(guide path, the sentence citing it)]."""
    out: dict[str, list[tuple[str, str]]] = {}
    for path in sorted(root.rglob("*.md")):
        if path.name not in GUIDES or "node_modules" in str(path):
            continue
        rel = str(path.relative_to(root))
        for line in path.read_text(errors="ignore").splitlines():
            for m in CITATION.finditer(line):
                out.setdefault(m.group(1), []).append((rel, line.strip()))
    return out


def missing_from_code(name: str, root: pathlib.Path) -> bool:
    # `-g !*.md` is the whole check: the guides live UNDER `crates/`, so a search
    # that includes them lets every citation satisfy itself.
    r = subprocess.run(
        ["rg", "-l", "--no-messages", "-F", "-g", "!*.md", name, *CODE_DIRS],
        cwd=root,
        capture_output=True,
        text=True,
    )
    return not r.stdout.strip()


def report(root: pathlib.Path) -> int:
    all_cited = cited(root)
    dead = {n: w for n, w in sorted(all_cited.items()) if missing_from_code(n, root)}
    print(f"guide-drift: {len(all_cited)} symbols cited, {len(dead)} not found in code")
    for name, where in dead.items():
        guide, sentence = where[0]
        print(f"\n  {name}  —  {guide}")
        print(f"    {sentence[:160]}")
    if dead:
        print("\n  A name can be cited BECAUSE it must not exist — read the sentence.")
    return 0


def selftest() -> int:
    fails: list[str] = []
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        (root / "crates" / "x" / "src").mkdir(parents=True)
        (root / "scripts").mkdir()
        (root / "crates" / "x" / "src" / "lib.rs").write_text("fn live_symbol() {}\n")
        guide = root / "crates" / "x" / "SHARP-EDGES.md"
        guide.write_text(
            "- **Entry.** `live_symbol` is real and `ghost_symbol` is not.\n"
            "  Prose mentioning `open` must stay below the length floor.\n"
        )
        names = cited(root)
        for want in ("live_symbol", "ghost_symbol"):
            if want not in names:
                fails.append(f"{want} must be picked up as a citation")
        if "open" in names:
            fails.append("`open` is 4 chars — the length floor must skip it")
        if missing_from_code("live_symbol", root):
            fails.append("a symbol present in code must NOT be reported")
        if not missing_from_code("ghost_symbol", root):
            fails.append("a symbol absent from code MUST be reported")
        # The bug this tool was born with: the guide satisfying its own citation.
        guide.write_text(guide.read_text() + "\nghost_symbol\n")
        if not missing_from_code("ghost_symbol", root):
            fails.append("a citation must not be satisfied by the guide that makes it")
    if fails:
        print("guide-drift selftest FAILED:")
        for f in fails:
            print(f"  - {f}")
        return 1
    print("guide-drift selftest: all checks passed")
    return 0


def main() -> int:
    if "--selftest" in sys.argv[1:]:
        return selftest()
    return report(pathlib.Path(__file__).resolve().parent.parent)


if __name__ == "__main__":
    sys.exit(main())
