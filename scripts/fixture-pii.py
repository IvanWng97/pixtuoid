#!/usr/bin/env python3
"""Re-scan the COMMITTED fixture tree for the recorder's own identity.

`capture_fixture`'s check runs ONCE, on the capturer's terminal, at capture
time. That is the wrong moment to be the only moment: it cannot see a fixture
added by hand, edited later, or captured before the check existed — and it has
already missed a class twice on this branch (an account email, then a whole MCP
server / skill roster in nine files). This runs in `lint` and CI, over what is
actually committed.

Names, not values: the capturer's `$HOME`/`$USER` are not knowable here, so this
keys on the FIELD and PREFIX markers that carry identity regardless of whose
machine produced them. `/Users/dev` is the declared redaction placeholder and is
expected everywhere.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "crates/pixtuoid-core/tests/sources"

# Kept in sync with `PII_MARKERS` in examples/capture_fixture.rs by
# `the_two_pii_marker_lists_agree`; the recorder refuses at capture time, this
# catches what is already on disk.
MARKERS = {
    "obsidian": "a personal skill/vault name",
    "api_key": "an api_key field",
    "Bearer ": "a bearer token",
    "account_id": "an account id",
}
# An MCP name is only identity when it names a REAL server. `mcp__example…` is
# the redaction placeholder this repo writes, so it must pass — the check has to
# stay silent on the legitimate variant or the first person to run it deletes it.
REAL_MCP = re.compile(r"mcp__(?!example)[A-Za-z0-9_-]+")
EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
# `noreply.github.com` is a BOT identity that the wire legitimately carries
# (copilot's commit author); `example.*`/localhost are the placeholders.
ALLOWED_EMAIL = re.compile(r"@(?:users\.)?noreply\.github\.com$|@example\.|@localhost$")
# Home-dir usernames that are conventions, not people. `dev` is the declared
# redaction placeholder; the rest predate it in hand-composed fixtures.
PLACEHOLDER_USERS = ("dev", "me", "user", "you", "test", "runner", "home")
FOREIGN_HOME = re.compile(
    r"/(?:Users|home)/(?!(?:" + "|".join(PLACEHOLDER_USERS) + r")\b)[A-Za-z0-9._-]+"
)


def main() -> int:
    bad: list[str] = []
    scanned = 0
    for f in sorted(FIXTURES.rglob("*")):
        if not f.is_file() or f.suffix not in (".jsonl", ".json", ".txt"):
            continue
        try:
            body = f.read_text(errors="replace")
        except OSError as e:
            bad.append(f"{f.relative_to(ROOT)}: unreadable ({e})")
            continue
        scanned += 1
        rel = f.relative_to(ROOT)
        for marker, what in MARKERS.items():
            if marker in body:
                bad.append(f"{rel}: {what} (`{marker}`)")
        if m := REAL_MCP.search(body):
            bad.append(f"{rel}: a real MCP server/tool name ({m.group(0)})")
        for m in EMAIL.finditer(body):
            if not ALLOWED_EMAIL.search(m.group(0)):
                bad.append(f"{rel}: an email address ({m.group(0)})")
                break
        for m in FOREIGN_HOME.finditer(body):
            bad.append(f"{rel}: a home path outside the `/Users/dev` placeholder ({m.group(0)})")
            break

    for line in bad:
        print(f"  {line}", file=sys.stderr)
    if bad:
        print(
            f"fixture PII: {len(bad)} problem(s) — redact, and say so in that "
            f"scenario's provenance `note`",
            file=sys.stderr,
        )
        return 1
    # A pass over an empty population says nothing; the corpus is 200+ files.
    if scanned < 100:
        print(f"fixture PII: only scanned {scanned} files — the walk found almost "
              f"nothing, so this pass says nothing about the corpus", file=sys.stderr)
        return 1
    print(f"fixture PII: clean ({scanned} files)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
