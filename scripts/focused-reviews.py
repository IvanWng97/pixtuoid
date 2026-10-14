#!/usr/bin/env python3
"""The merge gate's focused reviews (docs/CONTRIBUTING.md#the-merge-gate).

Each row of REVIEW.md's "Focused reviews" table whose Paths a PR touches needs
a maintainer's comment at the PR's head opening `<!-- focused-review:<row>:<sha> -->`.
Usage: `focused-reviews.py <owner/repo> <pr> <head sha>`, reading the PR through
`gh api`; exits 1 naming each row still owed. `--selftest` runs the pure
halves against the real REVIEW.md and tree.
"""

import fnmatch
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SECTION = "## Focused reviews"
# The repo's owner, its org's members and invited collaborators, as GitHub's
# CommentAuthorAssociation enum describes each (GraphQL schema introspection).
MAINTAINERS = {"OWNER", "MEMBER", "COLLABORATOR"}
MARKER = re.compile(r"<!-- focused-review:([a-z0-9-]+):([0-9a-f]{40}) -->")


def row_id(first_cell: str) -> str:
    """The row's first column up to any colon or parenthesis, lowercased, each
    run of non-alphanumerics one `-`."""
    head = re.split(r"[:(]", first_cell, maxsplit=1)[0]
    return re.sub(r"[^a-z0-9]+", "-", head.lower()).strip("-")


def rows(review: str) -> dict[str, list[str]]:
    """Each row's id and its Paths globs, after the table's header and rule."""
    parts = review.split(f"\n{SECTION}\n", 1)
    if len(parts) != 2:
        raise ValueError(f"REVIEW.md has no {SECTION!r} section")
    section = parts[1].split("\n## ", 1)[0]
    table = [line for line in section.splitlines() if line.startswith("|")][2:]
    out: dict[str, list[str]] = {}
    for line in table:
        cells = [c.strip() for c in line.strip("|").split("|")]
        out[row_id(cells[0])] = re.findall(r"`([^`]+)`", cells[1])
    return out


def changed(pr_files: list[list]) -> list[str]:
    """Every path a PR's files touch, a rename's old path included."""
    return [name for names in pr_files for name in names if name]


def matched(table: dict[str, list[str]], files: list[str]) -> list[str]:
    """The rows a changed path falls under; `*` crosses `/`."""
    return sorted(
        rid
        for rid, globs in table.items()
        if any(fnmatch.fnmatchcase(f, g) for g in globs for f in files)
    )


def owed(rows_matched: list[str], comments: list[dict], head: str) -> list[str]:
    """The matched rows with no maintainer's record at `head`."""
    recorded = set()
    for c in comments:
        m = MARKER.match(c["body"].lstrip())
        if m and c["association"] in MAINTAINERS and m.group(2) == head:
            recorded.add(m.group(1))
    return [r for r in rows_matched if r not in recorded]


def gh_lines(path: str, jq: str) -> list[dict]:
    out = subprocess.run(
        ["gh", "api", "--paginate", path, "--jq", jq],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    ).stdout
    return [json.loads(line) for line in out.splitlines() if line]


def main(repo: str, pr: str, head: str) -> int:
    table = rows((ROOT / "REVIEW.md").read_text())
    files = changed(gh_lines(f"repos/{repo}/pulls/{pr}/files", ".[] | [.filename, .previous_filename]"))
    comments = gh_lines(
        f"repos/{repo}/issues/{pr}/comments",
        ".[] | {association: .author_association, body: .body}",
    )
    due = matched(table, files)
    missing = owed(due, comments, head)
    for r in due:
        print(f"{r}: {'owed' if r in missing else 'recorded'}")
    if missing:
        print(
            f"::error::record each owed row at {head}: a comment opening "
            f"`<!-- focused-review:<row>:{head} -->`, then the review or "
            "`does not match: <why>`; then rerun this check"
        )
        return 1
    return 0


def selftest() -> None:
    assert row_id("CI, the merge gate or release tooling (a Dependabot `uses:` pin bump doesn't match)") == (
        "ci-the-merge-gate-or-release-tooling"
    )
    assert row_id("Motion / pose / walk-leg") == "motion-pose-walk-leg"
    assert row_id("A refactor: a `refactor` PR") == "a-refactor"

    table = rows((ROOT / "REVIEW.md").read_text())
    assert table, "the section holds no rows"
    tracked = subprocess.run(
        ["git", "ls-files"], cwd=ROOT, check=True, capture_output=True, text=True
    ).stdout.splitlines()
    for rid, globs in table.items():
        for g in globs:
            assert any(fnmatch.fnmatchcase(f, g) for f in tracked), f"{rid}: {g} matches no tracked file"
    assert matched(table, [".github/workflows/ci.yml"]) == ["ci-the-merge-gate-or-release-tooling"]
    assert matched(table, ["crates/pixtuoid-core/src/state/reducer/mod.rs"]) == ["reducer-liveness-sweeps"]
    assert matched(table, ["README.md"]) == []
    moved_out = changed([["crates/hook-shim/src/main.rs", "crates/pixtuoid-hook/src/main.rs"], ["README.md", None]])
    assert matched(table, moved_out) == ["the-shim"], "a rename out of a row's Paths matches"
    try:
        rows("# REVIEW\n")
        raise AssertionError("a REVIEW.md without the section must not parse")
    except ValueError:
        pass

    sha, old = "a" * 40, "b" * 40
    due = ["the-shim", "ci-the-merge-gate-or-release-tooling"]
    def rec(row: str, at: str, who: str = "OWNER", lead: str = "") -> dict:
        return {"association": who, "body": f"{lead}<!-- focused-review:{row}:{at} -->\nApproved."}

    assert owed(due, [rec("the-shim", sha), rec("ci-the-merge-gate-or-release-tooling", sha)], sha) == []
    assert owed(due, [rec("the-shim", old)], sha) == due, "a record judges its own head"
    assert owed(due, [rec("the-shim", sha, who="NONE")], sha) == due, "a maintainer's alone"
    assert owed(due, [rec("the-shim", sha, lead="see ")], sha) == due, "the comment opens with it"
    assert owed(due, [rec("the-shim", sha, lead="\n ")], sha) == due[1:]
    print("focused-reviews selftest: OK")


if __name__ == "__main__":
    if sys.argv[1:] == ["--selftest"]:
        selftest()
    elif len(sys.argv) == 4:
        sys.exit(main(*sys.argv[1:]))
    else:
        sys.exit(__doc__)
