#!/usr/bin/env python3
"""Self-test for check_upstream_drift.py — a regex-parser regression is a SILENT
monitor death: the weekly job either alarms on junk or watches nothing.

Run: `python3 scripts/check_upstream_drift_selftest.py` (exit 0 = pass). No pytest
dependency on purpose — the repo has no Python test harness."""

from __future__ import annotations

import ast
import contextlib
import io
import json
import pathlib
import re
import sys
import traceback
import urllib.error

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
# A `__pycache__` hit made this gate test a DIFFERENT file than the one on disk —
# a same-size restore inside one mtime tick left the .pyc valid. Not a gate, then.
sys.dont_write_bytecode = True
import check_upstream_drift as d  # noqa: E402

FAILS: list[str] = []


def check(cond: bool, msg: str) -> None:
    if not cond:
        FAILS.append(msg)


def _http_error(code: int) -> urllib.error.HTTPError:
    return urllib.error.HTTPError("https://x/y", code, "msg", {}, io.BytesIO(b""))


def test_try_fetch_classifies_permanent_vs_transient() -> None:
    real = d.fetch
    try:
        # Deliberately not `breaking`: a 404 says nothing about whether upstream
        # renamed anything, only that we are looking in the wrong place.
        for code in (404, 410, 451):
            d.fetch = lambda _u, c=code: (_ for _ in ()).throw(_http_error(c))
            r = d.Report()
            out = d.try_fetch("https://x/y", "T", r)
            check(out is None, f"{code}: returns None")
            check(
                len(r.blind) == 1 and not r.errors,
                f"{code}: -> blind (got blind={r.blind} errors={r.errors})",
            )
            check(str(code) in (r.blind[0] if r.blind else ""), f"{code}: message names the status")

        for code in (500, 502, 503, 429, 403):
            d.fetch = lambda _u, c=code: (_ for _ in ()).throw(_http_error(c))
            r = d.Report()
            d.try_fetch("https://x/y", "T", r)
            check(
                not r.blind and len(r.errors) == 1,
                f"{code}: -> transient (got blind={r.blind} errors={r.errors})",
            )

        d.fetch = lambda _u: (_ for _ in ()).throw(urllib.error.URLError("conn refused"))
        r = d.Report()
        d.try_fetch("https://x/y", "T", r)
        check(
            not r.blind and len(r.errors) == 1,
            f"URLError -> transient (got blind={r.blind} errors={r.errors})",
        )

        d.fetch = lambda _u: "BODY"
        r = d.Report()
        out = d.try_fetch("https://x/y", "T", r)
        check(out == "BODY" and not r.blind and not r.errors, "success returns body, no buckets")
    finally:
        d.fetch = real


# One snippet per anchored document; a new ANCHORS entry with no sample here
# fails the gate test below.
ANCHOR_SAMPLES: dict[str, str] = {
    d.OMP_EXT_DISCOVERY_URL:
        'export async function discoverExtensionModulePaths() {}\n',
    d.OPENCLAW_PATHS_URL: "export const DEFAULT_GATEWAY_PORT = 18789;\n",
}

# A document that satisfies NO anchor — the "upstream reorganized this file"
# stand-in: content that fetches fine and owns nothing.
_UNANCHORED = "mod config;\nmod executor;\npub use config::*;\n"


