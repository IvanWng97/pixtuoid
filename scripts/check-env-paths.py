#!/usr/bin/env python3
"""Fail when a PATH-valued env var is read through `env::var`.

`env::var` returns `Err(NotUnicode)` for a value that is not UTF-8, and a
filesystem path is not required to be on any Unix — so reading a path override
with it DROPS a legal value and sends the resolver to a fallback location. The
user's override is ignored and the office comes up empty: the #880/#343/#342/#195
failure shape, reached through the ENCODING rather than the precedence.

`env::var_os` is the std authority, wrapped as `platform::path_env` (and the
shim's deliberate twin in `pixtuoid-hook/src/paths.rs`, which cannot depend on
core). This gate exists because the type fix alone does not stop a NEW call site
from reintroducing the class — the same reason `home_env` is a required registry
field rather than a checklist bullet.

Self-test: `--selftest` proves the checker FIRES on a planted violation and stays
silent on the legitimate forms, so a green run is evidence rather than silence.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

# A name is PATH-valued when it names a directory/file location. Matched on the
# NAME, not the use site: a value called `*_HOME`/`*_DIR`/`XDG_*` is a path
# whatever the surrounding code does with it.
PATH_NAME = re.compile(
    r"""env::var\(\s*"(
        HOME | USERPROFILE | APPDATA | LOCALAPPDATA | HOMEDRIVE | HOMEPATH
      | XDG_[A-Z_]+
      | [A-Z0-9_]*_(?:HOME|DIR|PATH|ROOT)
      | PIXTUOID_SOCKET
    )"\s*\)""",
    re.VERBOSE,
)

# Test code may read whatever it likes: it SETS these vars to known-good UTF-8
# and restores them, and `var` is the natural way to save/restore.
SKIP_DIRS = ("/tests/", "/target/", "/.claude/worktrees/")
SKIP_FILES = ("tests.rs", "build.rs")


def offenders(root: Path) -> list[tuple[Path, int, str]]:
    hits: list[tuple[Path, int, str]] = []
    for rs in sorted(root.glob("crates/*/src/**/*.rs")):
        posix = rs.as_posix()
        if any(d in posix for d in SKIP_DIRS) or rs.name in SKIP_FILES:
            continue
        in_tests = False
        for n, line in enumerate(rs.read_text(encoding="utf-8").splitlines(), 1):
            # Everything after `#[cfg(test)]` in a file is test scaffolding; the
            # modules are always last, so a one-way flag is enough.
            if "#[cfg(test)]" in line:
                in_tests = True
            if in_tests or line.lstrip().startswith("//"):
                continue
            m = PATH_NAME.search(line)
            if m:
                hits.append((rs.relative_to(root), n, m.group(1)))
    return hits


def selftest() -> int:
    """Prove the checker can FAIL, and that it stays quiet on the right forms."""
    fires = [
        'let h = std::env::var("HOME").ok();',
        'if let Ok(d) = std::env::var("XDG_RUNTIME_DIR") {',
        'std::env::var("CODEX_HOME").ok()',
        'std::env::var("PIXTUOID_SOCKET")',
        'std::env::var("SOME_CLI_DIR")',
    ]
    silent = [
        'std::env::var_os("HOME")',
        'crate::platform::path_env("HOME")',
        'std::env::var("PIXTUOID_SOURCE").ok()',  # a NAME, not a path
        'std::env::var("RUST_LOG").ok()',
        'std::env::var("OMP_PROFILE")',  # a profile name, not a path
    ]
    bad = [s for s in fires if not PATH_NAME.search(s)]
    noisy = [s for s in silent if PATH_NAME.search(s)]
    for s in bad:
        print(f"SELFTEST FAIL: should have fired on: {s}", file=sys.stderr)
    for s in noisy:
        print(f"SELFTEST FAIL: should have stayed silent on: {s}", file=sys.stderr)
    if bad or noisy:
        return 1
    print(f"env-paths selftest: {len(fires)} fire + {len(silent)} silent cases ✓")
    return 0


def main() -> int:
    if "--selftest" in sys.argv:
        return selftest()
    root = Path(__file__).resolve().parent.parent
    hits = offenders(root)
    if not hits:
        print("env-paths: no PATH-valued env::var reads ✓")
        return 0
    print(
        "env-paths: a PATH-valued env var is read with `env::var`, which DROPS a\n"
        "value that is not UTF-8 — a legal path — and silently falls back to a\n"
        "different directory. Read it as bytes instead:\n"
        "  pixtuoid-core / pixtuoid : crate::platform::path_env(NAME)\n"
        "  pixtuoid-hook            : the local path_env in src/paths.rs\n",
        file=sys.stderr,
    )
    for path, line, var in hits:
        print(f"  {path}:{line}: env::var(\"{var}\")", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
