#!/usr/bin/env python3
"""Diff-scoped comment checks, reporting only on lines a PR added or changed.

The whole-repo scan is dominated by pre-existing, mostly legitimate hits, so a
whole-repo gate is wrong.

Four arms. Three need only git and python and BLOCK under `--gate`: an over-long
new `///`/`//!` run, a doc block re-homed onto a different item, and the diff's
narrative-prose share. The fourth wraps the ast-grep rules and is advisory
everywhere — it needs an npm install, which must never sit inside `ci-gate`.

Usage: comment-lint.py [BASE_REF] [--gate] [--worktree] [--github] [--selftest]
  BASE_REF     git ref to diff against (default: origin/main)
  --gate       exit 1 on the three deterministic arms (default: exit 0 always)
  --worktree   diff the WORKING TREE vs BASE, not the committed BASE...HEAD range
  --github     emit ::warning:: annotations onto the PR diff
  --selftest   pin every arm, both directions, on a throwaway repo
"""

from __future__ import annotations

import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

import gitenv

PATHSPEC = ("*.rs", "*.py", "*.pyi")


def added_lines_by_file(
    base: str, worktree: bool, cwd: str | None = None
) -> dict[str, set[int]]:
    """Map each changed file → its NEW-side added/changed line numbers, 1-indexed."""
    # `base...HEAD` is the merge-base range (the PR's own commits); bare `base`
    # also folds in uncommitted working-tree edits.
    rev = base if worktree else f"{base}...HEAD"
    diff = gitenv.git(
        "diff", "--unified=0", "--no-color", rev, "--", *PATHSPEC,
        capture_output=True,
        text=True,
        check=True,
        cwd=cwd,
    ).stdout
    added: dict[str, set[int]] = {}
    cur: str | None = None
    hunk = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")
    for line in diff.splitlines():
        if line.startswith("+++ b/"):
            cur = line[6:]
            added.setdefault(cur, set())
        elif cur and (m := hunk.match(line)):
            start = int(m.group(1))
            count = 1 if m.group(2) is None else int(m.group(2))
            added[cur].update(range(start, start + count))
    return added



def scan_hits(cwd: str | None = None) -> list[dict]:
    """`--no-ignore hidden`: ast-grep skips dot-dirs like ripgrep, but the diff
    filter matches `.claude/skills/**/*.py` — without it the scan reports a clean
    pass on files it never opened."""
    out = subprocess.run(
        ["ast-grep", "scan", "--json", "--no-ignore", "hidden"],
        capture_output=True,
        text=True,
        check=True,
        cwd=cwd,
    ).stdout
    return json.loads(out)


