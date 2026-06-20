#!/usr/bin/env python3
"""Self-test for census_reminder.py — pins the filename parser + the due-threshold
so a regex / off-by-one regression can't silently mis-file (or never file) a census
reminder, the exact silent-monitor-death class the upstream-drift selftest guards.
Pure, no network. Run: `python3 scripts/census_reminder_selftest.py` (exit 0 = pass)."""

import sys

import census_reminder as m


def run() -> int:
    fails: list[str] = []

    def check(name: str, cond: bool) -> None:
        if not cond:
            fails.append(name)

    # last_window_end: the MAX range END across ranged reports; the un-ranged first
    # census (`mining-2026-06.md`) and the YYYY-MM date digits must NOT be mistaken
    # for a PR range (the off-by-a-date-segment trap).
    files = [
        "mining-2026-06.md",
        "mining-2026-06-262-328.md",
        "mining-2026-06-329-383.md",
    ]
    check("max range end across reports", m.last_window_end(files) == 383)
    check("un-ranged file alone -> None", m.last_window_end(["mining-2026-06.md"]) is None)
    check("date digits are not a range", m.last_window_end(["mining-2026-12.md"]) is None)
    check("empty list -> None", m.last_window_end([]) is None)
    check(
        "full paths parse",
        m.last_window_end(["docs/review-metrics/mining-2026-06-329-383.md"]) == 383,
    )
    check(
        "picks the newer of two ranges regardless of order",
        m.last_window_end(["mining-2026-07-384-433.md", "mining-2026-06-329-383.md"]) == 433,
    )

    # census_due: latest >= last_end + interval (inclusive at exactly the threshold).
    check("due at exactly the threshold", m.census_due(383, 433, 50) is True)
    check("due past the threshold", m.census_due(383, 500, 50) is True)
    check("not due one below the threshold", m.census_due(383, 432, 50) is False)
    check("None last_end is never due", m.census_due(None, 999, 50) is False)

    # issue_body: names the window + the no-placeholder note (so a human/agent knows
    # exactly what to run, and that the Action — not a placeholder — files it).
    body = m.issue_body(383, 440, 50)
    check("body states the window", "#384" in body and "#440" in body)
    check("body cites the workflow", "census-reminder.yml" in body)
    check("body points at the methodology doc", "KNOWLEDGE-ENGINEERING.md" in body)

    if fails:
        print("census_reminder selftest FAILED:")
        for f in fails:
            print(f"  ✗ {f}")
        return 1
    print("census_reminder selftest: all assertions passed.")
    return 0


if __name__ == "__main__":
    sys.exit(run())
