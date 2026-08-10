"""The one way `scripts/` spawns git, and the gate that keeps it the only way.

A git hook exports the repo-local `GIT_*` vars, and those OUTRANK `git -C <dir>`
and `cwd=` — so a tool that drives a throwaway repo silently drives the REAL one.
`gen-guides.py --selftest` emptied the pusher's index to one phantom
`crate/CLAUDE.md`, and it PERSISTED: `--check` then died on a path that was never
on disk, which reads as a broken guide index rather than a broken index file.
`comment-lint.py` went further and wrote two commits. Both printed a clean pass
while doing it. githooks(5) prescribes the remedy: code invoking git "in a
foreign repository … should clear these environment variables".

Exposure is uneven, which is why a day of pushes missed it: `GIT_DIR` is
worktree-specific (a plain checkout's `pre-push` exports none of the relocating
vars), while `pre-commit` exports `GIT_INDEX_FILE` from ANY checkout.

Scrubbing at each `main()` would leave one forgettable line per script, so the
scrub lives in `git()` and `bypass_sweep()` fails the build for any git argv in
`scripts/**/*.py` that does not go through it. Python only, by decision rather
than oversight: nothing in `scripts/*.sh` or `*.mjs` spawns git at all, and the
class that bit us twice was the Python tools that drive throwaway repos.

Only `comment-lint`'s selftest still builds one — its pathspec control has to
drive a real `git diff`. `gen-guides` injects `tracked_guides` instead, so its
fixture is a plain directory that cannot reach an index at all. Removing the
precondition is why the controls below are this small: with `git()` the only
path, there is no per-`main()` call left to forget.
"""

from __future__ import annotations

import ast
import functools
import os
import pathlib
import subprocess
import sys
import tempfile
from typing import Any

SCRIPTS = pathlib.Path(__file__).resolve().parent
SELF = pathlib.Path(__file__).resolve()

# Callables that spawn a process, for the `shell=True` / `os.system` string form.
SPAWNERS = frozenset(
    {"run", "Popen", "call", "check_call", "check_output", "system", "getoutput"}
)


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
    """Env aiming git at `repo` POSITIVELY — every relocating var pinned or dropped."""
    # The blanket `GIT_*` drop is right HERE (unlike `clean_env`): this env only
    # ever reaches throwaway fixtures, so losing GIT_COMMITTER_* costs nothing.
    return {
        **{k: v for k, v in os.environ.items() if not k.startswith("GIT_")},
        "GIT_DIR": str(repo / ".git"),
        "GIT_WORK_TREE": str(repo),
        "GIT_INDEX_FILE": str(repo / ".git" / "index"),
    }


def _hook_env(repo: pathlib.Path) -> dict[str, str]:
    """What a REAL hook exports, aimed at `repo` — the child's simulated ambient env.

    Deliberately no `GIT_WORK_TREE`: no hook exports it, and pinning it here would
    point `add -A` back at `repo`'s own tree, so a genuinely leaking child would
    stage nothing new and the control would pass. Measured — with GIT_WORK_TREE the
    outer index stayed `['sub/kept.md']`; without it, it became `['PHANTOM.md']`.
    """
    return {
        **{k: v for k, v in os.environ.items() if not k.startswith("GIT_")},
        "GIT_DIR": str(repo / ".git"),
        "GIT_INDEX_FILE": str(repo / ".git" / "index"),
    }


def _harness_git(*args: str, env: dict[str, str], **kwargs: Any) -> subprocess.CompletedProcess:
    """Deliberately NOT `git()`: a control may not route through its own subject."""
    return subprocess.run(["git", *args], env=env, **kwargs)


def _git_argv_lines(source: str) -> list[int]:
    """Lines holding a git argv literal or a shelled-out git string."""
    # An AST walk, not a regex: comments and docstrings quoting the anti-pattern
    # are not nodes (a gate that fires on prose misdiagnoses), and a black/ruff
    # exploded `[\n "git",` argv is one node however it wraps.
    hits: set[int] = set()
    for node in ast.walk(ast.parse(source)):
        if isinstance(node, (ast.List, ast.Tuple)) and node.elts:
            head = node.elts[0]
            if isinstance(head, ast.Constant) and isinstance(head.value, str):
                if pathlib.PurePath(head.value).name == "git":
                    hits.add(node.lineno)
        elif isinstance(node, ast.Call) and node.args:
            name = getattr(node.func, "attr", None) or getattr(node.func, "id", None)
            head = node.args[0]
            if name in SPAWNERS and isinstance(head, ast.Constant) and isinstance(head.value, str):
                word = head.value.split()[:1]
                if word and pathlib.PurePath(word[0]).name == "git":
                    hits.add(node.lineno)
    return sorted(hits)


