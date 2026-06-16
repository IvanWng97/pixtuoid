#!/usr/bin/env python3
"""Make the bot-finding disposition channel visible (issue #335).

The review-history census found a real failure mode (#283): a claude[bot]
MEDIUM+ inline finding reached merge with NO terminal state — no ledger row, no
issue, no fix. "Every finding was disposed" was *inferred from absence* because
the only way to check was a manual `gh` sweep. This turns that sweep into one
command.

For each PR it lists every claude[bot] MEDIUM+ inline finding and checks each
against the mechanical disposition channel: a `Bot-findings-adjudicated:` marker
block in the PR's commit messages / body (see CONTRIBUTING.md), one line per
finding naming its file + terminal state (FIXED / REFUTED / ISSUE-FILED #N /
ACCEPTED-residual). A finding whose file no marker line mentions is reported
UN-ADJUDICATED and the tool exits non-zero.

ADVISORY, not a hard CI gate: matching is per-file (coarse) and the bot
re-flags stale commits across rounds, so a blocking gate would mis-fire. Run it
during the merge disposition sweep, or on a merged PR, to catch the #283 class
(a MEDIUM+ finding with zero disposition trace).

    check_review_disposition.py <PR> [<PR> ...] [--repo OWNER/REPO]
    check_review_disposition.py --selftest   # pure-function tests, no network
"""

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

DEFAULT_REPO = "IvanWng97/pixtuoid"
BOT_LOGINS = {"claude[bot]", "github-actions[bot]"}
# MEDIUM and up — LOW findings don't require a tracked disposition.
BLOCKING = ("CRITICAL", "HIGH", "MEDIUM")
LEDGER = Path(__file__).resolve().parent.parent / "docs" / "REVIEW-LEDGER.md"

# A bot inline comment leads with its severity, wrapped in bold and written a
# few ways across rounds: `**MEDIUM — …`, `**HIGH: …`, `**[MEDIUM] …`,
# `**LOW — …`. Match the first such token.
_SEVERITY = re.compile(r"\*\*\s*\[?\s*(CRITICAL|HIGH|MEDIUM|LOW)\b", re.IGNORECASE)
_MARKER_HEADER = re.compile(r"^\s*bot-findings-adjudicated\s*:", re.IGNORECASE)


@dataclass(frozen=True)
class Finding:
    severity: str
    path: str
    line: object  # int or None — bot inline comments often carry a null line
    title: str


def _strip_control(s: str) -> str:
    """Drop C0/C1 control chars + DEL so untrusted bot-comment text / paths can't
    emit a terminal escape (ANSI/OSC) when this report is printed — the Python
    twin of the Rust side's `verify::display_safe` sanitize-at-the-boundary rule
    (R0615-06). Applied where findings are constructed, not at each print site."""
    return "".join(ch for ch in s if not (ord(ch) < 0x20 or 0x7F <= ord(ch) <= 0x9F))


def severity_of(body: str) -> str | None:
    """The MEDIUM+/LOW severity a bot comment body leads with, or None."""
    m = _SEVERITY.search(body or "")
    return m.group(1).upper() if m else None


def title_of(body: str) -> str:
    """A one-line title for display: the bold heading, control chars stripped."""
    first = (body or "").strip().splitlines()[0] if (body or "").strip() else ""
    return _strip_control(first.replace("*", "").strip())[:120]


def extract_findings(comments: list[dict]) -> list[Finding]:
    """The MEDIUM+ findings among a PR's inline review comments by the bot."""
    out: list[Finding] = []
    for c in comments:
        login = (c.get("user") or {}).get("login", "")
        if login not in BOT_LOGINS:
            continue
        sev = severity_of(c.get("body", ""))
        if sev is None or sev not in BLOCKING:
            continue
        out.append(
            Finding(
                severity=sev,
                path=_strip_control(c.get("path", "") or ""),
                line=c.get("line"),
                title=title_of(c.get("body", "")),
            )
        )
    return out


def parse_marker_lines(commit_texts: list[str]) -> list[str]:
    """The `- …` lines under every `Bot-findings-adjudicated:` block found in
    any of the supplied commit messages / PR body."""
    lines: list[str] = []
    for text in commit_texts:
        in_block = False
        for raw in (text or "").splitlines():
            if _MARKER_HEADER.match(raw):
                in_block = True
                continue
            if in_block:
                stripped = raw.strip()
                if stripped.startswith(("-", "*")):
                    lines.append(stripped.lstrip("-* ").strip())
                elif stripped == "":
                    continue  # blank lines are allowed inside the block
                else:
                    in_block = False  # any other prose ends the block
    return lines