def selftest() -> int:
    """Pin this DRIVER's two behaviors: which files it diffs, and which it scans.
    The ast-grep RULES have their own pins (`just ast-grep-test`)."""
    root = pathlib.Path(__file__).resolve().parent.parent
    fails: list[str] = []
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        (repo / ".claude" / "skills").mkdir(parents=True)
        (repo / "src").mkdir()
        # The rules travel with the fixture; ast-grep resolves them from cwd.
        shutil.copytree(root / ".ast-grep", repo / ".ast-grep")
        shutil.copy(root / "sgconfig.yml", repo / "sgconfig.yml")
        run3 = "def f():\n    # one\n    # two\n    # three\n    return 1\n"
        (repo / "src" / "plain.py").write_text(run3)
        (repo / ".claude" / "skills" / "hidden.py").write_text(run3)
        (repo / "src" / "keep.rs").write_text(
            "fn f() -> u8 {\n    // one\n    // two\n    // three\n    1\n}\n"
        )
        # BOTH directions for the doc-run rule: one run past DOC_RUN_MAX (split by
        # a blank line, which does not end a doc comment), one at the limit, and a
        # `////` banner rustc does not treat as a doc comment. Lengths are
        # LITERAL: a fixture derived from DOC_RUN_MAX moves with the constant, so
        # retuning it left the whole rule passing against itself.
        if DOC_RUN_MAX != 10:
            fails.append(
                f"DOC_RUN_MAX is {DOC_RUN_MAX}, but these fixtures pin 10 — retune both together"
            )
        long_doc = "".join(f"/// l{n}\n" for n in range(5))
        long_doc += "\n" + "".join(f"/// m{n}\n" for n in range(6))
        (repo / "src" / "docs.rs").write_text(long_doc + "pub const A: u8 = 0;\n")
        (repo / "src" / "docs_ok.rs").write_text(
            "".join(f"/// l{n}\n" for n in range(10)) + "pub const B: u8 = 0;\n"
        )
        (repo / "src" / "banner.rs").write_text(
            "".join(f"//// l{n}\n" for n in range(14)) + "pub const C: u8 = 0;\n"
        )
        ident = ("-c", "user.email=t@t", "-c", "user.name=t")
        gitenv.git(*ident, "init", "-q", "-b", "main", cwd=repo, check=True)
        # The re-parent rule needs these two in the BASE for their doc owner to
        # have changed: `moved.rs` wedges a const in (fires), `renamed.rs`
        # renames the documented fn (must not).
        (repo / "src" / "moved.rs").write_text("/// Doc for the fn.\nfn owner() {}\n")
        (repo / "src" / "renamed.rs").write_text("/// Doc for the fn.\nfn before() {}\n")
        twins = "/// Shared opening line.\n/// One.\nfn a() {}\n\n/// Shared opening line.\n/// Two.\nfn b() {}\n"
        (repo / "src" / "twins.rs").write_text(twins)
        # Staged by PATH, not `-A`: everything else must stay NEW in the fixture
        # commit or the diff-scoped checks above see an empty diff.
        gitenv.git(*ident, "add", "src/moved.rs", "src/renamed.rs", "src/twins.rs", cwd=repo, check=True)
        gitenv.git(*ident, "commit", "-qm", "base", cwd=repo, check=True)
        (repo / "src" / "moved.rs").write_text(
            "/// Doc for the fn.\nconst WEDGE: u8 = 0;\n\nfn owner() {}\n"
        )
        (repo / "src" / "renamed.rs").write_text("/// Doc for the fn.\nfn after() {}\n")
        # SWAPPED: with a first-line key the second block's owner overwrites
        # nothing and the first's changes, so the pair reads as a re-parent.
        (repo / "src" / "twins.rs").write_text(
            "/// Shared opening line.\n/// Two.\nfn b() {}\n\n"
            "/// Shared opening line.\n/// One.\nfn a() {}\n"
        )
        gitenv.git(*ident, "add", "-A", cwd=repo, check=True)
        gitenv.git(*ident, "commit", "-qm", "fixture", cwd=repo, check=True)

        # Only this arm needs ast-grep, so skipping it lets the gate run in the
        # PROTECTED ci-lint group; the advisory job that owns ast-grep still runs
        # the full selftest.
        if shutil.which("ast-grep") is None:
            print("comment-lint selftest: ast-grep absent — SKIPPED the scan-pathspec arm")
        else:
            scanned = {h["file"] for h in scan_hits(cwd=str(repo))}
            for path, why in (
                ("src/plain.py", "a .py in the tree is scanned"),
                (".claude/skills/hidden.py", "a .py under a DOT-DIR is scanned (--no-ignore hidden)"),
                ("src/keep.rs", "Rust coverage is unchanged"),
            ):
                if path not in scanned:
                    fails.append(f"{why}: {path} missing from {sorted(scanned)}")

        # Driven through the REAL reader, not a second copy of the extension list,
        # which would pass while the production pathspec changed underneath it.
        added = added_lines_by_file("HEAD~1", worktree=False, cwd=str(repo))
        for path in ("src/plain.py", ".claude/skills/hidden.py", "src/keep.rs"):
            if not added.get(path):
                fails.append(f"the pathspec drops {path}: {sorted(added)}")

        # The doc-run rule, BOTH directions — a gate that has only ever been seen
        # to pass is not known to work.
        cwd = os.getcwd()
        try:
            os.chdir(repo)
            flagged = {f for f, _, _ in doc_run_hits(added)}
        finally:
            os.chdir(cwd)
        if "src/docs.rs" not in flagged:
            fails.append(f"a >{DOC_RUN_MAX}-line doc run must fire: {sorted(flagged)}")
        for path, why in (
            ("src/docs_ok.rs", f"a {DOC_RUN_MAX}-line run is at the limit, not over"),
            ("src/banner.rs", "`////` is an ordinary comment to rustc"),
        ):
            if path in flagged:
                fails.append(f"{why}: {path} must NOT fire")

        # The re-parent rule, BOTH directions.
        reparented = {f for f, _, _, _ in reparented_doc_hits("HEAD~1", False, str(repo))}
        if "src/moved.rs" not in reparented:
            fails.append(f"a doc block wedged off its fn must fire: {sorted(reparented)}")
        if "src/renamed.rs" in reparented:
            fails.append("a renamed fn keeping its doc must NOT fire: src/renamed.rs")
        if "src/twins.rs" in reparented:
            fails.append("two blocks sharing an opening line must NOT fire: src/twins.rs")

        # The prose-share rule. Each fixture is its OWN commit so the real
        # `prose_share` measures it — a hand-rolled per-file recount would pass
        # while the production classifier changed underneath it.
        # LITERAL 20, like the DOC_RUN_MAX fixtures above: derived from the
        # constant, the fixture moves with it and the rule passes against itself.
        if PROSE_MIN_CODE != 20:
            fails.append(
                f"PROSE_MIN_CODE is {PROSE_MIN_CODE}, but these fixtures pin 20 — retune both"
            )
        code_floor = "".join(f"fn f{n}() {{}}\n" for n in range(20))

        def prose_case(name: str, body: str) -> tuple[int, int]:
            (repo / "src" / f"{name}.rs").write_text(body)
            gitenv.git(*ident, "add", "-A", cwd=repo, check=True)
            gitenv.git(*ident, "commit", "-qm", name, cwd=repo, check=True)
            cwd2 = os.getcwd()
            try:
                os.chdir(repo)
                pr, co, _ = prose_share("HEAD~1", worktree=False)
                return pr, co
            finally:
                os.chdir(cwd2)

        def over(p: int, c: int) -> bool:
            return c >= PROSE_MIN_CODE and p * 100 > (p + c) * PROSE_MAX_PCT

        for name, body, want, why in (
            ("prosey", "".join(f"// n{n}\n" for n in range(9)) + code_floor,
             True, f"9 narrative lines over {PROSE_MIN_CODE} code must exceed"),
            ("lean", "// one\n" + code_floor,
             False, f"1 narrative line over {PROSE_MIN_CODE} code must NOT exceed"),
            # `///` is REQUIRED by `missing_docs`; charging for it would set two
            # gates against each other.
            ("docsonly", "".join(f"/// d{n}\n" for n in range(9)) + code_floor,
             False, "doc comments must not count as prose"),
            # The `////` bypass's twin: block prose classified as code would hide
            # itself AND pad the denominator.
            ("blocky", "/*\n" + "".join(f" * n{n}\n" for n in range(7)) + " */\n" + code_floor,
             True, "`/* */` prose must count as prose"),
            # `////` is an ordinary comment to rustc, so a `startswith("///")`
            # test exempts it and `doc_run_hits`' regex does not — the same
            # one-keystroke bypass, on the other arm.
            ("banner", "".join(f"//// n{n}\n" for n in range(9)) + code_floor,
             True, "`////` is prose, not a doc comment"),
            ("tiny", "// one\n// two\nfn a() {}\nfn b() {}\n",
             False, f"under {PROSE_MIN_CODE} code lines the ratio must not be judged"),
        ):
            p, c = prose_case(name, body)
            if over(p, c) is not want:
                fails.append(f"{why}: got {p} prose / {c} code")

        # An OPEN `/*` whose `*/` is CONTEXT, then a second file of plain code.
        # Named so the block-opener sorts FIRST: `git diff` walks paths in order,
        # and a plain-code file ahead of it is counted before the block opens,
        # which is how the first version of this fixture passed unfixed.
        # `--unified=0` hands the walker non-contiguous added lines, so without a
        # reset at each header the open block charges the rest of the DIFF —
        # including another file's code — as prose. The `blocky` case above
        # cannot catch it: it adds the `/*` and the `*/` together.
        (repo / "src" / "a_openblock.rs").write_text("*/\nfn kept() {}\n")
        gitenv.git(*ident, "add", "-A", cwd=repo, check=True)
        gitenv.git(*ident, "commit", "-qm", "openblock-base", cwd=repo, check=True)
        (repo / "src" / "a_openblock.rs").write_text(
            "/*\n" + "".join(f" * n{n}\n" for n in range(3)) + "*/\nfn kept() {}\n"
        )
        (repo / "src" / "b_after.rs").write_text(code_floor)
        p, c = prose_case("leak", "")
        if p != 4 or c < 20:
            fails.append(
                f"an unterminated `/*` must not leak past the file header: got {p} prose / {c} code"
            )

        # Which arms BLOCK, both directions. Without this the ast-grep arm can
        # silently become a hard gate on a machine that happens to have it.
        if gate_fails(True, [], [], False):
            fails.append("nothing found must not fail the gate")
        if gate_fails(False, ["d"], ["m"], True):
            fails.append("without --gate nothing may fail")
        for arm, args in (
            ("doc-run", (["d"], [], False)),
            ("re-parent", ([], ["m"], False)),
            ("prose", ([], [], True)),
        ):
            if not gate_fails(True, *args):
                fails.append(f"the {arm} arm must block under --gate")

    # Outside the fixture block: this spawns the whole selftest again.
    if leak := gitenv.ambient_git_control(pathlib.Path(__file__)):
        fails.append(leak)

    if fails:
        print("comment-lint selftest FAILED:")
        for f in fails:
            print(f"  - {f}")
        return 1
    print("comment-lint selftest: all checks passed")
    return 0


