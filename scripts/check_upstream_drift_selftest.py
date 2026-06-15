#!/usr/bin/env python3
"""Self-test for check_upstream_drift.py — the drift watcher had ZERO tests, yet
a regex-parser regression = a SILENT monitor death (it returns empty / raises and
the weekly job either alarms on junk or watches nothing). This pins:

  1. `try_fetch` failure classification — the PR that added it fixed the
     `HTTPError ⊂ URLError ⊂ FETCH_ERRORS` swallow that bucketed a permanent 404
     as transient. A 404/410/451 MUST be breaking; 5xx/429/timeouts transient.
  2. The `read_*_events` source parsers still find a non-empty, well-shaped set
     (catches "the regex broke" / "it grabbed the wrong block").
  3. The `upstream_*` parsers extract names from a representative snippet.

Run: `python3 scripts/check_upstream_drift_selftest.py` (exit 0 = pass).
No pytest dependency on purpose — the repo has no Python test harness.
"""

from __future__ import annotations

import io
import pathlib
import re
import sys
import urllib.error

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import check_upstream_drift as d  # noqa: E402

FAILS: list[str] = []


def check(cond: bool, msg: str) -> None:
    if not cond:
        FAILS.append(msg)


def _http_error(code: int) -> urllib.error.HTTPError:
    return urllib.error.HTTPError("https://x/y", code, "msg", {}, io.BytesIO(b""))


def test_try_fetch_classifies_permanent_vs_transient() -> None:
    real = d.fetch
    try:
        # Permanent HTTP → breaking (the URL moved; watch blind).
        for code in (404, 410, 451):
            d.fetch = lambda _u, c=code: (_ for _ in ()).throw(_http_error(c))
            br: list[str] = []
            er: list[str] = []
            out = d.try_fetch("https://x/y", "T", br, er)
            check(out is None, f"{code}: returns None")
            check(len(br) == 1 and not er, f"{code}: -> breaking (got breaking={br} errors={er})")
            check(str(code) in br[0], f"{code}: message names the status")

        # Transient HTTP (server/throttle) → errors, NOT breaking.
        for code in (500, 502, 503, 429, 403):
            d.fetch = lambda _u, c=code: (_ for _ in ()).throw(_http_error(c))
            br, er = [], []
            d.try_fetch("https://x/y", "T", br, er)
            check(not br and len(er) == 1, f"{code}: -> transient (got breaking={br} errors={er})")

        # Network-layer failure → transient.
        d.fetch = lambda _u: (_ for _ in ()).throw(urllib.error.URLError("conn refused"))
        br, er = [], []
        d.try_fetch("https://x/y", "T", br, er)
        check(not br and len(er) == 1, f"URLError -> transient (got breaking={br} errors={er})")

        # Success → returns the body, no buckets touched.
        d.fetch = lambda _u: "BODY"
        br, er = [], []
        out = d.try_fetch("https://x/y", "T", br, er)
        check(out == "BODY" and not br and not er, "success returns body, no buckets")
    finally:
        d.fetch = real


def test_source_parsers_find_nonempty_well_shaped_sets() -> None:
    # (reader, a shape regex every member must match) — non-empty + shape catches
    # a broken regex / wrong-block grab WITHOUT coupling to the exact event set
    # (which legitimately grows as sources are added).
    cases = [
        (d.read_codex_events, r"^[A-Za-z]\w+$"),
        (d.read_cc_events, r"^[A-Za-z]\w+$"),
        (d.read_dispatch_names, r"^[A-Za-z]\w+$"),
        (d.read_reasonix_events, r"^[A-Za-z]\w+$"),
        (d.read_codewhale_events, r"^[a-z][a-z_]*$"),
        (d.read_opencode_events, r"^[a-z][a-z0-9._]*$"),
        (d.read_copilot_events, r"^[a-z][a-z0-9._]*$"),
        (d.read_cursor_events, r"^[a-zA-Z]\w+$"),
    ]
    for reader, shape in cases:
        name = reader.__name__
        got = reader()
        check(isinstance(got, set) and len(got) >= 2, f"{name}: non-empty (>=2), got {got!r}")
        bad = [m for m in got if not re.match(shape, m)]
        check(not bad, f"{name}: members match {shape}; offenders={bad}")


def test_upstream_parsers_extract_from_a_snippet() -> None:
    # Codex HookEventName enum snippet.
    codex = 'pub enum HookEventName {\n    SessionStart,\n    PreToolUse,\n    Stop,\n}'
    up = d.upstream_codex_hooks(codex)
    check(up is not None and {"SessionStart", "PreToolUse"} <= up, f"codex enum parse: {up}")

    # Copilot schema: definitions[*].properties.type.const.
    schema = (
        '{"definitions":{"A":{"properties":{"type":{"const":"session.start"}}},'
        '"B":{"properties":{"type":{"const":"tool.execution_start"}}}}}'
    )
    up = d.upstream_copilot_events(schema)
    check(up is not None and {"session.start", "tool.execution_start"} <= up, f"copilot schema parse: {up}")

    # A malformed schema → None (signals "restructured", handled as breaking upstream).
    check(d.upstream_copilot_events("not json") is None, "copilot bad json -> None")

    # CC hook-event summary table — the MOST complex parser (anchors to the
    # "| Event |" header + separator, extracts the backtick-quoted first cell).
    # A wrong-but-non-None match here would silently miss a renamed event, so pin
    # both a real table and the no-table -> None case.
    cc_md = (
        "| Event | When it fires |\n"
        "|---|---|\n"
        "| `PreToolUse` | before a tool call |\n"
        "| `PostToolUse` | after a tool call |\n"
    )
    up = d.upstream_cc_hook_events(cc_md)
    check(up is not None and {"PreToolUse", "PostToolUse"} <= up, f"cc table parse: {up}")
    check(d.upstream_cc_hook_events("no table here") is None, "cc no table -> None")

    # Reasonix Go consts: `Ident Event = "Wire"`.
    reasonix_go = 'const (\n\tPreToolUse Event = "PreToolUse"\n\tStop Event = "Stop"\n)'
    up = d.upstream_reasonix_hooks(reasonix_go)
    check(up is not None and {"PreToolUse", "Stop"} <= up, f"reasonix consts parse: {up}")
    check(d.upstream_reasonix_hooks("no consts here") is None, "reasonix none -> None")

    # CodeWhale Rust enum → snake_case wire names (serde rename_all = snake_case).
    codewhale_rs = "pub enum HookEvent {\n    SessionStart,\n    PreToolUse,\n}"
    up = d.upstream_codewhale_hooks(codewhale_rs)
    check(up is not None and {"session_start", "pre_tool_use"} <= up, f"codewhale enum parse: {up}")
    check(d.upstream_codewhale_hooks("no enum here") is None, "codewhale none -> None")


def main() -> int:
    for t in (
        test_try_fetch_classifies_permanent_vs_transient,
        test_source_parsers_find_nonempty_well_shaped_sets,
        test_upstream_parsers_extract_from_a_snippet,
    ):
        t()
    if FAILS:
        print("DRIFT SELFTEST FAILED:")
        for f in FAILS:
            print(f"  - {f}")
        return 1
    print("drift selftest: all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
