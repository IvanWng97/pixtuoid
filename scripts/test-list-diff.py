#!/usr/bin/env python3
"""Print the tests a PR removes, adds and moves, from CI's `test-list` artifacts.

A refactor row's "no test leaves `cargo nextest list`" check (REVIEW.md), with
no local build. The head list is the PR run's, built on the merge commit, so the
base is the list of the main commit that merge's first parent names, not the
merge-base: main's own test changes since then cancel out. Only API-set run
metadata (`workflow_run.head_sha`, `head_branch`) is trusted; the artifact is
PR-built data, so its claimed parents are checked against the PR head and main.

Usage: `test-list-diff.py PR [--head-list FILE] [--base-list FILE]`, in a clone
with `gh` authenticated. A missing artifact (expired, or its run failed first)
prints the local `cargo nextest list` that replaces it; a local head list is a
build of the head itself, so its base is the merge-base. `--selftest` checks the
pure diff with no network; exit 0 = pass.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys
import tempfile
import traceback

ARTIFACT = "test-list"
MAIN = "main"
# Exit code when an artifact is missing and a local list must stand in.
NO_EVIDENCE = 2

TestId = tuple[str, str]


def parse_list(text: str) -> set[TestId]:
    """`cargo nextest list --message-format json` → {(binary-id, test name)}."""
    suites = json.loads(text)["rust-suites"]
    return {(s["binary-id"], name) for s in suites.values() for name in s["testcases"]}


def leaf(test: TestId) -> str:
    return test[1].rsplit("::", 1)[-1]


def diff(base: set[TestId], head: set[TestId]):
    """(removed, added, moved): a move pairs the ONE removed and ONE added test
    sharing a leaf name; an ambiguous leaf stays in removed and added."""
    removed, added = base - head, head - base
    by_leaf: dict[str, tuple[list[TestId], list[TestId]]] = {}
    for t in removed:
        by_leaf.setdefault(leaf(t), ([], []))[0].append(t)
    for t in added:
        by_leaf.setdefault(leaf(t), ([], []))[1].append(t)
    moved = sorted((r[0], a[0]) for r, a in by_leaf.values() if len(r) == 1 and len(a) == 1)
    for old, new in moved:
        removed.discard(old)
        added.discard(new)
    return sorted(removed), sorted(added), moved


def show(t: TestId) -> str:
    return f"{t[0]} {t[1]}"


def merge_parent(parents: list[str], head: str) -> str | None:
    """The main commit a PR run merged `head` into, when its checkout's parents
    say it built exactly that merge."""
    return parents[0] if len(parents) == 2 and parents[1] == head else None


def on_main(compare_status: str) -> bool:
    """`compare/<commit>...main`'s status: main contains the commit."""
    return compare_status in ("ahead", "identical")


def read_artifact(got: pathlib.Path) -> tuple[str, list[str]] | None:
    """(tests.json, parents), or None when the upload is partial or corrupt."""
    try:
        tests = (got / "tests.json").read_text()
        parse_list(tests)
        parents = (got / "parents.txt").read_text().split() if (got / "parents.txt").exists() else []
    except (OSError, ValueError, KeyError, TypeError):
        return None
    return tests, parents


def gh(*args: str) -> str:
    return subprocess.run(["gh", *args], check=True, capture_output=True, text=True).stdout


def artifact_run(sha: str, branch: str | None) -> int | None:
    """The newest unexpired run whose API-set head is `sha` (on `branch`, if given)."""
    rows = gh(
        "api", "--paginate",
        f"repos/{{owner}}/{{repo}}/actions/artifacts?name={ARTIFACT}&per_page=100",
        "--jq", ".artifacts[] | select(.expired | not) | .workflow_run"
        " | [.id, .head_sha, .head_branch] | @json",
    )
    runs = [
        run for run, head, ref in map(json.loads, rows.splitlines())
        if head == sha and branch in (None, ref)
    ]
    return max(runs, default=None)


def download(run: int, into: pathlib.Path) -> pathlib.Path:
    gh("run", "download", str(run), "--name", ARTIFACT, "--dir", str(into))
    return into


