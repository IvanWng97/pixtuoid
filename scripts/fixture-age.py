#!/usr/bin/env python3
"""Report which recorded fixtures have drifted from the CLI that produced them.

A recorded fixture pins ONE version of one CLI's wire. Nothing else in the tree
notices when that CLI moves: `check_upstream_drift.py` watches upstream SOURCE,
the conformance snapshots watch our decoders, and `captured` was prose with no
reader. codex switched its whole tool surface between 2026-07-25 and 2026-08-14
and the corpus said nothing — that switch was found by accident.

Two axes, in order of sharpness:

1. **Version drift** — `provenance.version` vs the CLI's version ON THIS HOST.
   The real signal, and the reason this is a LOCAL tool: CI has none of these
   CLIs installed, so there is nothing for it to compare against.
2. **Age** — a recorded fixture older than `--max-age-days`.

Advisory by design, exit 3 for "candidates found" (the `corpus-all` convention:
a stale fixture is a re-capture candidate, not a defect). `--check-metadata` is
the half that runs everywhere and DOES fail: it asserts the fields this report
reads are present and parseable, so the data the advisory depends on cannot rot
into uselessness.

    scripts/fixture-age.py                     # the report
    scripts/fixture-age.py --check-metadata    # the hard half (CI-safe)
"""

import argparse
import datetime as dt
import json
import pathlib
import re
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "crates/pixtuoid-core/tests/sources"
ROSTER = ROOT / "target/release/examples/corpus_check"


def version_probes() -> dict[str, list[str]]:
    """The registry's OWN probes, read through `corpus_check --roster` column 5.

    A hand-copied table here shipped already missing `agy`, silently — the exact
    "reuse an authority, never re-copy it" rule the root CLAUDE.md states. An
    unbuilt roster degrades to no version comparison rather than a wrong one.
    """
    if not ROSTER.is_file():
        return {}
    try:
        out = subprocess.run([str(ROSTER), "--roster"], capture_output=True, text=True,
                             timeout=30, stdin=subprocess.DEVNULL)
    except (OSError, subprocess.SubprocessError):
        return {}
    probes = {}
    for line in out.stdout.splitlines():
        cols = line.split("\t")
        if len(cols) >= 5 and cols[4] != "-":
            argv = cols[4].split()
            probes[argv[0]] = argv[1:]
    return probes



def provenances():
    for p in sorted(FIXTURES.rglob("provenance.json")):
        try:
            yield p, json.loads(p.read_text())
        except json.JSONDecodeError as e:
            yield p, {"_unparseable": str(e)}


def local_version(cli: str, probes: dict[str, list[str]]) -> str | None:
    if cli not in probes or not shutil.which(cli):
        return None
    try:
        out = subprocess.run([cli, *probes[cli]], capture_output=True, text=True,
                             timeout=20, stdin=subprocess.DEVNULL)
    except (OSError, subprocess.SubprocessError):
        return None
    return (out.stdout or out.stderr).strip().splitlines()[0] if (out.stdout or out.stderr) else None


def semverish(s: str) -> str | None:
    m = re.search(r"\d+\.\d+(?:\.\d+)?", s or "")
    return m.group(0) if m else None


def check_metadata() -> int:
    """The half that runs everywhere: the report's inputs must exist and parse."""
    bad = []
    for p, prov in provenances():
        rel = p.relative_to(ROOT)
        if "_unparseable" in prov:
            bad.append(f"{rel}: not valid JSON ({prov['_unparseable']})")
            continue
        if prov.get("origin") != "recorded":
            continue
        for field in ("cli", "version", "captured"):
            if not str(prov.get(field, "")).strip():
                bad.append(f"{rel}: recorded fixture has no `{field}`")
        captured = str(prov.get("captured", ""))
        if captured and captured != "unknown":
            try:
                dt.date.fromisoformat(captured)
            except ValueError:
                bad.append(f"{rel}: `captured` is not an ISO date: {captured!r}")
    for line in bad:
        print(f"  {line}", file=sys.stderr)
    if bad:
        print(f"fixture metadata: {len(bad)} problem(s)", file=sys.stderr)
        return 1
    print("fixture metadata: ok")
    return 0


def report(max_age_days: int) -> int:
    probes = version_probes()
    if not probes:
        print("  (corpus_check not built — version drift not checked; `just build --release --examples`)")
    today = dt.date.today()
    stale = []
    rows = []
    for p, prov in provenances():
        if prov.get("origin") != "recorded":
            continue
        rel = p.parent.relative_to(FIXTURES)
        cli, pinned = prov.get("cli", "?"), str(prov.get("version", "unknown"))
        captured = str(prov.get("captured", "unknown"))
        age = "?"
        if captured != "unknown":
            try:
                age = (today - dt.date.fromisoformat(captured)).days
            except ValueError:
                age = "?"
        live = local_version(cli, probes)
        drift = ""
        if live and pinned != "unknown":
            a, b = semverish(pinned), semverish(live)
            if a and b and a != b:
                drift = f"DRIFT {a} -> {b}"
                stale.append(str(rel))
        if isinstance(age, int) and age > max_age_days and str(rel) not in stale:
            drift = drift or f"AGE {age}d"
            stale.append(str(rel))
        rows.append((str(rel), cli, pinned, str(age), drift))

    w = max((len(r[0]) for r in rows), default=10)
    for rel, cli, pinned, age, drift in rows:
        print(f"  {rel:<{w}}  {cli:<12} {pinned:<24} {age:>4}d  {drift}")
    if stale:
        print(f"\n{len(stale)} fixture(s) worth re-capturing:", file=sys.stderr)
        for s in stale:
            print(f"  {s}", file=sys.stderr)
        print("\n  just capture-fixture <source> <scenario> <cmd…>   (BILLED)", file=sys.stderr)
        return 3
    print(f"\n{len(rows)} recorded fixture(s), none stale")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check-metadata", action="store_true",
                    help="assert the fields this report reads are present and parseable")
    ap.add_argument("--max-age-days", type=int, default=180)
    args = ap.parse_args()
    return check_metadata() if args.check_metadata else report(args.max_age_days)


if __name__ == "__main__":
    sys.exit(main())