def bypass_sweep(where: pathlib.Path = SCRIPTS) -> list[str]:
    """`name:line` for every git argv under `where`'s `*.py` that dodges `git()`."""
    out: list[str] = []
    for path in sorted(where.rglob("*.py")):
        if path.resolve() == SELF or "__pycache__" in path.parts:
            continue
        try:
            lines = _git_argv_lines(path.read_text())
        except SyntaxError as e:  # unparseable == unswept; never pass silently
            out.append(f"{path.name}:{e.lineno} (unparseable, so unswept: {e.msg})")
            continue
        out += [f"{path.name}:{n}" for n in lines]
    return out


def selftest() -> int:
    """Negative-control the scrub, the sweep, and the control — each must FAIL on cue."""
    fails: list[str] = []

    if offenders := bypass_sweep():
        fails.append(f"these bypass `gitenv.git()` and can be relocated by a hook: {offenders}")

    with tempfile.TemporaryDirectory() as tmp:
        planted = pathlib.Path(tmp) / "planted"
        planted.mkdir()
        (planted / "routed.py").write_text('gitenv.git("add", "-A")\n')
        # Prose quoting the anti-pattern must not red the build.
        (planted / "prose.py").write_text('# never write subprocess.run(["git", "add"])\n')
        if bypass_sweep(planted):
            fails.append("the sweep must NOT fire on a routed call or on prose")
        for name, body in (
            ("bare.py", 'subprocess.run(["git", "add", "-A"])\n'),
            ("tup.py", 'subprocess.run(("git", "add", "-A"))\n'),
            ("abs.py", 'subprocess.run(["/usr/bin/git", "add", "-A"])\n'),
            ("shell.py", 'subprocess.run("git add -A", shell=True)\n'),
            ("wrapped.py", 'subprocess.run(\n    [\n        "git",\n        "add",\n    ]\n)\n'),
            ("alias.py", 'GIT = ["git", "-C", "x"]\nsubprocess.run([*GIT, "add"])\n'),
        ):
            (planted / name).write_text(body)
            # Not `:1` — an exploded argv's node starts on the `[` line, not the call's.
            if not any(o.startswith(f"{name}:") for o in bypass_sweep(planted)):
                fails.append(f"the sweep must FIRE on {name}'s git argv, naming it")
            (planted / name).unlink()
        (planted / "broken.py").write_text("def f(:\n")
        if not any("unparseable" in o for o in bypass_sweep(planted)):
            fails.append("an unparseable file must be REPORTED, not silently unswept")
        (planted / "broken.py").unlink()

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

        # Assert the CONTENT, not just before==after: if the harness ever reads the
        # wrong repo — e.g. someone folds `_harness_git` into `git()` and loses the
        # aim — a self-comparison matches trivially and the control goes vacuous.
        if (before := snap()) != "sub/kept.md\n":
            fails.append(f"the control's own fixture must be the repo it reads (got {before!r})")

        leaky = pathlib.Path(tmp) / "leaky"
        leaky.mkdir()
        (leaky / "PHANTOM.md").write_text("# phantom\n")
        # Both vars a hook exports, not just GIT_DIR: with only GIT_DIR simulated, an
        # ambient GIT_INDEX_FILE redirects the damage past this control's own victim.
        ambient = _hook_env(victim)
        saved = {k: os.environ.get(k) for k in ("GIT_DIR", "GIT_INDEX_FILE")}
        try:
            os.environ.update({k: ambient[k] for k in saved})
            clean_env.cache_clear()
            git("-C", str(leaky), "init", "-q", check=True, capture_output=True)
            git("-C", str(leaky), "add", "-A", check=True, capture_output=True)
        finally:
            for k, v in saved.items():
                if v is None:
                    os.environ.pop(k, None)
                else:
                    os.environ[k] = v
            clean_env.cache_clear()
        if snap() != before:
            fails.append("`git()` must not let an ambient GIT_DIR relocate a `-C` call")

    if fails:
        print("gitenv selftest FAILED:")
        for f in fails:
            print(f"  - {f}")
        return 1
    print("gitenv selftest: every control passed")
    return 0


def main() -> int:
    if "--selftest" in sys.argv[1:]:
        return selftest()
    print(__doc__)
    return 0


if __name__ == "__main__":
    sys.exit(main())
