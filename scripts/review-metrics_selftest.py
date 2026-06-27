#!/usr/bin/env python3
"""Self-test for review-metrics.py — pins the keyword role classifier so a
keyword/ordering regression can't silently mis-bucket an agent's role. The
metrics feed REVIEW-LEDGER.md's before/after evidence, so a silent mis-classify
corrupts the economics record — the same silent-monitor-death class the other
governance selftests guard. Pure, no network. review-metrics.py was the only
governance script without a selftest; this closes that gap.

review-metrics.py has a hyphen in its name (not an importable identifier), so it
is loaded by path rather than `import`. Run:
`python3 scripts/review-metrics_selftest.py` (exit 0 = pass)."""

import importlib.util
import sys
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "review_metrics", Path(__file__).with_name("review-metrics.py")
)
m = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(m)


def run() -> int:
    fails: list[str] = []

    def check(name: str, cond: bool) -> None:
        if not cond:
            fails.append(name)

    # Each role: a prompt carrying ONLY that role's keywords classifies to it.
    check("verifier", m.classify("adversarially verify the claim") == "verifier")
    check("dedup", m.classify("deduplicate the distinct findings") == "dedup")
    check("finder", m.classify("find bugs in the auth subsystem") == "finder")
    check("implementer", m.classify("implement the new source") == "implementer")

    # The "build " implementer keyword carries a LOAD-BEARING trailing space — it
    # must match "build" as a word, NOT "building"/"rebuilding" (which stay "other").
    # Pin it so a maintainer "normalizing" the keyword to "build" (which would
    # silently re-bucket "building…" as implementer) trips this test.
    check("'building' stays other (trailing-space guard)", m.classify("building the feature") == "other")

    # Unknown / empty -> the explicit "other" sentinel, never a crash.
    check("unknown -> other", m.classify("summarize the meeting notes") == "other")
    check("empty -> other", m.classify("") == "other")

    # classify() lowercases first, so matching is case-insensitive.
    check("case-insensitive", m.classify("VERIFY THIS") == "verifier")

    # FIRST-MATCH-WINS by ROLE_KEYWORDS order — the documented ambiguity. A prompt
    # matching multiple roles returns the FIRST role in the list, not a "best" fit.
    # Pin the order so a reorder of ROLE_KEYWORDS is caught, not silently swapped.
    check(
        "verifier outranks dedup (list order)",
        m.classify("verify and then dedup the findings") == "verifier",
    )
    check(
        "dedup outranks finder + implementer (list order)",
        m.classify("implement a finder for the dedup stage") == "dedup",
    )
    # finder outranks implementer: a prompt carrying BOTH an implementer keyword
    # ("implement") and a finder keyword ("find bugs") must return the FIRST role
    # in the list — finder. This is the ONLY assertion that co-presents the two,
    # so it's what actually gives the "reorder is caught" claim teeth: a
    # finder<->implementer swap of ROLE_KEYWORDS flips this to "implementer".
    check(
        "finder outranks implementer (list order)",
        m.classify("implement a fix and find bugs") == "finder",
    )

    if fails:
        print("review-metrics selftest FAILED:")
        for f in fails:
            print(f"  ✗ {f}")
        return 1
    print("review-metrics selftest: all assertions passed.")
    return 0


if __name__ == "__main__":
    sys.exit(run())
