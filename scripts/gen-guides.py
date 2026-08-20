#!/usr/bin/env python3
"""Regenerate the nested guides' index blocks from their sibling files.

Each nested CLAUDE.md is an INDEX over sibling reference files (SHARP-EDGES.md,
WHERE-TO-LOOK.md); every indexed line is a PROJECTION of the sibling's own
text — an entry's opening span plus a clipped gist, or a question. A
hand-maintained projection is two copies of one fact (the latent-drift bug the
magic-number rule names), so the blocks are marked and regenerated instead —
the gen-readme idiom:

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
import sys

import gitenv

ROOT = pathlib.Path(__file__).resolve().parent.parent

START = re.compile(r"^<!-- (edges|lookup):start · generated from (\S+) ")
END = {kind: f"<!-- {kind}:end -->" for kind in ("edges", "lookup")}
# Group 1 keeps the lead's own trailing separator: re-synthesizing a " " between
# lead and gist emitted `** ,` for a comma-followed lead — not verbatim, ungreppable.
BOLD_LEAD = re.compile(r"^(\*\*.+?\*\*\s*)(.*)$", re.S)
# The question, then optionally a parenthetical aside, then the → answer arm.
QUESTION = re.compile(r'^- "(.+?)"[^→]*→')

# Chars of sibling text an index line carries before deferring to the sibling —
# enough for the entry's claim, small enough that a couple dozen entries stay an index.
GIST_BUDGET = 96
# An entry is fact + WHY + authority pointer; past this it has become a per-subsystem
# changelog — split it into its component edges or move the argument to its issue.
MAX_ENTRY_CHARS = 2000

# Whole-file byte budgets for the always-injected / reviewer-loaded docs — the
# deletion-side forcing function (2026-08 context diet): growth past the cap fails
# `--check` AND generation, so a grown file must cut, or raise its budget
# consciously in the same PR (one-in-one-out).
BUDGETS: dict[str, int] = {
    "CLAUDE.md": 14_000,
    "docs/CONTRIBUTING.md": 19_000,
    "docs/ARCHITECTURE.md": 7_500,
    "docs/PARALLEL-DELIVERY.md": 4_000,
    ".github/prompts/pr-review.prompt.md": 11_000,
    ".github/prompts/impl-plan.prompt.md": 8_000,
    ".github/prompts/pr_review_rules.md": 8_000,
    ".claude/skills/two-lens-review/SKILL.md": 14_000,
    "crates/pixtuoid-core/CLAUDE.md": 10_500,
    "crates/pixtuoid-core/SHARP-EDGES.md": 16_000,
    "crates/pixtuoid-core/WHERE-TO-LOOK.md": 4_000,
    "crates/pixtuoid-core/tests/CLAUDE.md": 7_000,
    "crates/pixtuoid-scene/CLAUDE.md": 10_500,
    "crates/pixtuoid-scene/SHARP-EDGES.md": 13_000,
    "crates/pixtuoid-scene/WHERE-TO-LOOK.md": 5_300,
    "crates/pixtuoid/CLAUDE.md": 5_000,
    "crates/pixtuoid/SHARP-EDGES.md": 8_000,
    "crates/pixtuoid/WHERE-TO-LOOK.md": 3_000,
    "crates/pixtuoid/src/tui/CLAUDE.md": 1_600,
    "crates/pixtuoid/src/tui/WHERE-TO-LOOK.md": 3_500,
    "site/CLAUDE.md": 7_500,
    "site/SINGLE-SOURCED.md": 8_500,
    "integrations/raycast/CLAUDE.md": 8_500,
}


def guard_file_budgets(root: pathlib.Path, budgets: dict[str, int]) -> list[str]:
    out = []
    for rel, cap in sorted(budgets.items()):
        p = root / rel
        if not p.exists():
            out.append(f"{rel}: budgeted file is MISSING — a renamed/deleted file "
                       f"silently loses its cap; fix or remove the BUDGETS row")
        elif (size := p.stat().st_size) > cap:
            hint = (" (its index block is GENERATED: cut a SHARP-EDGES/WHERE-TO-LOOK "
                    "entry or the hand-written prose, never the block)"
                    if rel.endswith("CLAUDE.md") else "")
            out.append(f"{rel}: {size} bytes > budget {cap} — cut it{hint}, or raise "
                       f"its budget consciously in the same PR (one-in-one-out)")
    return out


def guard_budget_coverage(tracked: list[str], budgets: dict[str, int]) -> list[str]:
    """A new tracked guide must arrive with a cap — an allowlist the author must
    remember is the fail-open half of a byte budget."""
    return [f"{rel}: tracked guide has no BUDGETS entry — add one"
            for rel in tracked if rel not in budgets]


RS_COMMENT_WRAP = re.compile(r"\n\s*//[/!]?")
PINNED_BY = re.compile(r"[Pp]inned by\s+\[?`([a-z0-9_]{4,})`")
DECLARED_FN = re.compile(r"\bfn\s+([a-z0-9_]+)")


def guard_pinned_by_claims(root: pathlib.Path) -> list[str]:
    """A ``Pinned by `x``` comment must name a function that exists — prose outlives
    the code it describes; a named mechanism cannot. Generalizes `drift_surface.rs`."""
    sources = [p for p in root.rglob("*.rs") if "target" not in p.parts]
    declared: set[str] = set()
    claims: list[tuple[str, str]] = []
    for path in sources:
        try:
            src = path.read_text(errors="ignore")
        except OSError:
            continue
        declared.update(DECLARED_FN.findall(src))
        joined = RS_COMMENT_WRAP.sub(" ", src)
        claims += [(str(path.relative_to(root)), n) for n in PINNED_BY.findall(joined)]
    return [f"{rel}: claims to be pinned by `{name}`, which no module declares — "
            f"an unbacked claim of coverage; name the real mechanism or drop it"
            for rel, name in claims if name not in declared]


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


def guard_entry_sizes(sib: pathlib.Path) -> None:
    for e in bullet_entries(sib.read_text()):
        if len(e) > MAX_ENTRY_CHARS:
            sys.exit(
                f"{sib}: entry is {len(e)} chars (max {MAX_ENTRY_CHARS}) — an entry is "
                f"fact + WHY + pointer; split it or move the argument to its issue: {e[:60]}…"
            )


def edges_block(sib: pathlib.Path) -> list[str]:
    guard_entry_sizes(sib)
    lines = []
    for e in bullet_entries(sib.read_text()):
        m = BOLD_LEAD.match(e)
        if m:
            lines.append(f"- {m.group(1)}{clip(m.group(2), GIST_BUDGET)}".rstrip())
        else:
            lines.append(f"- {clip(e, GIST_BUDGET)}")
    return lines


def lookup_block(sib: pathlib.Path) -> list[str]:
    guard_entry_sizes(sib)
    return [f"- {m.group(1)}" for ln in sib.read_text().splitlines() if (m := QUESTION.match(ln))]


BUILDERS = {"edges": edges_block, "lookup": lookup_block}


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
        else:
            continue
        probe = probe[:-2].rstrip() if probe.endswith(" …") else probe
        if probe and probe not in raw:
            sys.exit(
                f"{guide.name}: index line is not verbatim in {sib.name} — "
                f"unwrap the sibling entry to one line, or fix the builder: {probe[:60]}…"
            )


def tracked_guides(root: pathlib.Path) -> list[str]:
    """The guides git knows about — the ONLY git this module needs."""
    return gitenv.git(
        "-C", str(root), "ls-files", "*CLAUDE.md",
        capture_output=True, text=True, check=True,
    ).stdout.splitlines()


def run(
    root: pathlib.Path,
    tracked: list[str],
    check: bool,
    quiet: bool = False,
    budgets: dict[str, int] | None = None,
) -> int:
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
    # Budgets are measured AFTER regeneration — the run that GROWS a guide must be
    # the one that reds, and an over-budget file must not brick unrelated regen.
    eff = BUDGETS if budgets is None else budgets
    over = (guard_file_budgets(root, eff) + guard_budget_coverage(tracked, eff)
            + guard_pinned_by_claims(root))
    if not quiet:
        for msg in over:
            print(msg)
    if check and drifted:
        if not quiet:
            for rel in drifted:
                print(f"{rel}: index block drifted — `just gen-guides` REBUILDS the block FROM the sibling,\n"
                      f"  discarding any hand-edit inside it; put the change in the SIBLING first")
        return 1
    if not quiet and not over:
        verb = "would rewrite" if check else "rewrote"
        print(f"gen-guides: {verb} {len(drifted)} guide(s)" if drifted else "gen-guides: all index blocks current ✓")
    return 1 if over else 0


def selftest() -> int:
    """Negative-control every failure arm on a throwaway tree — a generator whose
    own fires/does-not-fire contract broke rewrites blocks with garbage quietly.

    The fixture is a plain directory, not a git repo: `tracked_guides` is the only
    git this module does, and injecting its result keeps every control here unable
    to reach a real index at all.
    """
    import tempfile

    fails: list[str] = []
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        (repo / "crate").mkdir(parents=True)
        tracked = ["crate/CLAUDE.md"]

        def stage(rel: str, body: str) -> None:
            (repo / rel).write_text(body)

        def gen(check: bool, budgets: dict[str, int] | None = None) -> int:
            try:
                # The fixture default budgets its one tracked guide generously, so
                # baseline arms stay green while coverage runs on EVERY call.
                return run(repo, tracked, check, quiet=True,
                           budgets={"crate/CLAUDE.md": 10_000} if budgets is None else budgets)
            except SystemExit as e:  # sys.exit from a builder/assert arm
                return 1 if e.code else 0

        guide = (
            "# g\n\n## Known sharp edges\n\n"
            "<!-- edges:start · generated from SHARP-EDGES.md by `just gen-guides` — x -->\n"
            "<!-- edges:end -->\n\n## Where to look\n\n"
            "<!-- lookup:start · generated from WHERE-TO-LOOK.md by `just gen-guides` — x -->\n"
            "<!-- lookup:end -->\n"
        )
        edges = "# e\n\n- **Edge one.** the full text\n"
        lookup = '# w\n\n- "How does it work?" → the answer\n'
        stage("crate/CLAUDE.md", guide)
        stage("crate/SHARP-EDGES.md", edges)
        stage("crate/WHERE-TO-LOOK.md", lookup)

        if gen(False) != 0:
            fails.append("a well-formed fixture must generate")
        first = (repo / "crate/CLAUDE.md").read_text()
        if gen(False) != 0 or (repo / "crate/CLAUDE.md").read_text() != first:
            fails.append("generation must be idempotent")
        if "- How does it work?" not in first or "- **Edge one.**" not in first:
            fails.append("both block kinds must emit")
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

        gen(False)
        stage("crate/src.rs", "/// Pinned by `a_test_that_exists`.\nfn a_test_that_exists() {}\n")
        if gen(True) != 0:
            fails.append("a pinned-by claim naming a declared fn must stay green — if "
                         "this red, the resync above stopped clearing the prior arm's "
                         "deliberate drift and the control is measuring that instead")
        stage("crate/src.rs", "/// Pinned by\n/// `no_such_test`.\nfn unrelated() {}\n")
        if gen(True) != 1:
            fails.append("a pinned-by claim WRAPPED across two `///` lines must still "
                         "red --check — a line-at-a-time matcher passes it silently")
        (repo / "crate/src.rs").unlink()

        stage("crate/SHARP-EDGES.md", "# e\n\n- **Huge.** " + "x " * MAX_ENTRY_CHARS + "\n")
        if gen(False) != 1:
            fails.append("an oversized edges entry must FAIL generation (guard_entry_sizes)")
        stage("crate/SHARP-EDGES.md", edges)
        stage("crate/WHERE-TO-LOOK.md", '# w\n\n- "How?" → ' + "y " * MAX_ENTRY_CHARS + "\n")
        if gen(False) != 1:
            fails.append("an oversized lookup entry must FAIL generation (guard_entry_sizes)")
        stage("crate/WHERE-TO-LOOK.md", lookup)
        stage("crate/SHARP-EDGES.md", "# e\n\n- **Ok.** " + "x " * ((MAX_ENTRY_CHARS - 40) // 2) + "\n")
        if gen(False) != 0:
            fails.append("a just-under-ceiling entry must still generate (the does-not-fire arm)")
        stage("crate/SHARP-EDGES.md", edges)
        gen(False)  # re-sync BEFORE the budget arms — drifted-index red would confound them
        tiny = {"crate/SHARP-EDGES.md": 10}
        if gen(True, budgets=tiny) != 1:
            fails.append("an over-budget file must red --check (guard_file_budgets)")
        if gen(False, budgets=tiny) != 1:
            fails.append("an over-budget file must FAIL generation too")
        if gen(True, budgets={"crate/MISSING.md": 10}) != 1:
            fails.append("a BUDGETS key with no file must red (stale-key fail-open)")
        if gen(True, budgets={"crate/SHARP-EDGES.md": 10_000, "crate/CLAUDE.md": 10_000}) != 0:
            fails.append("an under-budget file must stay green (budget does-not-fire arm)")
        if gen(True, budgets={"crate/SHARP-EDGES.md": 10_000}) != 1:
            fails.append("an unbudgeted tracked guide must red THROUGH run() (coverage wiring)")
        gen(False)
        at_cap = {"crate/CLAUDE.md": (repo / "crate/CLAUDE.md").stat().st_size}
        stage("crate/SHARP-EDGES.md", edges + "- **Edge two.** a second entry\n")
        if gen(False, budgets=at_cap) != 1:
            fails.append("generation that pushes a guide over budget must FAIL, not pass (growth arm)")
        stage("crate/SHARP-EDGES.md", edges)
        gen(False)
        if not guard_budget_coverage(["crate/CLAUDE.md"], {}):
            fails.append("an unbudgeted tracked guide must be reported (coverage fires)")
        if guard_budget_coverage(["crate/CLAUDE.md"], {"crate/CLAUDE.md": 1}):
            fails.append("a budgeted tracked guide must pass (coverage does-not-fire arm)")
        if gen(False) != 0 or gen(True) != 0:
            fails.append("fixture must return to green after the controls")

    if fails:
        print("gen-guides selftest FAILED:")
        for f in fails:
            print(f"  - {f}")
        return 1
    print("gen-guides selftest: every control passed")
    return 0


def main() -> int:
    if "--selftest" in sys.argv[1:]:
        return selftest()
    return run(ROOT, tracked_guides(ROOT), "--check" in sys.argv[1:])


if __name__ == "__main__":
    sys.exit(main())
