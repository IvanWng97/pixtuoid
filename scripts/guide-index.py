#!/usr/bin/env python3
"""Gate the nested guides' index↔sibling contract.

A crate guide's "Known sharp edges" section is an INDEX: one line per entry,
each line the entry's own opening bold span VERBATIM, so `grep` on it lands in
the sibling `SHARP-EDGES.md`. Nothing else enforces that, and both halves rot
silently — a new entry with no index line is invisible to every agent that
reads only the guide, and a reworded index line still READS fine while no
longer grepping to anything.

Third arm: no OTHER doc may tell an agent the entries live in a `CLAUDE.md`.
That is the regression this file exists for — splitting the bulk out left 15
sites across 10 prompts/skills/rules pointing at the index as if it were the
text, which degrades every review that refutes a finding by citing a sharp edge.

Usage: guide-index.py [--selftest]
"""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

EDGES_HEADING = re.compile(r"^## .*[Ss]harp edges")
LEAD = re.compile(r"^- \*\*(.+?)\*\*")
PLAIN_LEAD = re.compile(r"^- (?!\*\*)(.+)$")

# Phrasings that assert the ENTRIES live in a CLAUDE.md. Deliberately narrow:
# "sharp edges are INDEXED in the nested CLAUDE.md" is correct and must not fire.
MISDIRECTION = [
    re.compile(r'"Known sharp edges" in\s+`?CLAUDE\.md', re.I),
    re.compile(r'`?CLAUDE\.md`?\s+"Known sharp edges"', re.I),
    re.compile(r"`?CLAUDE\.md`?\s+sharp edge", re.I),
    re.compile(r'sharp edges"?\s+live in the nested\s+`?CLAUDE\.md', re.I),
]
# The patterns above only catch a reference that NAMES CLAUDE.md next to the
# phrase. The costlier form is anaphoric — "read those for the known sharp
# edges", where "those" was CLAUDE.md two clauses back — which no regex reaches.
# So: any doc that sends a reader to the section by name owes the sibling's name
# too. Scoped to the exact section title, not the bare words "sharp edge".
POINTER_PHRASE = re.compile(r'"?known sharp edges"?', re.I)
SIBLING_NAME = "SHARP-EDGES.md"
# The guides themselves legitimately carry a `## Known sharp edges` index heading.
MISDIRECTION_SKIP = {"CLAUDE.md", "AGENTS.md", "SHARP-EDGES.md"}


def index_leads(guide: pathlib.Path) -> list[str]:
    """The guide's sharp-edge index lines, in order."""
    out, inside = [], False
    for ln in guide.read_text().splitlines():
        if ln.startswith("## "):
            inside = bool(EDGES_HEADING.match(ln))
            continue
        if not inside:
            continue
        if m := LEAD.match(ln):
            out.append(m.group(1).strip())
        elif m := PLAIN_LEAD.match(ln):
            out.append(m.group(1).strip())
    return out


def entry_leads(sib: pathlib.Path) -> list[str]:
    """The sibling's entry leads, in order."""
    return [m.group(1).strip() for ln in sib.read_text().splitlines() if (m := LEAD.match(ln))]


def tracked(root: pathlib.Path) -> list[pathlib.Path]:
    """Git-TRACKED files only — `.gitignore` is the authority on what is ours.
    A skip list would have to re-learn every local scratch dir (`.superpowers/`,
    vendored `node_modules/`) that git already knows to ignore."""
    out = subprocess.run(
        ["git", "-C", str(root), "ls-files"], capture_output=True, text=True, check=True
    ).stdout
    return [root / line for line in out.splitlines() if line]


def check_pairs(root: pathlib.Path) -> list[str]:
    fails = []
    for sib in sorted(p for p in tracked(root) if p.name == "SHARP-EDGES.md"):
        guide = sib.parent / "CLAUDE.md"
        rel = sib.relative_to(root)
        if not guide.exists():
            fails.append(f"{rel}: no sibling CLAUDE.md to index it")
            continue
        idx, ent = index_leads(guide), entry_leads(sib)
        body = sib.read_text()
        for missing in [e for e in ent if e not in idx]:
            fails.append(f"{rel}: entry «{missing[:60]}» has NO index line in {guide.name}")
        for stale in [i for i in idx if i.rstrip(":") not in body]:
            fails.append(f"{guide.relative_to(root)}: index «{stale[:60]}» greps to nothing in {sib.name}")
    return fails


