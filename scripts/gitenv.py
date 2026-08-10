"""The one way `scripts/` spawns git, and the gate that keeps it the only way.

A git hook exports the repo-local `GIT_*` vars, and those OUTRANK `git -C <dir>`
and `cwd=` — so a tool that drives a throwaway repo silently drives the REAL one.
`gen-guides.py --selftest` emptied the pusher's index to one phantom
`crate/CLAUDE.md`; `comment-lint.py` went further and wrote two commits. Both
printed a clean pass while doing it. githooks(5) prescribes the remedy: code
invoking git "in a foreign repository … should clear these environment
variables". Only a linked worktree's `pre-push` exports them — from a plain
checkout it does not, which is why this survived a day of pushes unnoticed.

Scrubbing at each `main()` would leave one forgettable line per script, so the
scrub lives in `git()` and `bypass_sweep()` fails the build for any git argv in
`scripts/` that does not go through it.
"""

from __future__ import annotations

import functools
import os
import pathlib
import re
import subprocess
import sys
import tempfile
from typing import Any

SCRIPTS = pathlib.Path(__file__).resolve().parent
SELF = pathlib.Path(__file__).name

# Set on the child in `ambient_git_control` so the child's own control is a no-op.
INNER_ENV = "PIXTUOID_SELFTEST_INNER"

# A git argv list anywhere in `scripts/`. Matches the literal that starts it, so
# `[*git, "add"]` aliases are caught at the line that built `git`.
BARE_GIT = re.compile(r"""\[\s*["']git["']""")


@functools.cache
def clean_env() -> dict[str, str]:
    """The ambient environment minus every var that would relocate a git call."""
    # git owns this list and extends it (GIT_CONFIG_COUNT is a later addition); a
    # `GIT_*` prefix match would also drop GIT_SSH_COMMAND / GIT_COMMITTER_* / …
    names = set(
        subprocess.run(
            ["git", "rev-parse", "--local-env-vars"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.split()
    )
    return {k: v for k, v in os.environ.items() if k not in names}


def git(*args: str, **kwargs: Any) -> subprocess.CompletedProcess:
    """Run git located ONLY by its own arguments, never by an inherited `GIT_*`."""
    return subprocess.run(["git", *args], env=clean_env(), **kwargs)


def _at(repo: pathlib.Path) -> dict[str, str]:
    """Env aiming git at `repo` POSITIVELY, so no ambient var can redirect it."""
    # GIT_INDEX_FILE too: a pre-commit hook exports it, and pinning only GIT_DIR
    # would still stage through the ambient index.
    return {
        **os.environ,
        "GIT_DIR": str(repo / ".git"),
        "GIT_WORK_TREE": str(repo),
        "GIT_INDEX_FILE": str(repo / ".git" / "index"),
    }


def _harness_git(*args: str, env: dict[str, str], **kwargs: Any) -> subprocess.CompletedProcess:
    """Deliberately NOT `git()`: a control may not route through its own subject."""
    return subprocess.run(["git", *args], env=env, **kwargs)


def bypass_sweep(where: pathlib.Path = SCRIPTS) -> list[str]:
    """`path:line` for every git argv in `where` that dodges `git()` above."""
    return [
        f"{path.name}:{n}"
        for path in sorted(where.glob("*.py"))
        if path.name != SELF
        for n, line in enumerate(path.read_text().splitlines(), 1)
        if BARE_GIT.search(line)
    ]


def ambient_git_control(script: pathlib.Path) -> str | None:
    """Fail-message if `script --selftest` reaches the repo an ambient `GIT_DIR` names.

    It SPAWNS the entry point rather than calling the scrub in-process, because
    losing the scrub is the regression and an in-process control walks straight
    past it. The snapshots go through `_harness_git` aimed positively at `outer`:
    routing them through `git()` would let a broken `git()` redirect BOTH to the
    same wrong index, where they match and the control passes mid-corruption.
    """
    if os.environ.get(INNER_ENV):
        return None
    with tempfile.TemporaryDirectory() as tmp:
        outer = pathlib.Path(tmp) / "outer"
        (outer / "sub").mkdir(parents=True)
        (outer / "sub" / "kept.md").write_text("# kept\n")
        at_outer = _at(outer)
        _harness_git("init", "-q", env=at_outer, check=True, capture_output=True)
        _harness_git("add", "-A", env=at_outer, check=True, capture_output=True)

        def snap() -> str:
            return _harness_git(
                "ls-files", env=at_outer, capture_output=True, text=True, check=True
            ).stdout

        before = snap()
        subprocess.run(
            [sys.executable, str(script), "--selftest"],
            env={**clean_env(), "GIT_DIR": str(outer / ".git"), INNER_ENV: "1"},
            capture_output=True,
        )
        after = snap()
    if after != before:
        return (
            f"an inherited GIT_DIR must not let `{script.name} --selftest` reach the "
            f"outer index (it now holds {after.split() or '<empty>'})"
        )
    return None


def selftest() -> int:
    """Negative-control the scrub and the sweep — both must be able to FAIL."""
    fails: list[str] = []

    if offenders := bypass_sweep():
        fails.append(f"these bypass `gitenv.git()` and can be relocated by a hook: {offenders}")

    with tempfile.TemporaryDirectory() as tmp:
        planted = pathlib.Path(tmp) / "planted"
        planted.mkdir()
        (planted / "routed.py").write_text('gitenv.git("add", "-A")\n')
        if bypass_sweep(planted):
            fails.append("the bypass sweep must NOT fire on a routed call")
        (planted / "bare.py").write_text('subprocess.run(["git", "add", "-A"])\n')
        if bypass_sweep(planted) != ["bare.py:1"]:
            fails.append("the bypass sweep must FIRE on a bare git argv, naming it")

        victim = pathlib.Path(tmp) / "victim"
        (victim / "sub").mkdir(parents=True)
        (victim / "sub" / "kept.md").write_text("# kept\n")
        at_victim = _at(victim)
        _harness_git("init", "-q", env=at_victim, check=True, capture_output=True)
        _harness_git("add", "-A", env=at_victim, check=True, capture_output=True)

        def snap() -> str:
            return _harness_git(
                "ls-files", env=at_victim, capture_output=True, text=True, check=True
            ).stdout

        before = snap()

        leaky = pathlib.Path(tmp) / "leaky"
        leaky.mkdir()
        (leaky / "PHANTOM.md").write_text("# phantom\n")
        saved = os.environ.get("GIT_DIR")
        os.environ["GIT_DIR"] = str(victim / ".git")
        try:
            clean_env.cache_clear()
            git("-C", str(leaky), "init", "-q", check=True, capture_output=True)
            git("-C", str(leaky), "add", "-A", check=True, capture_output=True)
        finally:
            if saved is None:
                os.environ.pop("GIT_DIR", None)
            else:
                os.environ["GIT_DIR"] = saved
            clean_env.cache_clear()
        if snap() != before:
            fails.append("`git()` must not let an ambient GIT_DIR relocate a `-C` call")

    if fails:
        print("gitenv selftest FAILED:")
        for f in fails:
            print(f"  - {f}")
        return 1
    print("gitenv selftest: all 4 controls passed")
    return 0


def main() -> int:
    if "--selftest" in sys.argv[1:]:
        return selftest()
    print(__doc__)
    return 0


if __name__ == "__main__":
    sys.exit(main())