def test_anchor_gate_fires_in_both_directions() -> None:
    """A PURE UPSTREAM REFACTOR must produce probe-health, never drift (#793)."""
    real = d.fetch
    try:
        # A floor, because everything below iterates `ANCHORS`: an emptied table
        # would run zero loop bodies and this test would pass VACUOUSLY.
        check(len(d.ANCHORS) >= 2, f"ANCHORS covers every swept document, got {len(d.ANCHORS)}")
        missing_samples = sorted(set(d.ANCHORS) - set(ANCHOR_SAMPLES))
        check(not missing_samples, f"every ANCHORS entry needs a sample: {missing_samples}")

        for url, anchor in sorted(d.ANCHORS.items()):
            served: list[str] = []

            d.fetch = lambda u, _s=served: (_s.append(u), _UNANCHORED)[1]
            r = d.Report()
            out = d.fetch_anchored(url, "T", r)
            check(served == [url], f"{anchor.owns}: the stubbed fetch actually ran")
            check(out is None, f"{anchor.owns}: no anchor -> the sweep is skipped")
            check(len(r.blind) == 1 and not r.errors, f"{anchor.owns}: -> blind, got {r.blind}")
            # Guarded index: a regressed gate leaves `blind` empty, and that must
            # REPORT rather than abort the suite before the later tests run.
            check(
                "NOT evidence that upstream changed" in (r.blind[0] if r.blind else ""),
                f"{anchor.owns}: the line disclaims upstream causation, got {r.blind!r}",
            )

            sample = ANCHOR_SAMPLES.get(url, "")
            d.fetch = lambda _u, _s=sample: _s
            r = d.Report()
            out = d.fetch_anchored(url, "T", r)
            check(
                out == sample and not r.blind and not r.errors,
                f"{anchor.owns}: anchor present -> body returned, got blind={r.blind}",
            )
    finally:
        d.fetch = real


def test_two_declarations_are_probe_health_not_a_first_match() -> None:
    """A first-match read picks the decoy the day upstream grows a `#[cfg(test)]`
    twin above the real declaration, and then files breaking drift against a
    healthy upstream."""
    check(d.sole_match(r"P\s*=\s*(\d+)", "P = 7777\n") is not None, "one match -> the match")
    check(d.sole_match(r"P\s*=\s*(\d+)", "nothing here\n") is None, "no match -> None")
    check(d.sole_match(r"P\s*=\s*(\d+)", "P = 1\nP = 7777\n") is None,
          "two matches -> None, never the first")

    real = d.fetch
    try:
        rep0 = d.Report()
        ports = sorted(d.read_our_names(rep0).openclaw_gateway_port or ())
        check(bool(ports), "the fragment supplies the gateway port")

        def drive(doc: str) -> d.Report:
            def stub(u: str, _d: str = doc) -> str:
                if u == d.OPENCLAW_PATHS_URL:
                    return _d
                raise urllib.error.URLError("offline: not this case's document")

            d.fetch = stub
            rep = d.Report()
            d.run_checks(d.read_our_names(rep), report=rep)
            return rep

        sole = drive(f"export const DEFAULT_GATEWAY_PORT = {ports[0]};\n")
        check(not sole.breaking, f"one declaration matching ours is silent; got {sole.breaking}")

        twin = drive(
            "const DEFAULT_GATEWAY_PORT = 1234; // a fixture twin, ABOVE the real one\n"
            f"export const DEFAULT_GATEWAY_PORT = {ports[0]};\n"
        )
        check(
            not twin.breaking and any("DEFAULT_GATEWAY_PORT" in b for b in twin.blind),
            f"ambiguity must abstain, never report the decoy's port as drift; "
            f"breaking={twin.breaking} blind={twin.blind}",
        )
    finally:
        d.fetch = real


def test_no_anchor_is_a_name_the_watcher_checks() -> None:
    """`Anchor`'s docstring bans this in prose, and prose has no failure mode.

    Such an anchor vanishes WITH the names on the very rename it exists to tell
    apart from a stale pin, and the blind line then argues the maintainer out of
    the fix."""
    rep = d.Report()
    ours = d.read_our_names(rep)
    names = {
        n
        for f in ours.__dataclass_fields__
        if isinstance(v := getattr(ours, f), set)
        for n in v
    }
    # An empty vocabulary would make every `re.search` below vacuous.
    check(bool(names), "the fragments supply the swept vocabulary")
    circular = sorted(
        (a.owns, n) for a in d.ANCHORS.values() for n in names if re.search(a.pattern, n)
    )
    check(not circular, f"an anchor must not match a name it guards: {circular}")