DOC_RUN_MAX = 10
"""Longest NEW `///`/`//!` run allowed — above the tree's own long tail, whose
longest legitimate run is 35. Diff-scoped, so existing runs are grandfathered,
and a new block past it is not banned, only made deliberate."""


def doc_run_hits(
    added: dict[str, set[int]], worktree: bool = True, cwd: str | None = None
) -> list[tuple[str, int, int]]:
    """New `///`/`//!` runs longer than DOC_RUN_MAX, as (file, start_line, len).

    The ast-grep rules deliberately exclude doc comments, so nothing else in this
    script can see the place most bloat lands. `////` is NOT a doc comment to
    rustc, and a blank line does not end one, so both are handled here or the
    check is a one-keystroke bypass.
    """
    out = []
    for f, lines in added.items():
        if not f.endswith(".rs") or not lines:
            continue
        # `added`'s line numbers come from the diff, so the source must be read at
        # the SAME revision — a dirty tree shifts them under an unchanged block.
        if worktree:
            try:
                src = open(os.path.join(cwd or ".", f), errors="ignore").read().splitlines()
            except OSError:
                continue
        else:
            r = gitenv.git(
                "show", f"HEAD:{f}", capture_output=True, text=True, check=False, cwd=cwd
            )
            if r.returncode != 0:
                continue
            src = r.stdout.splitlines()
        run_start = None
        run_len = 0

        def close(run_start: int | None, run_len: int) -> None:
            # Anchor on the run's FIRST line, like the ast-grep path: flagging a
            # whole pre-existing block because one line inside it was touched is
            # what sends an author to trim a legitimate WHY.
            if run_start and run_len > DOC_RUN_MAX and run_start in lines:
                out.append((f, run_start, run_len))

        for i, raw in enumerate(src, start=1):
            t = raw.strip()
            if re.match(r"(///|//!)($|[^/])", t):
                run_start = run_start or i
                run_len += 1
            elif t == "" and run_start:
                continue  # a blank line splits the TEXT, not the doc comment
            else:
                close(run_start, run_len)
                run_start, run_len = None, 0
        close(run_start, run_len)
    return out


