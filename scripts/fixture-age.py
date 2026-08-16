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


# A dotted-run major at or above this looks like a YEAR/date token, not a semver
# major. MIRRORS `doctor::parse_version`'s const of the same name — the pair is
# pinned by `test_semverish_matches_doctors_documented_cases`, because the two
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


def cross_check(prov_dir: pathlib.Path, prov: dict) -> list[str]:
    """Falsify what the bytes can answer. A provenance whose every field is
    unverifiable is a claim, not a record — a wholly false one (`cli: codex`,
    `version: 0.0.0-A-LIE`) passed every gate in this tree until this ran.

    Three axes are answerable in-repo: the payloads' own `_pixtuoid_source`,
    `captured` vs the shim's `_shim_ts_ms`, and `captured` vs a capture date the
    recording CLI put in a TRANSCRIPT FILENAME. `cli` is checked in Rust, where
    the registry's probe argv is in scope (`every_scenario_declares_its_provenance`).
    """
    out = []
    stamps, dates = set(), set()
    hooks = prov_dir / "hook-payloads.jsonl"
    if hooks.is_file():
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
    # codex and omp name their transcripts by capture instant, which is the only
    # date evidence in a transcript-only fixture — 12 of these trees ship no hook
    # payloads at all, and the whole function used to return [] for every one.
    for f in prov_dir.rglob("*.jsonl"):
        m = re.match(r"(?:rollout-)?(\d{4}-\d{2}-\d{2})T\d{2}-\d{2}-\d{2}", f.name)
        if m:
            dates.add(m.group(1))
    if len(stamps) > 1:
        out.append(f"payloads carry TWO sources ({sorted(stamps)}) — a capture scooped up another CLI")
    # Under `fixtures/<source>/<scenario>/` the id IS the grandparent dir, so a
    # single WRONG stamp is answerable too. The single-owner trees (`<module>/
    # fixtures/`) key by module name, which is not the source id for all of them.
    elif len(stamps) == 1 and prov_dir.parent.parent.name == "fixtures":
        expect = prov_dir.parent.name
        if (only := next(iter(stamps))) != expect:
            out.append(f"payloads are stamped `{only}` but this tree is `{expect}`")
    captured = str(prov.get("captured", ""))
    if dates and captured not in ("", "unknown") and captured not in dates:
        out.append(
            f"`captured` {captured} contradicts the shim stamps in the bytes "
            f"({sorted(dates)}) — the recorder dates in UTC"
        )
    return out


# `provenance.schema.json` is the ONE statement of what each origin requires —
# this gate owns the single-owner trees no Rust gate reaches, and it shipped
# without `command` while the Rust gate and the README both required it.
SCHEMA = json.loads((FIXTURES / "fixtures/provenance.schema.json").read_text())["origins"]
ORIGINS = frozenset(SCHEMA)


def required(origin: str) -> list[str]:
    return SCHEMA[origin]["required"]


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
        if origin not in ORIGINS:
            bad.append(
                f"{rel}: `origin` is {origin!r}, not " + "/".join(sorted(ORIGINS))
            )
            continue
        for field in required(origin):
            if not str(prov.get(field, "")).strip():
                bad.append(f"{rel}: {origin} fixture has no `{field}`")
        if origin != "recorded":
            continue
        seen += 1
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
    ap.add_argument("--check-metadata", action="store_true",
                    help="assert the fields this report reads are present and parseable")
    ap.add_argument("--max-age-days", type=int, default=180)
    args = ap.parse_args()
    if not args.check_metadata:
        return report(args.max_age_days)
    return selftest() or check_metadata()


if __name__ == "__main__":
    sys.exit(main())