def check_misdirection(root: pathlib.Path) -> list[str]:
    fails = []
    for p in sorted(tracked(root)):
        if p.suffix not in {".md", ".yml", ".yaml"} or p.name in MISDIRECTION_SKIP:
            continue
        text = p.read_text(errors="ignore")
        hit = next((m for rx in MISDIRECTION if (m := rx.search(text))), None)
        if hit:
            fails.append(
                f"{p.relative_to(root)}: «{hit.group(0)}» — the entries live in the crate's "
                "SHARP-EDGES.md; CLAUDE.md only indexes them"
            )
        elif (m := POINTER_PHRASE.search(text)) and SIBLING_NAME not in text:
            fails.append(
                f"{p.relative_to(root)}: sends the reader to «{m.group(0)}» but never names "
                f"{SIBLING_NAME}, where those entries actually live"
            )
    return fails


def selftest() -> int:
    """Negative control: prove each arm CAN fail, on a fixture built to break it."""
    import tempfile

    fails = []
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        (repo / "crate").mkdir(parents=True)
        (repo / "docs").mkdir()
        git = ["git", "-C", str(repo)]
        subprocess.run([*git, "init", "-q"], check=True)

        def stage(rel: str, body: str) -> None:
            """Write a fixture file and re-stage — the checks read `git ls-files`."""
            (repo / rel).write_text(body)
            subprocess.run([*git, "add", "-A"], check=True)

        GUIDE, SIB, DOC = "crate/CLAUDE.md", "crate/SHARP-EDGES.md", "docs/guide.md"
        good_guide = "## Known sharp edges\n\n- **Edge one.** ignored tail\n"
        good_sib = "# edges\n\n- **Edge one.** the full text\n"

        stage(GUIDE, good_guide)
        stage(SIB, good_sib)
        if check_pairs(repo):
            fails.append("a MATCHING index/sibling pair must pass")

        stage(SIB, good_sib + "- **Edge two.** unindexed\n")
        if not check_pairs(repo):
            fails.append("an entry with no index line must FAIL")

        stage(SIB, good_sib)
        stage(GUIDE, "## Known sharp edges\n\n- **Reworded lead.** x\n")
        if not check_pairs(repo):
            fails.append("an index line that greps to nothing must FAIL")

        stage(GUIDE, good_guide)  # restore, so only the misdirection arm speaks below

        stage(DOC, 'check "Known sharp edges" in CLAUDE.md before filing\n')
        if not check_misdirection(repo):
            fails.append("a doc pointing at CLAUDE.md for the entries must FAIL")

        stage(DOC, "sharp edges are indexed in the nested `CLAUDE.md`; see SHARP-EDGES.md\n")
        if check_misdirection(repo):
            fails.append("the CORRECT 'indexed in' phrasing must NOT fire")

        # The anaphoric arm: names the section, never names the sibling.
        stage(DOC, 'read those for the "known sharp edges"\n')
        if not check_misdirection(repo):
            fails.append("an anaphoric pointer that never names the sibling must FAIL")

        stage(DOC, 'the "known sharp edges" live in SHARP-EDGES.md\n')
        if check_misdirection(repo):
            fails.append("naming the sibling must clear the anaphoric arm")

    if fails:
        print("guide-index selftest FAILED:")
        for f in fails:
            print(f"  - {f}")
        return 1
    print("guide-index selftest: all 7 controls passed")
    return 0


def main() -> int:
    if "--selftest" in sys.argv[1:]:
        return selftest()
    fails = check_pairs(ROOT) + check_misdirection(ROOT)
    if fails:
        print(f"guide-index: {len(fails)} problem(s)")
        for f in fails:
            print(f"  {f}")
        return 1
    print("guide-index: every sharp-edge entry is indexed, every index line greps ✓")
    return 0


if __name__ == "__main__":
    sys.exit(main())
