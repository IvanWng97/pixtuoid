#!/usr/bin/env python3
"""Diff-scoped advisory for the ast-grep comment-slop rules.

The whole-repo scan has ~5k pre-existing hits (mostly legitimate dense WHY
comments), so a whole-repo gate is wrong. This reports ONLY hits on lines a PR
ADDED or CHANGED vs its base — the new-slop signal — and is ADVISORY: it prints
findings and always exits 0 unless `--gate` is passed. Mirrors the diff-scoping
`just mutants --in-diff` already uses.

Usage: comment-lint.py [BASE_REF] [--gate] [--worktree] [--selftest]
  BASE_REF     git ref to diff against (default: origin/main)
  --gate       exit 1 if any new-code hit is found (default: advisory, exit 0)
  --worktree   diff the WORKING TREE vs BASE (lint uncommitted changes) instead
               of the committed BASE...HEAD range (the default, for CI/PRs)
  --selftest   pin this driver's pathspec + hidden-dir scan on a throwaway repo
"""
import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

# The file types comment-lint covers. One definition: the selftest drives the
# reader that uses it, so a change here cannot pass a stale second copy.
PATHSPEC = ("*.rs", "*.py", "*.pyi")


def added_lines_by_file(
    base: str, worktree: bool, cwd: str | None = None
) -> dict[str, set[int]]:
    """Map each changed .rs/.py/.pyi file → the set of its NEW-side added/changed line
    numbers (1-indexed), parsed from a zero-context diff."""
    # `base...HEAD` = the PR's own commits (merge-base range); `base` alone also
    # folds in uncommitted working-tree edits (local `--worktree` mode).
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
    """Every rule's hits for the tree at `cwd`.

    `--no-ignore hidden`: ast-grep skips dot-dirs like ripgrep, but the diff
    filter matches `.claude/skills/**/*.py` — without it the scan reports a clean
    pass on files it never opened. Pinned by `--selftest`.
    """
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

    The ast-grep RULES have their own fires/does-not-fire pins (`just
    ast-grep-test`); this covers the Python half of the pathspec and the
    hidden-dir scan flag, which live here and had no coverage. Both are
    regressions a refactor could make silently — the hidden-dir case shipped as a
    clean pass over six unread files before it was caught in review.
    """
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

        # The pathspec half, driven through the REAL reader — not a second copy
        # of the extension list, which would pass while the production pathspec
        # silently changed underneath it.
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

    # Fail SOFT if ast-grep isn't installed — this is an advisory (it's excluded
    # from setup-tools' verify loop on purpose), so a dev without it gets a hint,
    # never a raw traceback that reads like the tool is broken.
    if shutil.which("ast-grep") is None:
        print("comment-lint: ast-grep not found — run `just setup-tools` (advisory skipped)")
        return 0

    hits = scan_hits()

    new_hits = []
    for h in hits:
        f = h["file"]
        # ast-grep JSON `start.line` is 0-indexed; the diff is 1-indexed. The hit
        # anchors on the LAST comment of a run, so prepending a line onto an
        # existing 2-run isn't caught (the anchor is unchanged) — a rare,
        # accepted diff-scoping residual; a fresh 3+-run in new code IS caught.
        ln = h["range"]["start"]["line"] + 1
        if ln in added.get(f, ()):  # noqa: SIM118 — set membership
            new_hits.append((f, ln, h["text"].strip().splitlines()[0], h["message"]))

    if not new_hits:
        print("comment-lint: no new 3+-comment runs in the diff vs", base, "✓")
        return 0

    github = "--github" in sys.argv[1:]
    # A run of N>3 comments yields N-2 overlapping ast-grep windows, so this
    # counts flagged LINES, not distinct runs — each still points at its fn.
    print(f"comment-lint: {len(new_hits)} new comment-slop finding(s) in a fn body")
    print("  (advisory — pr-review.prompt.md comment-value factor)")
    for f, ln, txt, msg in sorted(new_hits):
        print(f"  {f}:{ln}: {txt}")
        if github:
            # GitHub Actions annotation (inline on the PR diff); the rule's own
            # `message` is the single source of the guidance text.
            print(f"::warning file={f},line={ln}::{msg}")
    return 1 if gate else 0


if __name__ == "__main__":
    sys.exit(main())
