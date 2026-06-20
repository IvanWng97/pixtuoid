#!/usr/bin/env python3
"""Keep `docs/review-metrics/sharp-edge-inventory.md` in lockstep with the
CLAUDE.md sharp edges, and validate the ledger's `[edge:<slug>]` citations.

The review-history census's sharp-edge-citation leg counts ledger citations
against a canonical slug set. This makes that set a COMMITTED artifact (the
inventory) instead of an ad-hoc per-census harvest (#386), and guards it two ways
— the `supported_sources_manifest` bridge-test pattern:

  1. **Per-file count parity** — each nested CLAUDE.md's "Known sharp edges"
     bullet count must equal the inventory rows for that guide, so a sharp edge
     can't be added/removed from a CLAUDE.md without updating the inventory (the
     silent-drift the leg would otherwise mis-measure).
  2. **No orphan citations** — every `[edge:<slug>]` in REVIEW-LEDGER.md must
     resolve to an inventory slug (a typo / a removed edge is a dangling cite).

Pure parsers + a thin I/O `main`; offline. Run: `python3 scripts/sharp_edge_inventory.py`
(exit 0 = in sync) or `--selftest`.
"""

import argparse
import re
import sys
from collections import Counter
from pathlib import Path

# inventory `file` key → the guide it indexes.
FILE_MAP = {
    "core": "crates/pixtuoid-core/CLAUDE.md",
    "tests": "crates/pixtuoid-core/tests/CLAUDE.md",
    "scene": "crates/pixtuoid-scene/CLAUDE.md",
    "bin": "crates/pixtuoid/CLAUDE.md",
    "tui": "crates/pixtuoid/src/tui/CLAUDE.md",
}
# `qa`/`alias` rows may also point at the workspace root guide (no countable
# bullets there); only `edge` rows are count-parity'd against FILE_MAP.
QA_FILES = set(FILE_MAP) | {"root"}
INVENTORY = Path("docs/review-metrics/sharp-edge-inventory.md")
LEDGER = Path("docs/REVIEW-LEDGER.md")

_SECTION_HDR = re.compile(r"^## (Known sharp edges|Sharp edges)\b")
# | `<slug>` | <file> | <kind> | …
_ROW = re.compile(r"^\|\s*`([a-z0-9-]+)`\s*\|\s*(\w+)\s*\|\s*(\w+)\s*\|")
_CITE = re.compile(r"\[edge:([a-z0-9-]+)\]")


def count_sharp_edges(claude_md: str) -> int:
    """Formal sharp-edge bullets = the `- **…**` lines under the
    "Known sharp edges"/"Sharp edges" `##` header, up to the next `##`."""
    in_section = False
    n = 0
    for line in claude_md.splitlines():
        if _SECTION_HDR.match(line):
            in_section = True
            continue
        if in_section and line.startswith("## "):
            break
        if in_section and line.startswith("- **"):
            n += 1
    return n


def inventory_rows(inventory_md: str) -> "list[tuple[str, str, str]]":
    """The (slug, file-key, kind) triples from the inventory table's data rows."""
    return [
        (m.group(1), m.group(2), m.group(3))
        for line in inventory_md.splitlines()
        if (m := _ROW.match(line))
    ]


def ledger_citations(ledger_md: str) -> "list[str]":
    """Every `[edge:<slug>]` tag in the ledger."""
    return _CITE.findall(ledger_md)


def duplicate_slugs(rows: "list[tuple[str, str, str]]") -> "list[str]":
    """Slugs that appear on more than one inventory row — the slug is the citation
    key, so a duplicate is ambiguous (and would otherwise surface only as a
    misleading count DRIFT)."""
    counts = Counter(slug for slug, _, _ in rows)
    return sorted(slug for slug, n in counts.items() if n > 1)


def check() -> "list[str]":
    problems: list[str] = []
    rows = inventory_rows(INVENTORY.read_text())

    # A duplicate slug is named explicitly (else it only shows as a confusing
    # per-file count DRIFT) — the slug is the ledger's citation key, must be unique.
    for slug in duplicate_slugs(rows):
        problems.append(
            f"DUPLICATE SLUG: `{slug}` appears on multiple inventory rows — "
            "the slug is the citation key and must be unique"
        )

    # Count parity applies ONLY to `edge` rows (qa/alias have no `- **` bullet).
    edge_per_file = Counter(fk for _, fk, kind in rows if kind == "edge")
    for fk, path in FILE_MAP.items():
        actual = count_sharp_edges(Path(path).read_text())
        listed = edge_per_file.get(fk, 0)
        if actual != listed:
            problems.append(
                f"DRIFT: {path} has {actual} sharp-edge bullet(s) but the inventory "
                f"lists {listed} `edge` row(s) for `{fk}` — update {INVENTORY}"
            )

    for slug, fk, kind in rows:
        allowed = FILE_MAP if kind == "edge" else QA_FILES
        if fk not in allowed:
            problems.append(
                f"BAD FILE KEY: inventory `{kind}` slug `{slug}` names unknown guide `{fk}`"
            )

    # Orphan check resolves against EVERY slug (edge + qa + alias).
    slugs = {s for s, _, _ in rows}
    for cited in sorted(set(ledger_citations(LEDGER.read_text()))):
        if cited not in slugs:
            problems.append(
                f"ORPHAN CITATION: {LEDGER} cites [edge:{cited}] but no such slug is in "
                f"the inventory (typo, or a removed edge)"
            )
    return problems


def main() -> int:
    ap = argparse.ArgumentParser(description="Validate the sharp-edge inventory vs CLAUDE.md + the ledger.")
    ap.add_argument("--selftest", action="store_true", help="run pure-fn tests, no I/O")
    if ap.parse_args().selftest:
        import sharp_edge_inventory_selftest as st

        return st.run()
    problems = check()
    if problems:
        print("sharp-edge inventory: OUT OF SYNC")
        for p in problems:
            print(f"  ✗ {p}")
        return 1
    kinds = Counter(kind for _, _, kind in inventory_rows(INVENTORY.read_text()))
    print(
        f"sharp-edge inventory: in sync ({kinds['edge']} edges / {kinds['qa']} qa / "
        f"{kinds['alias']} alias across {len(FILE_MAP)} guides; ledger citations resolve)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