def ledger_mentions_pr(ledger_text: str, pr: int) -> bool:
    """Whether the review ledger references this PR (a soft fallback hint)."""
    return bool(re.search(rf"#\b{pr}\b", ledger_text))


def assess(
    findings: list[Finding], marker_lines: list[str]
) -> tuple[list[Finding], list[Finding]]:
    """Split MEDIUM+ findings into (adjudicated, un-adjudicated). A finding is
    adjudicated when a marker line names its file (full path or basename) —
    per-file, deliberately coarse (one marker line can cover sibling findings)."""
    adjudicated: list[Finding] = []
    undecided: list[Finding] = []
    for f in findings:
        base = f.path.rsplit("/", 1)[-1]
        covered = any(
            f.path and (f.path in ln or (base and base in ln)) for ln in marker_lines
        )
        (adjudicated if covered else undecided).append(f)
    return adjudicated, undecided


# --------------------------------------------------------------------------- #
# Network glue (untested — the pure functions above carry the logic).         #
# --------------------------------------------------------------------------- #


def _gh_json(args: list[str]):
    out = subprocess.run(
        ["gh", *args], capture_output=True, text=True, check=True
    ).stdout
    return json.loads(out) if out.strip() else None


def _commit_texts(repo: str, pr: int) -> list[str]:
    """PR body + each commit's message + the on-main squash commit body — the
    marker may live in a review-round commit or the squash."""
    texts: list[str] = []
    view = _gh_json(
        ["pr", "view", str(pr), "--repo", repo, "--json", "body,commits"]
    )
    if view:
        texts.append(view.get("body") or "")
        for c in view.get("commits", []):
            texts.append((c.get("messageHeadline", "") + "\n" + c.get("messageBody", "")))
    squash = subprocess.run(
        ["git", "log", "--format=%B", "-1", f"--grep=(#{pr})"],
        capture_output=True,
        text=True,
    ).stdout
    if squash.strip():
        texts.append(squash)
    return texts


def check_pr(repo: str, pr: int, ledger_text: str) -> int:
    comments = _gh_json(
        ["api", f"repos/{repo}/pulls/{pr}/comments", "--paginate"]
    ) or []
    findings = extract_findings(comments)
    if not findings:
        print(f"#{pr}: no MEDIUM+ bot inline findings.")
        return 0
    marker_lines = parse_marker_lines(_commit_texts(repo, pr))
    adjudicated, undecided = assess(findings, marker_lines)
    print(f"#{pr}: {len(findings)} MEDIUM+ bot finding(s)")
    for f in adjudicated:
        print(f"  ✓ {f.severity} {f.path} — {f.title}")
    for f in undecided:
        print(f"  ✗ {f.severity} {f.path} — {f.title}  [UN-ADJUDICATED]")
    if undecided:
        if not marker_lines:
            print(
                f"  → no `Bot-findings-adjudicated:` marker in #{pr}'s commits."
                " Add one (CONTRIBUTING.md) or record the dispositions."
            )
        if ledger_mentions_pr(ledger_text, pr):
            print(
                f"  → the review ledger references #{pr} — verify those rows"
                " cover the findings above, then add the marker."
            )
    return 1 if undecided else 0


def main() -> int:
    ap = argparse.ArgumentParser(description="Audit bot-finding dispositions per PR.")
    ap.add_argument("prs", nargs="*", type=int, help="PR numbers to audit")
    ap.add_argument("--repo", default=DEFAULT_REPO)
    ap.add_argument("--selftest", action="store_true", help="run pure-fn tests")
    args = ap.parse_args()
    if args.selftest:
        import check_review_disposition_selftest as st

        return st.run()
    if not args.prs:
        ap.error("pass at least one PR number (or --selftest)")
    ledger_text = LEDGER.read_text() if LEDGER.exists() else ""
    worst = 0
    for pr in args.prs:
        worst |= check_pr(args.repo, pr, ledger_text)
    return worst


if __name__ == "__main__":
    sys.exit(main())