def test_main_prints_exactly_the_four_buckets() -> None:
    """`drift_watch`'s `Buckets` requires these four keys and refuses any other,
    so a rename here fails the run instead of dropping findings."""
    real = d.fetch
    try:
        d.fetch = lambda _u: (_ for _ in ()).throw(urllib.error.URLError("offline"))
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            d.main()
        got = json.loads(out.getvalue())
        check(
            set(got) == {"breaking", "review", "blind", "errors"},
            f"main() prints the four buckets, got {sorted(got)}",
        )
        check(bool(got["errors"]), f"an offline run files its fetches as errors: {got}")
    finally:
        d.fetch = real


def test_report_is_the_only_way_to_file_a_finding() -> None:
    """`Report` owns the buckets and the blind line's wording."""
    r = d.Report()
    r.add_breaking("B-LINE")
    r.add_review("R-LINE")
    r.add_error("E-LINE")
    r.add_blind("WHAT", "WHERE", "CONSEQUENCE")
    check(r.breaking == ["B-LINE"], f"add_breaking -> breaking, got {r.breaking}")
    check(r.review == ["R-LINE"], f"add_review -> review, got {r.review}")
    check(r.errors == ["E-LINE"], f"add_error -> errors, got {r.errors}")
    check(len(r.blind) == 1, f"add_blind -> blind, got {r.blind}")

    line = r.blind[0] if r.blind else ""
    check(
        all(p in line for p in ("WHAT", "WHERE", "CONSEQUENCE")),
        f"add_blind keeps the caller's three facts: {line!r}",
    )
    check(
        "NOT evidence that upstream changed" in line and "do NOT change a decoder" in line,
        f"the default wording disclaims upstream causation: {line!r}",
    )
    ours = d.Report()
    ours.add_blind("WHAT", "WHERE", "CONSEQUENCE", our_source=True)
    our_line = ours.blind[0] if ours.blind else ""
    check(
        "nothing upstream was consulted" in our_line.lower()
        and "Fix the script" in our_line,
        f"our_source=True blames the script, not upstream: {our_line!r}",
    )

    # An AST walk, not a regex: a regex also fires on prose QUOTING the spelling.
    src = (pathlib.Path(__file__).parent / "check_upstream_drift.py").read_text()
    tree = ast.parse(src)
    report_cls = next(
        (n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "Report"), None
    )
    check(report_cls is not None, "the module defines a top-level `class Report`")
    inside = {id(n) for n in ast.walk(report_cls)} if report_cls else set()
    buckets = {"breaking", "review", "blind", "errors"}
    stray = []
    for node in ast.walk(tree):
        if id(node) in inside:
            continue
        if not (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)):
            continue
        if node.func.attr != "append":
            continue
        target = node.func.value
        name = (
            target.id if isinstance(target, ast.Name)
            else target.attr if isinstance(target, ast.Attribute)
            else None
        )
        if name in buckets:
            stray.append(f"{name}.append at check_upstream_drift.py:{node.lineno}")
    check(
        not stray,
        f"a finding must be filed through `report.add_*`, never appended to a "
        f"bucket — and no local list outside `Report` may borrow a bucket's name "
        f"(call it `findings`): {stray}",
    )


def test_every_swept_url_declares_an_anchor() -> None:
    """Every swept document declares an anchor. The anchor gate is #793's fix: a
    pin that 200s as a facade reads as mass drift without it. A new sweep declaring
    none would skip that gate silently — hence a census over every `*_URL`, not a
    floor on ANCHORS."""
    import re as _re

    src = (pathlib.Path(__file__).parent / "check_upstream_drift.py").read_text()
    urls = _re.findall(r"^([A-Z][A-Z0-9_]*_URLS?)\s*=", src, _re.M)
    check(bool(urls), "the *_URL declaration shape moved — this census reads nothing")
    unclassified = []
    for name in urls:
        value = getattr(d, name)
        for one in value if isinstance(value, tuple) else [value]:
            if one not in d.ANCHORS:
                unclassified.append(name)
    check(
        not unclassified,
        f"{unclassified} declare no anchor, so the #793 stale-pin gate does not "
        f"cover them.",
    )


