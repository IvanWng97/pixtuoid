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
        git = ["git", "-c", "user.email=t@t", "-c", "user.name=t"]
        subprocess.run([*git, "init", "-q", "-b", "main"], cwd=repo, check=True)
        subprocess.run([*git, "commit", "-q", "--allow-empty", "-m", "base"], cwd=repo, check=True)
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
    script can see the place most bloat lands.
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
        for i, raw in enumerate(src, start=1):
            if re.match(r"\s*(///|//!)", raw):
                run_start = run_start or i
                continue
            if run_start and i - run_start > DOC_RUN_MAX:
                if any(n in lines for n in range(run_start, i)):
                    out.append((f, run_start, i - run_start))
            run_start = None
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
    if shutil.which("ast-grep") is None:
        print("comment-lint: ast-grep not found — run `just setup-tools` (advisory skipped)")
        return 0

    hits = scan_hits()

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
    for f, ln, n in docs:
        print(f"comment-lint: {f}:{ln} — new {n}-line doc-comment run (max {DOC_RUN_MAX})")

    if not new_hits and not docs:
        print("comment-lint: no new 3+-comment runs in the diff vs", base, "✓")
        return 0
    if not new_hits:
        return 1 if gate else 0

    github = "--github" in sys.argv[1:]
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
