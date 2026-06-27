#!/usr/bin/env python3
"""Shared helpers for the governance scripts (check_dod.py / check_review_disposition.py).

Currently only the pure, gh-free helper that BOTH scripts need verbatim. The `gh`
plumbing (DEFAULT_REPO / _gh_json) deliberately stays inlined per script until a
third gh-consuming gate makes the rule of three — see docs/governance-scripts.md.

Importable because every governance script runs as `python3 scripts/<name>.py`, so
the script's own dir (scripts/) is sys.path[0]; the *_selftest.py twins insert it
explicitly before importing the script under test.
"""


def _strip_control(s: str) -> str:
    """Drop C0/C1 control chars + DEL so untrusted PR/diff/commit/bot text can't
    emit a terminal escape (ANSI/OSC) when a governance report prints — the Python
    twin of the Rust sanitize-at-the-boundary rule (R0615-06 / verify::display_safe).
    Applied where strings are constructed, not at each print site."""
    return "".join(ch for ch in s if not (ord(ch) < 0x20 or 0x7F <= ord(ch) <= 0x9F))