def test_every_surface_row_names_a_key_the_emitter_actually_writes() -> None:
    """Nothing in either language binds `SURFACE_ROWS` to the keys the RUST side
    writes, so a key renamed in `src/drift_surface.rs` would surface only as a
    probe-health line in the weekly run — late enough to have already skipped a
    sweep. This is that binding, and it runs in `just lint`. The reverse too: a key
    nothing consumes is the "maintaining what we don't use" this file deletes."""
    consumed = set()
    for field, rel, group, key in d.SURFACE_ROWS:
        frag = d.load_fragment(rel)
        check(group in frag, f"{rel} has no `{group}` group for {field}")
        check(key in frag[group], f"{rel} {group} has no `{key}` for {field}")
        check(frag[group].get(key), f"{rel} {group}.{key} is empty")
        consumed.add((rel, group, key))

    emitted = {
        (rel, group, key)
        for rel in (d.CORE_LIB_FRAGMENT, d.CORE_BIN_FRAGMENT)
        for group, rows in d.load_fragment(rel).items()
        for key in rows
    }
    unconsumed = sorted(f"{rel.split('/')[1]} {g}.{k}" for rel, g, k in emitted - consumed)
    check(not unconsumed, f"emitted but read by nobody: {unconsumed}")


def test_one_absent_surface_row_does_not_blind_the_sources_beside_it() -> None:
    """A gap in the emitted surface costs exactly what it feeds: per-KEY
    isolation, or one missing row silently darkens the whole watch."""
    real = d.load_fragment
    try:
        full = {rel: real(rel) for rel in (d.CORE_LIB_FRAGMENT, d.CORE_BIN_FRAGMENT)}

        holed = {rel: json.loads(json.dumps(f)) for rel, f in full.items()}
        del holed[d.CORE_BIN_FRAGMENT]["shipped"]["openclaw.default_gateway_port"]
        d.load_fragment = lambda rel: holed[rel]
        rep = d.Report()
        ours = d.read_our_names(rep)
        check(
            ours.openclaw_gateway_port is None,
            f"the holed field is dark, got {ours.openclaw_gateway_port!r}",
        )
        check(bool(ours.omp_extension_subdir), "omp_extension_subdir must survive a sibling's gap")
        named = [b for b in rep.blind if "`openclaw_gateway_port`" in b]
        check(
            len(named) == 1,
            f"exactly one probe-health line names openclaw_gateway_port: {rep.blind}",
        )

        # A fragment that does not load at all darkens only what IT feeds.
        def half(rel):
            if rel == d.CORE_BIN_FRAGMENT:
                raise OSError("gone")
            return full[rel]

        d.load_fragment = half
        rep2 = d.Report()
        ours2 = d.read_our_names(rep2)
        check(
            ours2.openclaw_gateway_port is None,
            "a field of the missing fragment is dark when its fragment is gone",
        )
        check(
            bool(ours2.omp_extension_subdir),
            "a field of the other fragment survives it going missing",
        )
    finally:
        d.load_fragment = real


