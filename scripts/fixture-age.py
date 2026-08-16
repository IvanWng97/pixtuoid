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
            prov = json.loads(p.read_text())
        except (OSError, json.JSONDecodeError) as e:
            yield p, {"_unparseable": str(e)}
            continue
        # Valid JSON that is not an object (`null`, `[]`, a bare number) would
        # otherwise reach the field reads and crash the whole sweep on one file,
        # replacing every other file's diagnostic with a traceback.
        if not isinstance(prov, dict):
            yield p, {"_unusable": f"top level is {type(prov).__name__}, not an object"}
            continue
        yield p, prov


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


def cross_check(prov_dir: pathlib.Path, prov: dict) -> list[str]:
    """Falsify what the bytes can answer. A provenance whose every field is
    unverifiable is a claim, not a record — a wholly false one (`cli: codex`,
    `version: 0.0.0-A-LIE`) passed every gate in this tree until this ran.

    Only two axes are answerable in-repo, and only where the shim stamped them:
    the payloads' own `_pixtuoid_source`, and `_shim_ts_ms` vs `captured`.
    """
    out = []
    hooks = prov_dir / "hook-payloads.jsonl"
    if not hooks.is_file():
        return out
    stamps, dates = set(), set()
    for line in hooks.read_text(errors="ignore").splitlines():
        if not line.strip():
            continue
        try:
            o = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(o, dict):
            if o.get("_pixtuoid_source"):
                stamps.add(o["_pixtuoid_source"])
            if isinstance(o.get("_shim_ts_ms"), int):
                dates.add(
                    dt.datetime.fromtimestamp(o["_shim_ts_ms"] / 1000, dt.UTC)
                    .date()
                    .isoformat()
                )
    # The stamp must name ONE source. Which one is not answerable here: a
    # conformance scenario sits under `fixtures/<source>/`, a single-owner tree
    # under `<module>/fixtures/`, so the parent name is the id in one layout and
    # the literal "fixtures" in the other. A capture that scooped up a second
    # CLI's hooks is the failure this can see, and it sees it either way.
    if len(stamps) > 1:
        out.append(f"payloads carry TWO sources ({sorted(stamps)}) — a capture scooped up another CLI")
    captured = str(prov.get("captured", ""))
    if dates and captured not in ("", "unknown") and captured not in dates:
        out.append(
            f"`captured` {captured} contradicts the shim stamps in the bytes "
            f"({sorted(dates)}) — the recorder dates in UTC"
        )
    return out


# Below this the walk found almost nothing, so a pass says nothing about the
# corpus — the vacuous-pass floor its sibling in the drift selftest already has.
MIN_PROVENANCES = 20


def check_metadata() -> int:
    """The half that runs everywhere: the report's inputs must exist and parse."""
    bad = []
    seen = 0
    for p, prov in provenances():
        rel = p.relative_to(ROOT)
        if "_unparseable" in prov:
            bad.append(f"{rel}: not valid JSON ({prov['_unparseable']})")
            continue
        if "_unusable" in prov:
            bad.append(f"{rel}: {prov['_unusable']}")
            continue
        origin = prov.get("origin")
        # A single field switched the entire check off: an absent or misspelled
        # `origin` fell through to `continue` and every other field went
        # unvalidated. The Rust schema gate closes this for the conformance tree
        # only — the single-owner trees are checked here or nowhere.
        if origin not in ("recorded", "composed", "unknown"):
            bad.append(f"{rel}: `origin` is {origin!r}, not recorded/composed/unknown")
            continue
        if origin != "recorded":
            continue
        seen += 1
        for field in ("cli", "version", "captured"):
            if not str(prov.get(field, "")).strip():
                bad.append(f"{rel}: recorded fixture has no `{field}`")
        # A `version` holding the INVOCATION rather than a version reports a
        # drift of nothing for as long as it sits there — the live instance was
        # `"grok --permission-mode default"`.
        version = str(prov.get("version", ""))
        if version not in ("", "unknown") and not re.search(r"\d", version):
            bad.append(f"{rel}: `version` carries no version number: {version!r}")
        captured = str(prov.get("captured", ""))
        if captured and captured != "unknown":
            try:
                dt.date.fromisoformat(captured)
            except ValueError:
                bad.append(f"{rel}: `captured` is not an ISO date: {captured!r}")
        for problem in cross_check(p.parent, prov):
            bad.append(f"{rel}: {problem}")
    for line in bad:
        print(f"  {line}", file=sys.stderr)
    if bad:
        print(f"fixture metadata: {len(bad)} problem(s)", file=sys.stderr)
        return 1
    if seen < MIN_PROVENANCES:
        print(
            f"fixture metadata: only {seen} recorded provenance(s) under {FIXTURES} — "
            f"expected at least {MIN_PROVENANCES}; the walk found almost nothing, so "
            f"this pass says nothing about the corpus.",
            file=sys.stderr,
        )
        return 1
    print(f"fixture metadata: ok ({seen} recorded)")
    return 0


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


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check-metadata", action="store_true",
                    help="assert the fields this report reads are present and parseable")
    ap.add_argument("--max-age-days", type=int, default=180)
    args = ap.parse_args()
    return check_metadata() if args.check_metadata else report(args.max_age_days)


if __name__ == "__main__":
    sys.exit(main())
