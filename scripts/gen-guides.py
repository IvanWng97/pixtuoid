#!/usr/bin/env python3
"""Regenerate the nested guides' index blocks from their sibling files.

Each nested CLAUDE.md is an INDEX over sibling reference files (SHARP-EDGES.md,
LAYOUT.md, WHERE-TO-LOOK.md); every indexed line is a PROJECTION of the
sibling's own text — an entry's opening span plus a clipped gist, a tree
entry's clipped annotation, a question. A hand-maintained projection is two
copies of one fact (the latent-drift bug the magic-number rule names), so the
blocks are marked and regenerated instead — the gen-readme idiom:

    <!-- edges:start · generated from SHARP-EDGES.md by `just gen-guides` ... -->
    <!-- edges:end -->

Every emitted line is a verbatim prefix of its sibling entry — ASSERTED at
generation time (main()'s post-condition), not assumed — so grepping an index
line lands on the full entry. Prose that points at a sibling BY NAME is
findable with `grep -rl SHARP-EDGES.md`; prose that points at the GUIDE
instead ("see CLAUDE.md's sharp edges" — some Rust doc-comments still do)
resolves only via the index line, and nothing catches a new one.

Usage: gen-guides.py [--check]   (--check: write nothing, exit 1 on drift)
"""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

START = re.compile(r"^<!-- (edges|layout|lookup):start · generated from (\S+) ")
END = {kind: f"<!-- {kind}:end -->" for kind in ("edges", "layout", "lookup")}
TREE_ENTRY = re.compile(r"^[│ ]*[├└]── ")
# A long filename squeezes the annotation column to ONE space; keying the split on
# a 2+-space run shipped that row unclipped AND unmarked. Glyph rows split on 1+.
TREE_SPLIT = re.compile(r"^([│ ]*[├└]── \S+|\S[^ ]*) +(\S.*)$")
# Group 1 keeps the lead's own trailing separator: re-synthesizing a " " between
# lead and gist emitted `** ,` for a comma-followed lead — not verbatim, ungreppable.
BOLD_LEAD = re.compile(r"^(\*\*.+?\*\*\s*)(.*)$", re.S)
# The question, then optionally a parenthetical aside, then the → answer arm.
QUESTION = re.compile(r'^- "(.+?)"[^→]*→')

# Chars of sibling text an index line carries before deferring to the sibling —
# enough for the entry's claim, small enough that a couple dozen entries stay an index.
GIST_BUDGET = 96
TREE_BUDGET = 58  # annotation kept per skeleton entry (narrower: the KEY is the filename)


def clip(text: str, budget: int) -> str:
    """Word-boundary prefix of `text`, retreating past unbalanced markdown so the
    clip stays verbatim-greppable AND renders clean; ` …` marks the elision."""
    text = " ".join(text.split())
    if len(text) <= budget:
        return text
    cut = text.rfind(" ", 0, budget + 1)
    out = text[: cut if cut > 0 else budget]
    while out.count("`") % 2 or len(re.findall(r"\*\*", out)) % 2:
        out = out[: max(out.rfind("`"), out.rfind("**"))]
    return out.rstrip(" ,;:—-(") + " …"


def bullet_entries(md: str) -> list[str]:
    """Top-level `- ` bullets with their continuation lines joined."""
    out: list[str] = []
    cur: list[str] | None = None
    for ln in md.splitlines():
        if ln.startswith("- "):
            if cur:
                out.append(" ".join(cur))
            cur = [ln[2:].strip()]
        elif ln.startswith("#"):
            if cur:
                out.append(" ".join(cur))
            cur = None
        elif cur is not None and ln.strip():
            cur.append(ln.strip())
    if cur:
        out.append(" ".join(cur))
    return out


def edges_block(sib: pathlib.Path) -> list[str]:
    lines = []
    for e in bullet_entries(sib.read_text()):
        m = BOLD_LEAD.match(e)
        if m:
            lines.append(f"- {m.group(1)}{clip(m.group(2), GIST_BUDGET)}".rstrip())
        else:
            lines.append(f"- {clip(e, GIST_BUDGET)}")
    return lines