def test_every_source_check_fires_on_a_vanish_and_stays_silent_otherwise() -> None:
    """The gate #941 asks for: EVERY surface row's check, both directions, offline.

    Each document is BUILT from the set we depend on, so an added event cannot make
    this stale."""
    real = d.fetch
    try:
        rep0 = d.Report()
        ours = d.read_our_names(rep0)
        check(not rep0.blind, f"the fragments load: {rep0.blind}")
        full: dict[str, list[str]] = {
            f: sorted(getattr(ours, f) or ()) for f, *_ in d.SURFACE_ROWS
        }

        cases = [
            ("openclaw_gateway_port", lambda ns: {
                d.OPENCLAW_PATHS_URL: f"export const DEFAULT_GATEWAY_PORT = {ns[0]};\n"}),
            ("omp_extension_subdir", lambda ns: {
                d.OMP_EXT_DISCOVERY_URL:
                    "export async function discoverExtensionModulePaths() {}\n"
                    + "".join(
                        f'discoverExtensionModulePaths(ctx, path.join(dir, "{n}"))\n'
                        for n in ns
                    )}),
        ]
        covered = {c[0] for c in cases}
        rows = {f for f, *_ in d.SURFACE_ROWS}
        check(covered == rows, f"every surface row needs a case; differ: {covered ^ rows}")

        def drive(docs: dict[str, str]) -> d.Report:
            """OFFLINE: every URL this case does not serve raises rather than going
            to the network. `else real(u)` made `just lint` issue 208 live requests
            per run, and `lint` joins its jobs with `wait`, so a blackholing network
            would hang preflight and pre-push at 30s/request."""
            def stub(u: str, _d: dict[str, str] = docs) -> str:
                if u in _d:
                    return _d[u]
                raise urllib.error.URLError("offline: not this case's document")

            d.fetch = stub
            rep = d.Report()
            d.run_checks(d.read_our_names(rep), report=rep)
            return rep

        for field, build in cases:
            names = full[field]
            check(bool(names), f"{field}: the fragment supplies a set")
            victims = names[:1]
            # The vanish arm ADDS as it drops, in the name's own shape. The decoy is
            # built from the VICTIM, so the mutated document carries the dropped
            # name's spelling in a NON-declaration position: that is the only shape
            # that can tell `f'"{name}"' not in text` from `name not in text`.
            stem = victims[0]
            if stem.isdigit():
                # A numeric VALUE row: a `Pxd` suffix leaves our digits at the front,
                # where the reader's `(\d+)` finds them and the arm reads unchanged.
                sfx = ("0", "00")
            else:
                sfx = ("Pxd", "Pxdb")
            kept = [n for n in names if n not in victims]

            intact = drive(build(names))
            check(not intact.breaking,
                  f"{field}: an intact document must stay silent; got {intact.breaking}")
            rep = drive(build(kept + [stem + s for s in sfx]))
            check(any(victims[0] in x for x in rep.breaking),
                  f"{field}: a vanished `{victims[0]}` must fire; got {rep.breaking} "
                  f"(skipped as probe health? {rep.blind})")
    finally:
        d.fetch = real


def main() -> int:
    # Derived, not hand-listed — a missing test is inert while the suite prints
    # "all checks passed". `ast.walk` over `.body`: a nested/async def shadows too.
    tree = ast.parse(pathlib.Path(__file__).read_text())
    defined: dict[str, list[int]] = {}
    for n in ast.walk(tree):
        name = getattr(n, "name", None) if isinstance(
            n, (ast.FunctionDef, ast.AsyncFunctionDef)
        ) else None
        if name is None and isinstance(n, ast.Assign):
            name = next(
                (t.id for t in n.targets if isinstance(t, ast.Name)),
                None,
            )
        if name and name.startswith("test_"):
            defined.setdefault(name, []).append(n.lineno)
    # Here, not in a test: a test detecting duplicate names is itself shadowable.
    if dupes := sorted(k for k, v in defined.items() if len(v) > 1):
        print(f"DRIFT SELFTEST FAILED:\n  - test name bound twice, shadowing: {dupes}")
        return 1
    tests = tuple(o for n, o in list(globals().items()) if n.startswith("test_"))
    # The binding a name RESOLVES to must be the one the AST saw: a
    # `globals()[...]` rebind leaves the AST untouched, so only this catches it.
    for t in tests:
        lines = defined.get(t.__name__)
        if lines is None or t.__code__.co_firstlineno not in lines:
            print(
                f"DRIFT SELFTEST FAILED:\n  - {t.__name__} resolves to a function "
                f"defined at line {t.__code__.co_firstlineno}, which this file does "
                f"not declare (rebound at runtime?)"
            )
            return 1
    for t in tests:
        # A raise past a recorded `check()` would otherwise replace the whole FAILS
        # list — the line that already explains the failure — with a traceback.
        try:
            t()
        except Exception:  # noqa: BLE001 - a raising test is a failing test
            FAILS.append(f"{t.__name__} raised:\n{traceback.format_exc()}")
    if FAILS:
        print("DRIFT SELFTEST FAILED:")
        for f in FAILS:
            print(f"  - {f}")
        return 1
    print("drift selftest: all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
