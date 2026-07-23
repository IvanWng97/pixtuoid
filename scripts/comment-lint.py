#!/usr/bin/env python3
"""Diff-scoped advisory for the ast-grep comment-slop rule.

The whole-repo scan has ~5k pre-existing hits (mostly legitimate dense WHY
comments), so a whole-repo gate is wrong. This reports ONLY hits on lines a PR
ADDED or CHANGED vs its base — the new-slop signal — and is ADVISORY: it prints
findings and always exits 0 unless `--gate` is passed. Mirrors the diff-scoping
`just mutants --in-diff` already uses.

Usage: comment-lint.py [BASE_REF] [--gate] [--worktree]
  BASE_REF     git ref to diff against (default: origin/main)
  --gate       exit 1 if any new-code hit is found (default: advisory, exit 0)
  --worktree   diff the WORKING TREE vs BASE (lint uncommitted changes) instead
               of the committed BASE...HEAD range (the default, for CI/PRs)
"""
import json
import re
import shutil
import subprocess
import sys


def added_lines_by_file(base: str, worktree: bool) -> dict[str, set[int]]:
    """Map each changed .rs file → the set of its NEW-side added/changed line
    numbers (1-indexed), parsed from a zero-context diff."""
    # `base...HEAD` = the PR's own commits (merge-base range); `base` alone also
    # folds in uncommitted working-tree edits (local `--worktree` mode).
    rev = base if worktree else f"{base}...HEAD"
    diff = subprocess.run(
        ["git", "diff", "--unified=0", "--no-color", rev, "--", "*.rs"],
        capture_output=True,
        text=True,
        check=True,
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


def main() -> int:
    flags = {"--gate", "--worktree", "--github"}
    args = [a for a in sys.argv[1:] if a not in flags]
    gate = "--gate" in sys.argv[1:]
    worktree = "--worktree" in sys.argv[1:]
    base = args[0] if args else "origin/main"

    added = added_lines_by_file(base, worktree)
    if not any(added.values()):
        print("comment-lint: no added/changed Rust lines vs", base)
        return 0

    # Fail SOFT if ast-grep isn't installed — this is an advisory (it's excluded
    # from setup-tools' verify loop on purpose), so a dev without it gets a hint,
    # never a raw traceback that reads like the tool is broken.
    if shutil.which("ast-grep") is None:
        print("comment-lint: ast-grep not found — run `just setup-tools` (advisory skipped)")
        return 0

    scan = subprocess.run(
        ["ast-grep", "scan", "--json"], capture_output=True, text=True, check=True
    ).stdout
    hits = json.loads(scan)

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
