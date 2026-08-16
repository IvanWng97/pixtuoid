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


def recorded_provenances():
    """Every `recorded` provenance, for the REPORT.

    A plain walk, deliberately: this half is a local advisory that reads four
    fields and never attributes a capture to a source, so it needs none of the
    layout rules the GATES ride. Those live in ONE place — `tests/sources/
    captures.rs` — and a mirror here would be a second thing to keep in step for
    no gate's benefit.
    """
    for path in sorted(FIXTURES.rglob("provenance.json")):
        try:
            doc = json.loads(path.read_text())
        except (OSError, json.JSONDecodeError):
            continue
        if isinstance(doc, dict) and doc.get("origin") == "recorded":
            yield path.parent, doc

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
    # A roster that RAN and FAILED used to read as "not built", so the report
    # printed a false diagnosis of why it was comparing nothing.
    if out.returncode != 0:
        print(f"corpus_check --roster exited {out.returncode}", file=sys.stderr)
        raise SystemExit(2)
    probes = {}
    for line in out.stdout.splitlines():
        cols = line.split("\t")
        if len(cols) >= 5 and cols[4] != "-":
            argv = cols[4].split()
            probes[argv[0]] = argv[1:]
    return probes

def local_version(cli: str, probes: dict[str, list[str]]) -> str | None:
    if cli not in probes or not shutil.which(cli):
        return None
    try:
        out = subprocess.run([cli, *probes[cli]], capture_output=True, text=True,
                             timeout=20, stdin=subprocess.DEVNULL)
    except (OSError, subprocess.SubprocessError):
        return None
    return (out.stdout or out.stderr).strip().splitlines()[0] if (out.stdout or out.stderr) else None

# A dotted-run major at or above this looks like a YEAR/date token, not a semver
# major. MIRRORS `doctor::parse_version`'s const of the same name — the pair is
# pinned by this file's `selftest()`, which `--check-metadata` runs, because the two
# compare the SAME `--version` banners across a language boundary.
IMPLAUSIBLE_MAJOR = 1000

def semverish(s: str) -> str | None:
    """The version in a `--version` banner, by `doctor::parse_version`'s rule.

    The naive first-dotted-run this replaced is the form that parser documents
    as wrong: a banner may print a build DATE before the semver.
    """
    runs = []
    for m in re.finditer(r"\d[\d.]*", s or ""):
        run = m.group(0)
        if "." not in run:
            continue
        major = int(run.split(".")[0])
        v_prefixed = m.start() > 0 and s[m.start() - 1] in "vV"
        runs.append((v_prefixed, major, run.rstrip(".")))
    if not runs:
        return None
    for vp, _, run in runs:
        if vp:
            return run
    for _, major, run in runs:
        if major < IMPLAUSIBLE_MAJOR:
            return run
    return runs[0][2]

def report(max_age_days: int) -> int:
    probes = version_probes()
    if not probes:
        print("  (corpus_check not built — version drift not checked; `just build --release --examples`)")
    # UTC, because `captured` is: the recorder stamps `today()` off UTC and the
    # payloads' own `_shim_ts_ms` are UTC too. A local `date.today()` reported a
    # fixture recorded hours ago as -1d.
    today = dt.datetime.now(dt.UTC).date()
    stale = []
    unchecked: list[tuple[str, str]] = []
    rows = []
    for capture_dir, doc in recorded_provenances():
        rel = capture_dir.relative_to(FIXTURES)
        cli, pinned = doc.get("cli") or "?", doc.get("version") or "unknown"
        captured = doc.get("captured") or "unknown"
        age = "?"
        if captured != "unknown":
            try:
                age = (today - dt.date.fromisoformat(captured)).days
            except ValueError:
                age = "?"
        live = local_version(cli, probes)
        drift = ""
        # A fixture we could not compare is neither fresh nor stale, and folding
        # it into "none stale" is the false green `corpus-all` already refuses
        # with its NOT COVERED lane.
        if pinned == "unknown":
            unchecked.append((str(rel), "no version pinned"))
        elif live is None:
            unchecked.append((str(rel), f"{cli} not installed here"))
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
    if unchecked:
        print(f"\n{len(unchecked)} not comparable:", file=sys.stderr)
        for rel, why in unchecked:
            print(f"  {rel}  ({why})", file=sys.stderr)
    if stale:
        print(f"\n{len(stale)} fixture(s) worth re-capturing:", file=sys.stderr)
        for s in stale:
            print(f"  {s}", file=sys.stderr)
        print("\n  just capture-fixture <source> <scenario> <cmd…>   (BILLED)", file=sys.stderr)
        return 3
    print(
        f"\n{len(rows)} recorded, {len(stale)} stale, {len(unchecked)} not comparable"
    )
    return 0

def selftest() -> int:
    """The pure logic this script's two gates ride on, with the parser's own
    documented cases. Runs inside `--check-metadata`, so it cannot rot unrun."""
    bad = []
    # The FIRST case is the one the naive `\d+\.\d+` form gets wrong, and is
    # verbatim the banner `doctor::parse_version`'s doc names.
    for banner, want in [
        ("Built 2026.06.04 — v1.2.3", "1.2.3"),
        ("2026.06.04", "2026.06.04"),
        ("codex-cli 0.147.0", "0.147.0"),
        ("Hermes Agent v0.20.1 (2026.8.13)", "0.20.1"),
        ("grok 0.2.102 (ab5ebf69acec) [stable]", "0.2.102"),
        ("2026.08.11-e8db854", "2026.08.11"),
        ("omp/17.3.4", "17.3.4"),
        ("no version here", None),
    ]:
        got = semverish(banner)
        if got != want:
            bad.append(f"semverish({banner!r}) = {got!r}, want {want!r}")

    doctor = ROOT / "crates/pixtuoid/src/doctor.rs"
    m = re.search(r"const IMPLAUSIBLE_MAJOR: u64 = (\d+);", doctor.read_text())
    if m is None:
        bad.append("doctor.rs no longer declares IMPLAUSIBLE_MAJOR — the mirror is unpinned")
    elif int(m.group(1)) != IMPLAUSIBLE_MAJOR:
        bad.append(
            f"IMPLAUSIBLE_MAJOR drifted: doctor.rs says {m.group(1)}, this says "
            f"{IMPLAUSIBLE_MAJOR}"
        )

    for line in bad:
        print(f"  {line}", file=sys.stderr)
    if bad:
        print(f"fixture-age selftest: {len(bad)} failure(s)", file=sys.stderr)
    return 1 if bad else 0

def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--max-age-days", type=int, default=180)
    args = ap.parse_args()
    return selftest() or report(args.max_age_days)

if __name__ == "__main__":
    sys.exit(main())