ITEM = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+|async\s+|unsafe\s+|extern\s+\"[^\"]*\"\s+)*"
    r"(fn|struct|enum|trait|impl|type|const|static|mod|macro_rules!)\s+([A-Za-z0-9_]+)"
)


def doc_owners(src: str) -> dict[str, str]:
    """Map each `///` block's FULL text → the `kind name` it documents.

    Keyed on the whole block, not its first line: two blocks in one file can
    legitimately open with the same sentence, and a first-line key merges them
    into a re-parent that never happened.
    """
    out: dict[str, str] = {}
    block: list[str] = []
    for raw in src.splitlines():
        t = raw.strip()
        if t.startswith("///"):
            block.append(t)
        elif t.startswith("#[") or t == "":
            continue  # attributes and blank lines do not end a doc block
        else:
            if block and (m := ITEM.match(raw)):
                out.setdefault("\n".join(block), f"{m.group(1)} {m.group(2)}")
            block = []
    return out


PROSE_MAX_PCT = 15
"""Ceiling on NARRATIVE prose as a share of a diff's new Rust lines.

Calibrate against the FLOW rate (added prose per added code) that this measures,
never the whole-tree stock — the stock is dominated by old code and runs 2-3x
lower, so a ceiling set from it flags almost every merge.

`///` and `//!` count as NEITHER, like blank lines: `missing_docs` REQUIRES a doc
on every published `pub` item, so charging for one sets two gates against each
other. Counting them as code instead lets a doc-heavy diff buy prose allowance.

Rust only: the `.py` here and the site's TS answer to different conventions.
"""

