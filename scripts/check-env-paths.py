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

TEST_ATTR = re.compile(r"^\s*#\[cfg\(test\)\]")
MOD_DECL = re.compile(r"^\s*(pub(\(\w+\))?\s+)?mod\s")


def offenders(root: Path) -> list[tuple[Path, int, str]]:
    hits: list[tuple[Path, int, str]] = []
    for rs in sorted(root.glob("crates/*/src/**/*.rs")):
        posix = rs.as_posix()
        if any(d in posix for d in SKIP_DIRS) or rs.name in SKIP_FILES:
            continue
        lines = rs.read_text(encoding="utf-8").splitlines()
        # Skip only the BODY of a `#[cfg(test)] mod`, tracked by brace depth. A
        # one-way "everything after the first #[cfg(test)]" flag looked simpler
        # and was catastrophically wrong: a mid-file `#[cfg(test)] const` (or the
        # string appearing inside a `//!` doc comment) blinded the rest of the
        # file — 44% of the scanned tree, including install/openclaw.rs, which is
        # a config-PATH resolver and exactly this gate's target class.
        depth = 0
        pending_attr = False
        for n, line in enumerate(lines, 1):
            stripped = line.lstrip()
            if depth:
                depth += line.count("{") - line.count("}")
                continue
            if TEST_ATTR.match(line):
                pending_attr = True
                continue
            if pending_attr:
                # Only a MODULE swallows a block; any other `#[cfg(test)]` item
                # is one declaration and the file keeps being scanned.
                if MOD_DECL.match(line) and "{" in line:
                    depth = line.count("{") - line.count("}")
                pending_attr = False
                continue
            if stripped.startswith("//"):
                continue
            m = PATH_NAME.search(line)
            if m:
                hits.append((rs.relative_to(root), n, m.group(1)))
    return hits


def walk_selftest() -> list[str]:
    """Negative-control the WALK, not just the regex.

    The regex half can be green while `offenders()` scans nothing — a one-way
    `#[cfg(test)]` flag once hid 44% of the tree behind a doc-comment mention.
    So plant a tree and assert on what the walker actually returns.
    """
    import tempfile

    fixture = {
        # a real violation, after a mid-file `#[cfg(test)] const` and a doc
        # comment that merely MENTIONS the attribute — both used to blind it
        "crates/a/src/lib.rs": (
            "//! see #[cfg(test)] below\n"
            "#[cfg(test)]\n"
            "const SENTINEL: u8 = 1;\n"
            "fn f() { let _ = std::env::var(\"CODEX_HOME\"); }\n"
        ),
        # inside a `#[cfg(test)] mod` — legitimately skipped
        "crates/b/src/lib.rs": (
            "#[cfg(test)]\nmod tests {\n"
            "    fn t() { let _ = std::env::var(\"HOME\"); }\n}\n"
        ),
        # skip-listed filename
        "crates/c/src/tests.rs": 'fn t() { std::env::var("HOME"); }\n',
    }
    fails: list[str] = []
    with tempfile.TemporaryDirectory() as td:
        root = Path(td)
        for rel, body in fixture.items():
            f = root / rel
            f.parent.mkdir(parents=True, exist_ok=True)
            f.write_text(body, encoding="utf-8")
        got = {(str(p), var) for p, _n, var in offenders(root)}
    if ("crates/a/src/lib.rs", "CODEX_HOME") not in got:
        fails.append("walk: missed a violation after a mid-file #[cfg(test)] item")
    if any(p == "crates/b/src/lib.rs" for p, _ in got):
        fails.append("walk: fired inside a #[cfg(test)] mod body")
    if any(p == "crates/c/src/tests.rs" for p, _ in got):
        fails.append("walk: fired in a skip-listed file")
    return fails


def selftest() -> int:
    """Prove the checker can FAIL — both halves: the regex AND the walk."""
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
    walk = walk_selftest()
    for f in walk:
        print(f"SELFTEST FAIL: {f}", file=sys.stderr)
    if bad or noisy or walk:
        return 1
    print(
        f"env-paths selftest: {len(fires)} fire + {len(silent)} silent regex cases, "
        "3 walk cases ✓"
    )
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
