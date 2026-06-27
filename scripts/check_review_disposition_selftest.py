#!/usr/bin/env python3
"""Self-test for check_review_disposition.py — pins the pure parsers + the
assessor against the REAL claude[bot] inline-comment shapes (captured from PRs
#306/#316/#326), no network. A regex regression here is a silent disposition
hole (the harvester would find nothing → a false "no drops", the exact #283
failure mode it exists to catch)."""

import sys

import check_review_disposition as m


def run() -> int:
    failures: list[str] = []

    def check(name: str, cond: bool) -> None:
        if not cond:
            failures.append(name)

    # severity_of — every real wording variant seen in the wild.
    check("sev medium em-dash", m.severity_of("**MEDIUM — no subprocess timeout**") == "MEDIUM")
    check("sev high colon", m.severity_of("**HIGH: extra_artifacts write has no isolation**") == "HIGH")
    check("sev bracket medium", m.severity_of("**[MEDIUM] Subprocess output printed raw**") == "MEDIUM")
    check("sev low em-dash", m.severity_of("**LOW — unchecked i64 → i32 narrowing**") == "LOW")
    check("sev none", m.severity_of("just a plain comment, no severity") is None)
    # prose-form (no bold) severity at line start — the #384 robustness fix: a bot
    # finding that leads with `MEDIUM —` / `[HIGH]` without bold must still register.
    check("sev prose em-dash", m.severity_of("MEDIUM — no ledger row for this finding") == "MEDIUM")
    check("sev prose colon", m.severity_of("HIGH: installed-but-no-sprite on Windows") == "HIGH")
    check("sev prose bracket", m.severity_of("[MEDIUM] subprocess output printed raw") == "MEDIUM")
    check("sev bold mid-line still matches", m.severity_of("Context first. **MEDIUM — foo**") == "MEDIUM")
    # negatives — a hyphenated/mid-prose severity word must NOT register (no false
    # MEDIUM+ from "Low-hanging" / "HIGH-traffic" / mid-sentence "medium").
    check("sev no false low-hanging", m.severity_of("Low-hanging fruit, an easy win") is None)
    check("sev no false high-traffic", m.severity_of("This is a HIGH-traffic code path") is None)
    check("sev no false mid-prose medium", m.severity_of("a medium amount of risk here") is None)

    # extract_findings — bot author + MEDIUM+ filter (LOW and non-bot dropped).
    comments = [
        {"user": {"login": "claude[bot]"}, "path": "a.rs", "line": 10,
         "body": "**MEDIUM — blocking flock on the executor**\n…"},
        {"user": {"login": "claude[bot]"}, "path": "b.rs", "line": None,
         "body": "**HIGH: no isolation from ~/.openclaw**"},
        {"user": {"login": "claude[bot]"}, "path": "c.rs", "line": 3,
         "body": "**LOW — narrowing**"},  # LOW → not blocking
        {"user": {"login": "some-human"}, "path": "d.rs", "line": 1,
         "body": "**MEDIUM — human comment, not the bot**"},  # not bot
    ]
    findings = m.extract_findings(comments)
    check("extract count = 2 (MEDIUM+ bot only)", len(findings) == 2)
    check("extract paths", {f.path for f in findings} == {"a.rs", "b.rs"})
    check("extract keeps null line", any(f.line is None for f in findings))

    # _strip_control (#339): control chars in the bot body/path are stripped at the
    # construction boundary so the printed report can't emit a terminal escape.
    check("strip_control removes ESC/OSC/DEL/C1",
          m._strip_control("a\x1b[31mb\x07c\x7fd\x9ee") == "a[31mbcde")
    check("strip_control keeps printable + unicode", m._strip_control("café ▪ ✓") == "café ▪ ✓")
    hostile = [{"user": {"login": "claude[bot]"}, "path": "x\x1b[2J.rs", "line": 1,
                "body": "**MEDIUM — \x1b]0;pwned\x07evil\x1b[0m title**"}]
    hf = m.extract_findings(hostile)[0]
    check("finding.title has no control chars", all(ord(ch) >= 0x20 and ord(ch) != 0x7f for ch in hf.title))
    check("finding.path has no control chars", "\x1b" not in hf.path)

    # parse_marker_lines — the Bot-findings-adjudicated: block, ended by prose.
    body = (
        "fix(install): address #316 bot findings\n\n"
        "Some prose about the change.\n\n"
        "Bot-findings-adjudicated:\n"
        "- MEDIUM crates/pixtuoid/src/install/mod.rs → FIXED (block_in_place)\n"
        "- HIGH b.rs → ISSUE-FILED #332\n"
        "\n"
        "Co-Authored-By: someone\n"
    )
    markers = m.parse_marker_lines([body])
    check("marker count = 2", len(markers) == 2)
    check("marker stops at Co-Authored", all("Co-Authored" not in ln for ln in markers))
    check("marker has file ref", any("install/mod.rs" in ln for ln in markers))

    # assess — per-file coverage; the uncovered file is flagged.
    adjudicated, undecided = m.assess(findings, markers)
    # a.rs → covered by the install/mod.rs marker? No (different file). b.rs → yes.
    check("assess b.rs adjudicated", any(f.path == "b.rs" for f in adjudicated))
    check("assess a.rs un-adjudicated", any(f.path == "a.rs" for f in undecided))

    # The #283 class: MEDIUM+ findings, ZERO markers → all un-adjudicated.
    _, all_undecided = m.assess(findings, [])
    check("no marker → all un-adjudicated", len(all_undecided) == 2)

    # basename match: a marker naming just the basename covers a full-path finding.
    f_full = [m.Finding("MEDIUM", "crates/x/src/doctor.rs", None, "t")]
    cov, _ = m.assess(f_full, ["MEDIUM doctor.rs → FIXED"])
    check("basename marker covers full path", len(cov) == 1)

    # ledger_mentions_pr — word-boundary so #28 doesn't match #283.
    check("ledger matches #283", m.ledger_mentions_pr("see #283 for the fix", 283))
    check("ledger no false #28 in #283", not m.ledger_mentions_pr("only #283 here", 28))

    # judge_prompt — the advisory substance prompt embeds the findings + the marker
    # lines so the shared LLM-judge can rate substance vs rubber-stamp. Pure; pinned
    # so a refactor can't silently empty it (the same silent-monitor-death guard).
    jp = m.judge_prompt(findings, markers)
    check("judge_prompt names a finding path", "b.rs" in jp)
    check("judge_prompt embeds a disposition marker", "ISSUE-FILED #332" in jp)
    check("judge_prompt asks for substantive verdict", "substantive" in jp and "rubber_stamped" in jp)
    empty = m.judge_prompt([], [])
    check("judge_prompt handles empty (no findings/markers)",
          "(none)" in empty and "no disposition marker" in empty)

    if failures:
        print("check_review_disposition selftest FAILED:")
        for f in failures:
            print(f"  ✗ {f}")
        return 1
    print("check_review_disposition selftest: all assertions passed.")
    return 0


if __name__ == "__main__":
    sys.exit(run())
