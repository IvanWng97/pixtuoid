#!/usr/bin/env python3
"""Diff-scoped advisory for the ast-grep comment-slop rules.

The whole-repo scan is dominated by pre-existing, mostly legitimate hits, so a
whole-repo gate is wrong; this reports ONLY hits on lines a PR added or changed.

Usage: comment-lint.py [BASE_REF] [--gate] [--worktree] [--selftest]
  BASE_REF     git ref to diff against (default: origin/main)
  --gate       exit 1 if any new-code hit is found (default: advisory, exit 0)
  --worktree   diff the WORKING TREE vs BASE, not the committed BASE...HEAD range
  --selftest   pin this driver's pathspec + hidden-dir scan on a throwaway repo
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

PATHSPEC = ("*.rs", "*.py", "*.pyi")


def added_lines_by_file(
    base: str, worktree: bool, cwd: str | None = None
) -> dict[str, set[int]]:
    """Map each changed file → its NEW-side added/changed line numbers, 1-indexed."""
    # `base...HEAD` is the merge-base range (the PR's own commits); bare `base`
    # also folds in uncommitted working-tree edits.
    rev = base if worktree else f"{base}...HEAD"
    diff = subprocess.run(
        ["git", "diff", "--unified=0", "--no-color", rev, "--", *PATHSPEC],
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
        git = ["git", "-c", "user.email=t@t", "-c", "user.name=t"]
        subprocess.run([*git, "init", "-q", "-b", "main"], cwd=repo, check=True)
        # The re-parent rule needs these two in the BASE for their doc owner to
        # have changed: `moved.rs` wedges a const in (fires), `renamed.rs`
        # renames the documented fn (must not).
        (repo / "src" / "moved.rs").write_text("/// Doc for the fn.\nfn owner() {}\n")
        (repo / "src" / "renamed.rs").write_text("/// Doc for the fn.\nfn before() {}\n")
        twins = "/// Shared opening line.\n/// One.\nfn a() {}\n\n/// Shared opening line.\n/// Two.\nfn b() {}\n"
        (repo / "src" / "twins.rs").write_text(twins)
        # Staged by PATH, not `-A`: everything else must stay NEW in the fixture
        # commit or the diff-scoped checks above see an empty diff.
        subprocess.run(
            [*git, "add", "src/moved.rs", "src/renamed.rs", "src/twins.rs"], cwd=repo, check=True
        )
        subprocess.run([*git, "commit", "-qm", "base"], cwd=repo, check=True)
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
        subprocess.run([*git, "add", "-A"], cwd=repo, check=True)
        subprocess.run([*git, "commit", "-qm", "fixture"], cwd=repo, check=True)

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

    if fails:
        print("comment-lint selftest FAILED:")
        for f in fails:
            print(f"  - {f}")
        return 1
    print("comment-lint selftest: all checks passed")
    return 0


DOC_RUN_MAX = 10
"""Longest NEW `///`/`//!` run allowed. 94.4% of the tree's existing runs are <= 8
and the longest legitimate one is 35, so this flags only the top few percent —
and diff-scoped, so those existing ones are grandfathered. A new block past it is
not banned, it is made deliberate."""


def doc_run_hits(added: dict[str, set[int]]) -> list[tuple[str, int, int]]:
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
        try:
            src = open(f, errors="ignore").read().splitlines()
        except OSError:
            continue
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
    files = subprocess.run(
        ["git", "diff", "--name-only", rev, "--", "*.rs"],
        capture_output=True, text=True, check=True, cwd=cwd,
    ).stdout.split()

    def at(ref: str | None, path: str) -> str:
        if ref is None:
            try:
                return open(os.path.join(cwd or ".", path), errors="ignore").read()
            except OSError:
                return ""
        r = subprocess.run(
            ["git", "show", f"{ref}:{path}"],
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

    docs = doc_run_hits(added)
    github = "--github" in sys.argv[1:]
    for f, ln, n in docs:
        print(f"comment-lint: {f}:{ln} — new {n}-line doc-comment run (max {DOC_RUN_MAX})")
        if github:
            print(
                f"::warning file={f},line={ln}::{n}-line doc comment "
                f"(max {DOC_RUN_MAX}) — every sentence must carry new information"
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

    if not new_hits and not docs and not moved:
        print("comment-lint: no new 3+-comment runs in the diff vs", base, "✓")
        return 0
    if not new_hits:
        return 1 if gate else 0

    # A long run yields overlapping ast-grep windows, so this counts flagged
    # LINES, not distinct runs.
    print(f"comment-lint: {len(new_hits)} new comment-slop finding(s) in a fn body")
    print("  (advisory — pr-review.prompt.md comment-value factor)")
    for f, ln, txt, msg in sorted(new_hits):
        print(f"  {f}:{ln}: {txt}")
        if github:
            # GitHub Actions annotation — inline on the PR diff.
            print(f"::warning file={f},line={ln}::{msg}")
    return 1 if gate else 0


if __name__ == "__main__":
    sys.exit(main())