PROSE_MIN_CODE = 20

#: How many of the counted prose lines the blocking message lists before eliding.
PROSE_SAMPLE_MAX = 12

"""Added code lines below which the ratio is not judged — a handful of lines is
all-or-nothing, and would fail on arithmetic rather than on slop."""


def prose_share(
    base: str, worktree: bool, cwd: str | None = None
) -> tuple[int, int, list[str]]:
    """`(narrative_prose, code)` line counts among the diff's ADDED Rust lines.

    Blank lines and doc comments count as neither (see [`PROSE_MAX_PCT`]). Test
    files are included: a test's prose rots the same way, and exempting them just
    moves the prose there.
    """
    rev = base if worktree else f"{base}...HEAD"
    diff = gitenv.git(
        "diff", "--unified=0", "--no-color", rev, "--", "*.rs",
        capture_output=True, text=True, check=True, cwd=cwd,
    ).stdout
    prose = code = 0
    in_block = False
    samples: list[str] = []
    cur_file = "?"
    for line in diff.splitlines():
        # A file or hunk header closes any open block. `--unified=0` makes the
        # added lines non-contiguous, so a `/*` whose `*/` is CONTEXT would
        # otherwise charge every later added line in the whole diff as prose.
        if line.startswith("+++"):
            cur_file = line[6:] if line.startswith("+++ b/") else line[4:]
            in_block = False
            continue
        if line.startswith("@@"):
            in_block = False
            continue
        if not line.startswith("+"):
            continue
        t = line[1:].strip()
        # `/* */` counted as code would BOTH hide the prose and inflate the
        # denominator — the one-keystroke bypass `doc_run_hits` guards against.
        if in_block:
            prose += 1
            samples.append(f"{cur_file}: {t}")
            in_block = "*/" not in t
        elif t.startswith("/*"):
            prose += 1
            samples.append(f"{cur_file}: {t}")
            in_block = "*/" not in t
        elif re.match(r"(///|//!)($|[^/])", t):
            continue
        elif t.startswith("//"):
            prose += 1
            samples.append(f"{cur_file}: {t}")
        elif t:
            code += 1
    return prose, code, samples


def reparented_doc_hits(
    base: str, worktree: bool, cwd: str | None = None
) -> list[tuple[str, str, str, str]]:
    """Doc blocks that changed OWNER, as (file, first_line, old_owner, new_owner).

    Inserting an item between an existing `///` block and what it documented
    re-homes the block onto the newcomer, and nothing else sees it: both sides
    compile, rustdoc renders, and the diff shows only the insertion.

    A hit also requires the old owner to still EXIST in the new file, which is
    what separates an accidental detachment from a plain rename.
    """
    rev = base if worktree else f"{base}...HEAD"
    files = gitenv.git(
        "diff", "--name-only", rev, "--", "*.rs",
        capture_output=True, text=True, check=True, cwd=cwd,
    ).stdout.split()

    def at(ref: str | None, path: str) -> str:
        if ref is None:
            try:
                return open(os.path.join(cwd or ".", path), errors="ignore").read()
            except OSError:
                return ""
        r = gitenv.git(
            "show", f"{ref}:{path}",
            capture_output=True,
            text=True,
            check=False,
            cwd=cwd,
        )
        return r.stdout if r.returncode == 0 else ""

    out = []
    for path in files:
        old = doc_owners(at(base, path))
        new_src = at(None if worktree else "HEAD", path)
        new = doc_owners(new_src)
        still_there = {o for o in new.values()} | {
            f"{m.group(1)} {m.group(2)}"
            for line in new_src.splitlines()
            if (m := ITEM.match(line))
        }
        for doc, owner in old.items():
            if doc in new and new[doc] != owner and owner in still_there:
                out.append((path, doc, owner, new[doc]))
    return out


def gate_fails(gate: bool, docs: list, moved: list, over_prose: bool) -> bool:
    """Whether `--gate` should exit non-zero — the ast-grep arm deliberately absent.

    That arm needs an npm install, so it runs only in advisory `ci-supplemental`;
    letting it block locally would make `just lint` stricter than the CI job that
    shares its recipe.
    """
    return gate and (bool(docs) or bool(moved) or over_prose)