def fallback(side: str, sha: str, flag: str) -> int:
    print(
        f"no {ARTIFACT} artifact for the {side} {sha}: build its list locally, then pass\n"
        f"  {flag} FILE\n"
        f"  git worktree add --detach <dir> {sha}\n"
        f"  (cd <dir> && cargo nextest list --workspace --message-format json) > FILE",
        file=sys.stderr,
    )
    return NO_EVIDENCE


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return selftest()
    cli = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    cli.add_argument("pr")
    cli.add_argument("--head-list", type=pathlib.Path)
    cli.add_argument("--base-list", type=pathlib.Path)
    args = cli.parse_args(argv)
    pr = args.pr
    head = gh("pr", "view", pr, "--json", "headRefOid", "--jq", ".headRefOid").strip()
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="test-list-"))

    if args.head_list:
        head_list = args.head_list.read_text()
        base = gh("api", f"repos/{{owner}}/{{repo}}/compare/{MAIN}...{head}",
                  "--jq", ".merge_base_commit.sha").strip()
        print(f"head {head} (local list), base merge-base {base}")
    else:
        run = artifact_run(head, None)
        if run is None:
            return fallback("PR head", head, "--head-list")
        art = read_artifact(download(run, tmp / "head"))
        if art is None:
            return fallback("PR head", head, "--head-list")
        head_list, parents = art
        base = merge_parent(parents, head)
        if base is None:
            print(f"run {run}'s checkout was not a merge of {head}: {parents}", file=sys.stderr)
            return 1
        status = gh("api", f"repos/{{owner}}/{{repo}}/compare/{base}...{MAIN}", "--jq", ".status")
        if not on_main(status.strip()):
            print(f"run {run}'s merge parent {base} is not on {MAIN}", file=sys.stderr)
            return 1
        print(f"head {head} run {run} (merged with {MAIN} {base})")

    if args.base_list:
        base_list = args.base_list.read_text()
        print(f"base {base} (local list)")
    else:
        run = artifact_run(base, MAIN)
        if run is None:
            return fallback(f"{MAIN} commit", base, "--base-list")
        art = read_artifact(download(run, tmp / "base"))
        if art is None:
            return fallback(f"{MAIN} commit", base, "--base-list")
        base_list = art[0]
        print(f"base {MAIN} {base} run {run}")

    removed, added, moved = diff(parse_list(base_list), parse_list(head_list))
    for title, rows in (
        ("removed", [show(t) for t in removed]),
        ("added", [show(t) for t in added]),
        ("moved", [f"{show(a)} -> {show(b)}" for a, b in moved]),
    ):
        print(f"{title} ({len(rows)}):")
        for row in rows:
            print(f"  {row}")
    return 0


FAILS: list[str] = []


def check(label: str, got, want) -> None:
    if got != want:
        FAILS.append(f"{label}: got {got!r}, want {want!r}")


def selftest() -> int:
    try:
        suites = {"rust-suites": {"core": {"binary-id": "core", "testcases": {"a::t": {}, "b::u": {}}}}}
        check("parse", parse_list(json.dumps(suites)), {("core", "a::t"), ("core", "b::u")})
        base = {("x", "m::kept"), ("x", "m::gone"), ("x", "m::moves"), ("x", "p::dup"), ("x", "q::dup")}
        head = {("x", "m::kept"), ("x", "m::new"), ("y", "n::moves"), ("x", "r::dup")}
        check(
            "diff",
            diff(base, head),
            (
                [("x", "m::gone"), ("x", "p::dup"), ("x", "q::dup")],
                [("x", "m::new"), ("x", "r::dup")],
                [(("x", "m::moves"), ("y", "n::moves"))],
            ),
        )
        check("identical", diff(base, base), ([], [], []))
        check("merge of head", merge_parent(["m", "h"], "h"), "m")
        check("parents swapped", merge_parent(["h", "m"], "h"), None)
        check("not a merge", merge_parent(["h"], "h"), None)
        for status, want in (("ahead", True), ("identical", True), ("behind", False), ("diverged", False)):
            check(f"on main: {status}", on_main(status), want)
        with tempfile.TemporaryDirectory() as d:
            got = pathlib.Path(d)
            (got / "tests.json").write_text("")
            check("an empty list is no artifact", read_artifact(got), None)
            (got / "tests.json").write_text(json.dumps(suites))
            check("parents missing", read_artifact(got), (json.dumps(suites), []))
    except Exception:
        FAILS.append(traceback.format_exc())
    for f in FAILS:
        print(f, file=sys.stderr)
    print(f"test-list-diff selftest: {'FAIL' if FAILS else 'ok'}")
    return 1 if FAILS else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
