#!/usr/bin/env python3
"""Auto-file the next review-history census reminder when the merged-PR backlog
crosses the cadence threshold (~50 PRs past the last census's window).

The census cadence used to ride a HAND-FILED placeholder issue ("the cadence rides
the issue backlog, not memory"). This replaces the placeholder with a scheduled
check: parse the last census window-END from the newest
`docs/review-metrics/mining-*.md` filename, compare it to the highest merged PR,
and (when due) emit an issue body for the weekly workflow to open — deduped on the
`census` label, exactly like the upstream-drift watcher.

Pure + offline: the highest merged PR is passed in (`--latest-pr`), so the logic
is fully unit-testable; the workflow does the one `gh` call.

    census_reminder.py --latest-pr <N> [--reports-dir docs/review-metrics] [--interval 50]
    census_reminder.py --selftest

Exit: 1 = census DUE (issue body on stdout), 0 = not due, 2 = error.
"""

import argparse
import re
import sys
from pathlib import Path

CENSUS_INTERVAL = 50

# A census report filename carries the window as `mining-<YYYY>-<MM>-<start>-<end>.md`.
# Anchor on the YYYY-MM date so the date digits can't be mistaken for the PR range,
# and so the first census (`mining-2026-06.md`, no range) is correctly ignored.
_RANGE = re.compile(r"mining-\d{4}-\d{2}-(\d+)-(\d+)\.md$")


def last_window_end(filenames) -> "int | None":
    """The highest PR-range END across the census report filenames, or None when
    no ranged report exists (only the first, un-ranged census)."""
    ends = [int(m.group(2)) for f in filenames if (m := _RANGE.search(str(f)))]
    return max(ends) if ends else None


def census_due(last_end, latest_pr: int, interval: int = CENSUS_INTERVAL) -> bool:
    """A census is due once the highest merged PR reaches `last_end + interval`."""
    return last_end is not None and latest_pr >= last_end + interval


def issue_body(last_end: int, latest_pr: int, interval: int = CENSUS_INTERVAL) -> str:
    window_start = last_end + 1
    return (
        f"The merged-PR backlog crossed the census cadence threshold: the last census "
        f"window ended at **#{last_end}** and **{latest_pr - last_end}** PRs have merged "
        f"since (≥ the ~{interval}-PR interval).\n\n"
        f"**Run the next review-history census** — window ≈ **#{window_start}–#{latest_pr}**. "
        f"Same harvest → escapes → synthesis → adversarial-verify pipeline; carry forward "
        f"the items from the last report's *Re-measuring* section "
        f"(`docs/review-metrics/mining-*-{last_end}.md`). Methodology + the standing legs: "
        f"`docs/KNOWLEDGE-ENGINEERING.md`.\n\n"
        f"Auto-filed by `.github/workflows/census-reminder.yml` — the cadence rides this "
        f"scheduled check, not a hand-filed placeholder. Close this when the census runs.\n\n"
        f"\U0001f916 Generated with [Claude Code](https://claude.com/claude-code)\n"
    )


def main() -> int:
    ap = argparse.ArgumentParser(description="Review-history census cadence reminder.")
    ap.add_argument("--latest-pr", type=int, help="the highest merged PR number")
    ap.add_argument("--reports-dir", default="docs/review-metrics")
    ap.add_argument("--interval", type=int, default=CENSUS_INTERVAL)
    ap.add_argument("--selftest", action="store_true", help="run pure-fn tests, no network")
    a = ap.parse_args()
    if a.selftest:
        import census_reminder_selftest as st

        return st.run()
    if a.latest_pr is None:
        ap.error("pass --latest-pr <N> (or --selftest)")
    files = list(Path(a.reports_dir).glob("mining-*.md"))
    last_end = last_window_end(files)
    if last_end is None:
        print(
            "no census report with a PR range found in "
            f"{a.reports_dir} — nothing to compare",
            file=sys.stderr,
        )
        return 2
    threshold = last_end + a.interval
    if census_due(last_end, a.latest_pr, a.interval):
        print(issue_body(last_end, a.latest_pr, a.interval))
        print(
            f"census DUE: last_end={last_end} latest={a.latest_pr} threshold={threshold}",
            file=sys.stderr,
        )
        return 1
    print(
        f"not due: last_end={last_end} latest={a.latest_pr} threshold={threshold}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