def main() -> int:
    if "--selftest" in sys.argv[1:]:
        return selftest()

    flags = {"--gate", "--worktree", "--github"}
    args = [a for a in sys.argv[1:] if a not in flags]
    gate = "--gate" in sys.argv[1:]
    worktree = "--worktree" in sys.argv[1:]
    base = args[0] if args else "origin/main"

    added = added_lines_by_file(base, worktree)
    if not any(added.values()):
        print("comment-lint: no added/changed Rust or Python lines vs", base)
        return 0

    # Fail SOFT if ast-grep isn't installed — this is an advisory, so a dev without
    # it gets a hint, never a traceback that reads like the tool is broken.
    docs_only = shutil.which("ast-grep") is None
    if docs_only:
        print("comment-lint: ast-grep not found — run `just setup-tools` (`//` rules skipped)")

    hits = [] if docs_only else scan_hits()

    new_hits = []
    for h in hits:
        f = h["file"]
        # ast-grep JSON `start.line` is 0-indexed; the diff is 1-indexed. The hit
        # anchors on the LAST comment of a run, so prepending onto an existing
        # 2-run isn't caught — an accepted residual, not a bug to fix here.
        ln = h["range"]["start"]["line"] + 1
        if ln in added.get(f, ()):  # noqa: SIM118 — set membership
            new_hits.append((f, ln, h["text"].strip().splitlines()[0], h["message"]))

    docs = doc_run_hits(added, worktree)
    github = "--github" in sys.argv[1:]
    for f, ln, n in docs:
        print(f"comment-lint: {f}:{ln} — new {n}-line doc-comment run (max {DOC_RUN_MAX})")
        if github:
            print(
                f"::warning file={f},line={ln}::{n}-line doc comment "
                f"(max {DOC_RUN_MAX}) — every sentence must carry new information"
            )

    prose, code, prose_lines = prose_share(base, worktree)
    over_prose = code >= PROSE_MIN_CODE and prose * 100 > (prose + code) * PROSE_MAX_PCT
    if over_prose:
        pct = 100 * prose / (prose + code)
        print(
            f"comment-lint: {pct:.1f}% of this diff's new Rust lines are narrative "
            f"prose ({prose} prose / {code} code, max {PROSE_MAX_PCT}%; doc comments exempt)"
        )
        # This arm BLOCKS, so it owes the reader a place to look and a remedy —
        # the ratio alone is the one message with neither.
        print(
            "  cut restated WHAT, migration traces and unmeasured numbers, "
            "not legitimate WHY. The lines counted:"
        )
        for s in prose_lines[:PROSE_SAMPLE_MAX]:
            print(f"  {s}")
        if len(prose_lines) > PROSE_SAMPLE_MAX:
            print(f"  … and {len(prose_lines) - PROSE_SAMPLE_MAX} more")
        if "--github" in sys.argv[1:]:
            print(
                f"::warning::{pct:.1f}% prose in new Rust ({prose}/{prose + code}, "
                f"max {PROSE_MAX_PCT}%) — cut restated WHAT, migration traces and "
                f"unmeasured numbers, not legitimate WHY"
            )

    moved = reparented_doc_hits(base, worktree)
    for f, doc, old_owner, new_owner in moved:
        print(f"comment-lint: {f} — doc block re-homed from `{old_owner}` to `{new_owner}`")
        print(f"    {doc.splitlines()[0]}")
        if github:
            print(
                f"::warning file={f}::this doc block documented `{old_owner}` on "
                f"{base} and now sits on `{new_owner}` — move it back, or give the "
                f"newcomer its own"
            )

    blocking = gate_fails(gate, docs, moved, over_prose)
    if not new_hits and not docs and not moved and not over_prose:
        print("comment-lint: no new 3+-comment runs in the diff vs", base, "✓")
        return 0
    if not new_hits:
        return 1 if blocking else 0

    # A long run yields overlapping ast-grep windows, so this counts flagged
    # LINES, not distinct runs.
    print(f"comment-lint: {len(new_hits)} new comment-slop finding(s) in a fn body")
    print("  (advisory — pr-review.prompt.md comment-value factor)")
    for f, ln, txt, msg in sorted(new_hits):
        print(f"  {f}:{ln}: {txt}")
        if github:
            # GitHub Actions annotation — inline on the PR diff.
            print(f"::warning file={f},line={ln}::{msg}")
    return 1 if blocking else 0


if __name__ == "__main__":
    sys.exit(main())
