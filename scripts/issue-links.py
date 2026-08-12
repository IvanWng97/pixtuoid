#!/usr/bin/env python3
"""A GitHub closing keyword may only appear as a trailer, never inside prose.

GitHub closes an issue when a merged commit message or pull-request description
contains `close|closes|closed|fix|fixes|fixed|resolve|resolves|resolved`
followed by `#N` — optionally with a colon, in any case, ANYWHERE in the text
(the docs place no line-position requirement). It reads the keyword, not the
sentence, so two issues here were closed by prose meaning the opposite: a
heading `Why this does NOT close #907`, and `(the alternative that also fixes
#912)` naming an option that was measured and rejected.

The rule is therefore positional, which is what makes it mechanical: an
INTENTIONAL link is a trailer line of its own (`Closes: #912`); a keyword
touching `#N` anywhere else is prose that will fire by accident.

Reads commit messages only. A pull-request DESCRIPTION is authored on GitHub and
never passes through here — that half of the surface is unguarded.

Usage: issue-links.py [RANGE | --stdin | --selftest]
  RANGE      git rev range whose commit messages to check (default: origin/main..HEAD)
  --stdin    check one message on stdin (the commit-msg hook's path)
  --selftest pin both directions on the real incidents
"""

from __future__ import annotations

import re
import sys

import gitenv

KEYWORDS = "close[sd]?|fix(?:e[sd])?|resolve[sd]?"
# A keyword touching `#N`, with the word boundary spelled out so `postfix #912`
# and `prefix #12` are prose, not links.
INLINE = re.compile(rf"(?:^|[^\w-])({KEYWORDS})[\s:]+#(\d+)", re.IGNORECASE)
# The whole line IS the link — the trailer form, and the only accepted one.
TRAILER = re.compile(rf"^\s*(?:{KEYWORDS}):?\s+#\d+\s*$", re.IGNORECASE)


def offenders(message: str) -> list[tuple[int, str]]:
    """Lines carrying a closing keyword that is not the line's whole content."""
    out = []
    for n, line in enumerate(message.splitlines(), 1):
        # `#`-led lines are git comments, stripped before the message is stored.
        if line.startswith("#") or TRAILER.match(line) or not INLINE.search(line):
            continue
        out.append((n, line.strip()))
    return out


def report(label: str, message: str) -> int:
    bad = offenders(message)
    for n, line in bad:
        print(f"  {label}:{n}: {line}")
    return len(bad)


def check_range(rev_range: str) -> int:
    r = gitenv.git(
        "log", "--format=%H%x00%B%x00%x00", rev_range, capture_output=True, text=True, check=False
    )
    if r.returncode != 0:
        print(f"issue-links: cannot read {rev_range}", file=sys.stderr)
        return 1
    total = 0
    for entry in r.stdout.split("\0\0"):
        if "\0" not in entry:
            continue
        sha, body = entry.split("\0", 1)
        total += report(sha.strip()[:8], body)
    if total:
        print(
            f"\nissue-links: {total} inline closing keyword(s). GitHub closes the issue on\n"
            "merge regardless of the sentence around it — move it to its own trailer line\n"
            "(`Closes: #N`), or reword so the keyword does not touch the number.",
            file=sys.stderr,
        )
        return 1
    print(f"issue-links: no inline closing keywords in {rev_range} ✓")
    return 0


def selftest() -> int:
    fails: list[str] = []
    # The two REAL incidents, verbatim, plus the shapes around them.
    cases = [
        (True, "the #912 incident", "(the alternative that also fixes #912) fails 132 tests."),
        (True, "the #907 incident", "Why this does NOT close #907 — the residual is the painter's."),
        (True, "mid-sentence past tense", "we Fixed #916 while we were in there"),
        (True, "mid-sentence with colon", "this Resolves: #900 only partially"),
        (False, "trailer with colon", "Closes: #912"),
        (False, "trailer without colon", "Closes #912"),
        (False, "indented trailer", "   closes #912   "),
        (False, "a non-keyword reference", "Refs #912, which stays open."),
        (False, "a word merely ending in fix", "the postfix #912 notation"),
        (False, "a hyphenated word", "the auto-fix #912 pass"),
        (False, "no reference at all", "nothing to see here"),
        (False, "a git comment line", "# Why this does NOT close #907"),
    ]
    for want, why, line in cases:
        got = bool(offenders(line))
        if got is not want:
            fails.append(f"{why}: expected {'a hit' if want else 'no hit'} — {line!r}")
    # A keyword and a number on one line, separated by other words, is prose to
    # GitHub too — the guard must not over-reach into things it cannot fire on.
    if offenders("closes the door on #912"):
        fails.append("a keyword NOT touching the number must not be reported")
    if fails:
        print("issue-links selftest FAILED:")
        for f in fails:
            print(f"  - {f}")
        return 1
    print("issue-links selftest: all checks passed")
    return 0


def main(argv: list[str]) -> int:
    args = argv[1:]
    if "--selftest" in args:
        return selftest()
    if "--stdin" in args:
        n = report("message", sys.stdin.read())
        if n:
            print(
                "\nissue-links: move it to its own trailer line (`Closes: #N`), or reword\n"
                "so the keyword does not touch the number.",
                file=sys.stderr,
            )
        return 1 if n else 0
    return check_range(args[0] if args else "origin/main..HEAD")


if __name__ == "__main__":
    sys.exit(main(sys.argv))
