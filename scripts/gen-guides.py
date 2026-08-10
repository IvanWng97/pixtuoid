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

import os
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
        joined = " ".join(x.strip() for x in para.strip().splitlines())
        if not joined:
            continue
        m = BOLD_LEAD.match(joined)
        if not m:
            # A silently-dropped note defeats the index (a `> `-quoted paragraph
            # shipped invisible this way); an unindexable one is the AUTHOR's bug.
            sys.exit(
                f"{sib}: post-fence paragraph has no bold lead, so it cannot be "
                f"indexed — bold its opening phrase: {joined[:60]}…"
            )
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


def run(root: pathlib.Path, check: bool, quiet: bool = False) -> int:
    tracked = subprocess.run(
        ["git", "-C", str(root), "ls-files", "*CLAUDE.md"],
        capture_output=True, text=True, check=True,
        # A hook exports GIT_DIR/GIT_INDEX_FILE and those OVERRIDE `-C`, so under
        # `pre-push` this listed the REAL repo's guides while `root` was the
        # selftest's throwaway tree, and the read below died on a path that does
        # not exist there.
        env={k: v for k, v in os.environ.items() if not k.startswith("GIT_")},
    ).stdout.splitlines()
    drifted = []
    for rel in tracked:
        guide = root / rel
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
    if not quiet:
        verb = "would rewrite" if check else "rewrote"
        print(f"gen-guides: {verb} {len(drifted)} guide(s)" if drifted else "gen-guides: all index blocks current ✓")
    return 0


def selftest() -> int:
    """Negative-control every failure arm on a throwaway repo — a generator whose
    own fires/does-not-fire contract broke rewrites blocks with garbage quietly."""
    import tempfile

    fails: list[str] = []
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        (repo / "crate").mkdir(parents=True)
        git = ["git", "-C", str(repo)]
        # A git hook exports GIT_DIR/GIT_INDEX_FILE, and those OVERRIDE `-C`, so
        # under `pre-push` these `add -A` calls staged the fixtures into the REAL
        # repo — after which `--check` read a tracked `crate/CLAUDE.md` with no
        # file behind it and died instead of reporting. Scrub the inherited git
        # env so the throwaway repo is the only one this can touch.
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        subprocess.run([*git, "init", "-q"], check=True, env=env)

        def stage(rel: str, body: str) -> None:
            (repo / rel).write_text(body)
            subprocess.run([*git, "add", "-A"], check=True, capture_output=True, env=env)

        def gen(check: bool) -> int:
            try:
                return run(repo, check, quiet=True)
            except SystemExit as e:  # sys.exit from a builder/assert arm
                return 1 if e.code else 0

        guide = (
            "# g\n\n## Layout\n\n"
            "<!-- layout:start · generated from LAYOUT.md by `just gen-guides` — x -->\n"
            "<!-- layout:end -->\n\n## Known sharp edges\n\n"
            "<!-- edges:start · generated from SHARP-EDGES.md by `just gen-guides` — x -->\n"
            "<!-- edges:end -->\n\n## Where to look\n\n"
            "<!-- lookup:start · generated from WHERE-TO-LOOK.md by `just gen-guides` — x -->\n"
            "<!-- lookup:end -->\n"
        )
        layout = "# l\n\n```\nsrc/\n├── a.rs   does the thing, at length beyond any budget anyone set, truly beyond it\n```\n\n**A note.** body\n"
        edges = "# e\n\n- **Edge one.** the full text\n"
        lookup = '# w\n\n- "How does it work?" → the answer\n'
        stage("crate/CLAUDE.md", guide)
        stage("crate/LAYOUT.md", layout)
        stage("crate/SHARP-EDGES.md", edges)
        stage("crate/WHERE-TO-LOOK.md", lookup)

        if gen(False) != 0:
            fails.append("a well-formed fixture must generate")
        first = (repo / "crate/CLAUDE.md").read_text()
        if gen(False) != 0 or (repo / "crate/CLAUDE.md").read_text() != first:
            fails.append("generation must be idempotent")
        if "- How does it work?" not in first or "- **Edge one.**" not in first or "├── a.rs" not in first:
            fails.append("all three block kinds must emit")
        if gen(True) != 0:
            fails.append("--check must pass right after generation")

        stage("crate/CLAUDE.md", first.replace("- **Edge one.** the full text", "- **Edge one.** EDITED"))
        if gen(True) != 1:
            fails.append("a hand-edited index line must red --check")
        gen(False)

        stage("crate/SHARP-EDGES.md", edges.replace("Edge one", "Edge ONE"))
        if gen(True) != 1:
            fails.append("a sibling edit must red --check")
        gen(False)

        stage("crate/WHERE-TO-LOOK.md", lookup.replace("How does it work?", "How does it REALLY work?"))
        if gen(True) != 1:
            fails.append("lookup drift must red --check (the lookup-only-skip regression)")
        gen(False)

        stage("crate/SHARP-EDGES.md", "# e\n\n- **Edge one.** the full\ntext wrapped hard\n")
        if gen(False) != 1:
            fails.append("a hard-wrapped sibling entry must FAIL generation (assert_verbatim)")
        stage("crate/SHARP-EDGES.md", edges)

        stage("crate/LAYOUT.md", layout + "\n> **Quoted note.** silently droppable?\n")
        if gen(False) != 1:
            fails.append("an unindexable post-fence paragraph must FAIL generation, not vanish")
        stage("crate/LAYOUT.md", layout)
        if gen(False) != 0 or gen(True) != 0:
            fails.append("fixture must return to green after the controls")

    if fails:
        print("gen-guides selftest FAILED:")
        for f in fails:
            print(f"  - {f}")
        return 1
    print("gen-guides selftest: all 10 controls passed")
    return 0


def main() -> int:
    if "--selftest" in sys.argv[1:]:
        return selftest()
    return run(ROOT, "--check" in sys.argv[1:])


if __name__ == "__main__":
    sys.exit(main())