def layout_block(sib: pathlib.Path) -> list[str]:
    lines = sib.read_text().splitlines()
    fence = [i for i, ln in enumerate(lines) if ln.startswith("```")]
    skeleton = []
    for ln in lines[fence[0] + 1 : fence[1]]:
        if not (TREE_ENTRY.match(ln) or not ln.startswith(("│", " "))):
            continue
        m = TREE_SPLIT.match(ln)
        if m and len(m.group(2)) > TREE_BUDGET:
            pad = ln[len(m.group(1)) : len(ln) - len(m.group(2))]
            skeleton.append(f"{m.group(1)}{pad}{clip(m.group(2), TREE_BUDGET)}")
        else:
            skeleton.append(ln.rstrip())
    notes = []
    for para in re.split(r"\n\s*\n", "\n".join(lines[fence[1] + 1 :])):
        m = BOLD_LEAD.match(" ".join(x.strip() for x in para.strip().splitlines()))
        if m:
            notes.append(f"- {m.group(1)}{clip(m.group(2), GIST_BUDGET)}".rstrip())
    return ["```"] + skeleton + ["```"] + ([""] + notes if notes else [])


def lookup_block(sib: pathlib.Path) -> list[str]:
    return [f"- {m.group(1)}" for ln in sib.read_text().splitlines() if (m := QUESTION.match(ln))]


BUILDERS = {"edges": edges_block, "layout": layout_block, "lookup": lookup_block}


def regenerate(guide: pathlib.Path) -> str:
    src = guide.read_text().splitlines()
    out: list[str] = []
    i = 0
    while i < len(src):
        out.append(src[i])
        m = START.match(src[i])
        if m:
            kind, sib_name = m.group(1), m.group(2)
            sib = guide.parent / sib_name
            if not sib.exists():
                sys.exit(f"{guide}: marker names missing sibling {sib_name}")
            if END[kind] not in src[i:]:
                sys.exit(f"{guide}: `{END[kind]}` is missing — restore the pair; the block regenerates between it")
            close = src.index(END[kind], i)
            built = BUILDERS[kind](sib)
            assert_verbatim(guide, sib, built)
            out.extend(built)
            out.append(END[kind])
            i = close
        i += 1
    return "\n".join(out) + "\n"


def assert_verbatim(guide: pathlib.Path, sib: pathlib.Path, built: list[str]) -> None:
    """--check compares the block to the generator; it cannot see the property the
    index exists FOR. Assert it here, or a rewrapped sibling breaks grep silently."""
    raw = sib.read_text()
    for ln in built:
        if ln.startswith("- "):
            probe = ln[2:]
        elif TREE_ENTRY.match(ln) and (m := TREE_SPLIT.match(ln)):
            probe = m.group(2)
        else:
            continue
        probe = probe[:-2].rstrip() if probe.endswith(" …") else probe
        if probe and probe not in raw:
            sys.exit(
                f"{guide.name}: index line is not verbatim in {sib.name} — "
                f"unwrap the sibling entry to one line, or fix the builder: {probe[:60]}…"
            )


def main() -> int:
    check = "--check" in sys.argv[1:]
    tracked = subprocess.run(
        ["git", "-C", str(ROOT), "ls-files", "*CLAUDE.md"],
        capture_output=True, text=True, check=True,
    ).stdout.splitlines()
    drifted = []
    for rel in tracked:
        guide = ROOT / rel
        if not any(START.match(ln) for ln in guide.read_text().splitlines()):
            continue
        new = regenerate(guide)
        if new != guide.read_text():
            drifted.append(rel)
            if not check:
                guide.write_text(new)
    if check and drifted:
        for rel in drifted:
            print(f"{rel}: index block drifted — `just gen-guides` REBUILDS the block FROM the sibling,\n"
                  f"  discarding any hand-edit inside it; put the change in the SIBLING first")
        return 1
    verb = "would rewrite" if check else "rewrote"
    print(f"gen-guides: {verb} {len(drifted)} guide(s)" if drifted else "gen-guides: all index blocks current ✓")
    return 0


if __name__ == "__main__":
    sys.exit(main())
