#!/usr/bin/env python3
"""Self-test for check_upstream_drift.py — a regex-parser regression is a SILENT
monitor death: the weekly job either alarms on junk or watches nothing.

Run: `python3 scripts/check_upstream_drift_selftest.py` (exit 0 = pass).
No pytest dependency on purpose — the repo has no Python test harness.
"""

from __future__ import annotations

import ast
import io
import json
import pathlib
import re
import sys
import urllib.error

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
# A `__pycache__` hit made this gate test a DIFFERENT file than the one on disk:
# restoring a same-size checker inside one mtime tick left the .pyc valid, and a
# reverted mutation kept failing. A gate that can read stale bytes is not a gate.
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






def test_cc_doc_marker_detection_fires_both_directions() -> None:
    quiet = "\n".join(d.CC_DEPENDED_DOC_MARKERS)
    got = d.cc_doc_marker_findings(quiet)
    check(got == [], f"quiet doc -> no findings: {got!r}")

    missing = "\n".join(m for m in d.CC_DEPENDED_DOC_MARKERS if m != "CLAUDE_EFFORT")
    got = d.cc_doc_marker_findings(missing)
    check(len(got) == 1 and "CLAUDE_EFFORT" in got[0], f"vanish fires: {got!r}")

    got = d.cc_doc_marker_findings(quiet + "\nultra_effort_exit\n")
    check(len(got) == 1 and "ultra_effort_exit" in got[0], f"appearance fires: {got!r}")










# One snippet per anchored document; a new ANCHORS entry with no sample here
# fails the gate test below.
ANCHOR_SAMPLES: dict[str, str] = {
    d.CURSOR_HOOKS_URL: "\n### Hook events\n\n#### preToolUse\n",
    d.KIMI_HOOKS_URL: '"hook_event_name": "PreToolUse"',
    d.CC_TOOLS_URL: "\n# Tools reference\n",
    d.CC_HOOKS_URL: "\n# Hooks reference\n",
    d.REASONIX_HOOK_URL: 'const (\n    SessionStart Event = "SessionStart"\n)\n',
    d.CODEWHALE_HOOK_URL: "pub enum HookEvent {\n    SessionStart,\n}\n",
    d.CODEX_PROTOCOL_URL: "pub enum HookEventName {\n    SessionStart,\n}\n",
    d.HERMES_PLUGINS_URL: 'VALID_HOOKS: Set[str] = {\n    "on_session_start",\n}\n',
    d.GROK_HOOK_URL: "pub enum HookEventName {\n    SessionStart,\n}\n",
    d.OPENCLAW_HOOK_TYPES_URL: 'export type PluginHookName =\n  | "agent_end"\n',
    d.OPENCODE_EVENT_URLS[0]: 'export const Event = {\n  Created: "session.created",\n}\n',
    d.OPENCODE_EVENT_URLS[1]: 'export const Event = {\n  Asked: "permission.v2.asked",\n}\n',
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
        check(len(d.ANCHORS) >= 4, f"ANCHORS covers every swept document, got {len(d.ANCHORS)}")
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


def test_report_is_the_only_way_to_file_a_finding() -> None:
    """`Report` owns the buckets, their wording, their order, and the exit code."""
    empty = d.Report()
    check(empty.exit_code() == 0, f"an empty report exits 0, got {empty.exit_code()}")
    check("✅ No drift" in empty.render(), f"an empty report says so:\n{empty.render()}")

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

    out = r.render()
    offsets = [out.find(x) for x in ("B-LINE", "R-LINE", "WHAT", "E-LINE")]
    check(
        all(x >= 0 for x in offsets) and offsets == sorted(offsets),
        f"every bucket renders, in disposition order (offsets {offsets}):\n{out}",
    )

    # errors-ONLY is exit 2 (transient) — the one case `main()` cannot reach
    # without faking the network.
    for adder, want in (("add_breaking", 1), ("add_review", 1), ("add_error", 2)):
        one = d.Report()
        getattr(one, adder)("LINE")
        check(
            one.exit_code() == want,
            f"a {adder}-only report exits {want}, got {one.exit_code()}",
        )
    blind_only = d.Report()
    blind_only.add_blind("w", "where", "c")
    check(blind_only.exit_code() == 1, f"a blind-only report exits 1, got {blind_only.exit_code()}")
    both = d.Report()
    both.add_error("E")
    both.add_blind("w", "where", "c")
    check(both.exit_code() == 1, f"actionable outranks transient, got {both.exit_code()}")

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












def test_the_acp_method_check_separates_a_rename_from_a_restructure() -> None:
    """The `x-method` surface is this check's anchor, because `schema.json` is
    GENERATED and the key is a generator-emitted vendor extension: absent means the
    probe landed on a schema that declares no methods (probe health), NOT that ours
    was renamed. Collapsing those is what made #793 report five phantom renames."""
    real = d.fetch
    tags = (
        '"$defs":{"SessionUpdate":{"oneOf":['
        '{"properties":{"sessionUpdate":{"const":"tool_call"}}},'
        '{"properties":{"sessionUpdate":{"const":"tool_call_update"}}}]}}'
    )

    def drive(body: str) -> tuple[list[str], list[str]]:
        def stub(url: str) -> str:
            if url in (d.ACP_V1_SCHEMA_URL, d.ACP_V1_SCHEMA_UNSTABLE_URL):
                return body
            raise urllib.error.URLError("not stubbed")

        d.fetch = stub
        rep = d.Report()
        d.run_checks(d.OurNames(acp_decoded_tags={"tool_call", "tool_call_update"}), report=rep)
        pick = lambda xs: [x for x in xs if "session/update" in x or "x-method" in x]  # noqa: E731
        return pick(rep.breaking), pick(rep.blind)

    br, bl = drive('{"x-method":"session/update",' + tags + "}")
    check(not br and not bl, f"an intact schema is silent, got {br} {bl}")

    br, bl = drive('{"x-method":"session/updateV2",' + tags + "}")
    check(len(br) == 1, f"a RENAMED method is breaking drift, got {br}")
    check(not bl, f"and not probe health, got {bl}")

    br, bl = drive('{"x-side":"client",' + tags + "}")
    check(not br, f"a RESTRUCTURE must claim no rename, got {br}")
    check(len(bl) == 1, f"a RESTRUCTURE is one probe-health line, got {bl}")
    d.fetch = real






def test_report_separates_verified_change_from_probe_health() -> None:
    """The two dispositions must not share a heading: the heading is the
    instruction, and a reader who trusts a wrong one edits a decoder (#793).
    """
    real_run = d.run_checks
    try:
        def fake(*a: object, report: d.Report, **k: object) -> None:
            report.add_breaking("VERIFIED-CHANGE-LINE")
            report.add_blind("PROBE-HEALTH-LINE", "somewhere", "Checks were SKIPPED.")

        d.run_checks = fake
        buf = io.StringIO()
        real_stdout, sys.stdout = sys.stdout, buf
        try:
            code = d.main()
        finally:
            sys.stdout = real_stdout
        out = buf.getvalue()

        check(code == 1, f"either bucket is actionable, exit 1 (got {code})")
        verified_hd = "## ⛔ Verified upstream change"
        probe_hd = "## 🩺 Probe could NOT verify"
        check(verified_hd in out and probe_hd in out, f"both headings present:\n{out}")
        order = [
            out.find(verified_hd),
            out.find("VERIFIED-CHANGE-LINE"),
            out.find(probe_hd),
            out.find("PROBE-HEALTH-LINE"),
        ]
        check(
            all(x >= 0 for x in order) and order == sorted(order),
            f"each line filed under its own heading (offsets {order}):\n{out}",
        )
        check(
            "do NOT change a decoder" in out,
            "the probe-health section says what NOT to do",
        )

        # A blind-ONLY case, because the one above cannot test it: with a breaking
        # line injected, `breaking` carries the exit code and `blind`'s
        # contribution is never exercised.
        def blind_only(*a: object, report: d.Report, **k: object) -> None:
            report.add_blind("PROBE-HEALTH-ONLY", "somewhere", "Checks were SKIPPED.")

        d.run_checks = blind_only
        buf = io.StringIO()
        real_stdout, sys.stdout = sys.stdout, buf
        try:
            code = d.main()
        finally:
            sys.stdout = real_stdout
        check(code == 1, f"a blind-ONLY report is actionable and exits 1, got {code}")
        check(
            "Probe could NOT verify" in buf.getvalue()
            and "Verified upstream change" not in buf.getvalue(),
            "a blind-only report renders ONLY the probe-health section",
        )
    finally:
        d.run_checks = real_run




def test_report_h1_is_the_issue_title_and_carries_the_disposition() -> None:
    """The H1 is a CROSS-FILE contract: upstream-drift.yml titles the GitHub issue
    with `head -1 | sed 's/^# //'` rather than keeping its own copy of these strings.
    """
    real = d.run_checks
    cases = [
        ("breaking", "Upstream CLI wire-format drift detected"),
        ("review", "New upstream events to review"),
        ("blind", "Upstream drift watch could not verify — repin needed"),
        ("clean", "Upstream wire-format watch: no drift"),
    ]
    try:
        for name, want in cases:
            def fake(*a: object, report: d.Report, _n: str = name, **k: object) -> None:
                if _n == "blind":
                    report.add_blind("LINE", "somewhere", "Checks were SKIPPED.")
                elif _n != "clean":
                    getattr(report, f"add_{_n}")("LINE")

            d.run_checks = fake
            buf = io.StringIO()
            real_stdout, sys.stdout = sys.stdout, buf
            try:
                d.main()
            finally:
                sys.stdout = real_stdout
            got = buf.getvalue().splitlines()[0]
            check(got == f"# {want}", f"{name}: H1 is {want!r}, got {got!r}")
    finally:
        d.run_checks = real

    wf = (pathlib.Path(__file__).parents[1] / ".github/workflows/upstream-drift.yml").read_text()
    check(
        "head -1 drift-report.md" in wf and "s/^# //" in wf,
        "upstream-drift.yml still titles the issue from the report's H1",
    )
    check(
        "Verified upstream change" not in wf and "could not verify — repin" not in wf,
        "upstream-drift.yml keeps NO second copy of the report's strings",
    )



# A minimal `env.ts` satisfying every omp-env sweep, shaped like the real file in
# the two ways the sweep can be fooled: `.env`/`OMP_` appear in JSDoc as well as
# code, and the quote detector (a lone `"` inside a single-quoted string) sits
# ABOVE a comment that names checked symbols. Tracking only `"` desynchronises
# there and leaks every later comment back into the "code" — which is why the
# masking cases below live AFTER the detector, not before it.




















def test_the_floor_is_what_we_already_handle() -> None:
    """A parse returning fewer names than we already decode is broken or degraded,
    and must file probe health rather than five ⛔ against working decoders —
    #929's false ⛔ on `pre_approval_request` came from exactly a partial
    document. Proven by running the sweep, not by reading the comparison."""
    rep = d.Report()
    ours = d.OurNames(cc={f"ev{i}" for i in range(9)})
    check(
        not d.parse_is_believable("cc", {f"ev{i}" for i in range(7)}, ours, rep),
        "a parse below what we handle must not be believed",
    )
    check(
        any("SKIPPED for cc" in f for f in rep.blind),
        f"and must file probe health naming the source: {rep.blind}",
    )
    rep2 = d.Report()
    check(
        d.parse_is_believable("cc", ours.cc | {"brand_new"}, ours, rep2),
        "at/above the floor must be believed",
    )
    check(not rep2.blind, f"and must file nothing: {rep2.blind}")


def test_every_believability_gate_can_name_the_document_it_doubts() -> None:
    """`parse_is_believable` reads `PARSE_SOURCES[source]` to say WHICH pin to
    re-check, so a caller missing from that table raises KeyError — and only on
    the day a parse actually degrades, which is the day the report matters."""
    src = (pathlib.Path(__file__).resolve().parent / "check_upstream_drift.py").read_text()
    callers = set(re.findall(r'parse_is_believable\(\s*"(\w+)"', src))
    check(callers, "the gate still has callers")
    missing = sorted(callers - set(d.PARSE_SOURCES))
    check(
        not missing,
        f"{missing} call parse_is_believable but declare no PARSE_SOURCES row, so a "
        f"degraded parse would raise KeyError instead of filing probe health.",
    )
    stale = sorted(set(d.PARSE_SOURCES) - callers)
    check(not stale, f"PARSE_SOURCES lists {stale}, which no longer gates anything.")
    # The gate must still REFUSE: a floor nothing can fail is not a floor.
    rep = d.Report()
    caller = sorted(callers)[0]
    ours = d.OurNames(**{caller: {f"h{i}" for i in range(4)}})
    check(
        not d.parse_is_believable(caller, {"only_one"}, ours, rep),
        f"a one-name parse must fail {caller}'s floor",
    )
    check(
        any(f"SKIPPED for {caller}" in f for f in rep.blind),
        f"and must name {caller} in the probe-health line: {rep.blind}",
    )






def test_every_swept_url_declares_an_anchor() -> None:
    """A document is either anchored or deliberately exempt — never neither.

    The anchor gate is #793's fix: a pin that 200s as a facade reads as mass
    drift without it. A new sweep that declares no anchor and is not listed as
    structurally parsed would skip that gate silently, which is why this is a
    classification test over every `*_URL` rather than a floor on ANCHORS.
    """
    import re as _re

    src = (pathlib.Path(__file__).parent / "check_upstream_drift.py").read_text()
    urls = _re.findall(r"^([A-Z][A-Z0-9_]*_URLS?)\s*=", src, _re.M)
    check(len(urls) >= 4, f"the sweep still fetches documents, got {urls}")
    unclassified = []
    for name in urls:
        value = getattr(d, name)
        for one in value if isinstance(value, tuple) else [value]:
            if one not in d.ANCHORS and one not in d.UNANCHORED_BY_DESIGN:
                unclassified.append(name)
    check(
        not unclassified,
        f"{unclassified} declare neither an anchor nor structural parsing, so the "
        f"#793 stale-pin gate does not cover them.",
    )


def test_every_surface_row_names_a_key_the_emitter_actually_writes() -> None:
    """`SURFACE_ROWS` maps an `OurNames` field to a key in a fragment the RUST
    side writes. Nothing in either language binds the two, so a key renamed in
    `src/drift_surface.rs` would surface only as a probe-health line in the
    weekly run — loud enough to notice, late enough to have already skipped a
    sweep. This is the binding, and it runs in `just lint`.

    The reverse direction matters too: a key emitted that nothing consumes is
    exactly the "maintaining what we don't use" this file exists to delete.
    """
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
    """A gap in the emitted surface costs exactly what it feeds.

    The 16 hand-written readers this replaced had per-reader `try` for it; the
    fragments give per-KEY isolation instead, and it has to be the same promise
    or one missing row silently darkens the whole watch.
    """
    real = d.load_fragment
    try:
        full = {rel: real(rel) for rel in (d.CORE_LIB_FRAGMENT, d.CORE_BIN_FRAGMENT)}

        holed = {rel: json.loads(json.dumps(f)) for rel, f in full.items()}
        del holed[d.CORE_LIB_FRAGMENT]["decoded"]["copilot.kinds"]
        d.load_fragment = lambda rel: holed[rel]
        rep = d.Report()
        ours = d.read_our_names(rep)
        check(ours.copilot is None, f"the holed field is dark, got {ours.copilot!r}")
        for other in ("cc", "cursor", "kimi", "acp_decoded_tags", "dispatch_names"):
            check(getattr(ours, other), f"{other} must survive a sibling's gap")
        named = [b for b in rep.blind if "copilot" in b]
        check(len(named) == 1, f"exactly one probe-health line names copilot: {rep.blind}")

        # A fragment that does not load at all darkens only what IT feeds.
        def half(rel):
            if rel == d.CORE_BIN_FRAGMENT:
                raise OSError("gone")
            return full[rel]

        d.load_fragment = half
        rep2 = d.Report()
        ours2 = d.read_our_names(rep2)
        check(ours2.cc is None, "a registration field is dark when its fragment is gone")
        check(bool(ours2.copilot), "a DECODE field survives the other fragment going missing")
    finally:
        d.load_fragment = real


def test_the_surviving_upstream_parsers_extract_from_a_snippet() -> None:
    """Each parser, against a hand-made document of the shape it reads.

    They fail SAFE (an empty parse becomes probe health, not a ⛔), but the
    selftest's own premise is that a regex-parser regression is a silent monitor
    death — and these four are what the whole remaining watch stands on. The
    coverage that used to be here went with the source-scraping parsers it also
    covered; this is the half that had to come back.
    """
    cc = d.upstream_cc_hook_events(
        "# Hooks reference\n\n"
        "| Event | When it fires |\n|---|---|\n"
        "| `PreToolUse` | before a tool |\n"
        "| `PermissionRequest` | on a gate |\n"
    )
    check(cc is not None and {"PreToolUse", "PermissionRequest"} <= cc, f"cc table: {cc}")
    check(
        d.upstream_cc_hook_events("# Hooks reference\n\nprose with no table\n") in (None, set()),
        "a table-less page must not read as a full parse",
    )

    schema = (
        '{"definitions": {"SessionEvent": {"anyOf": ['
        '{"$ref": "#/definitions/Start"}, {"$ref": "#/definitions/Stop"}]},'
        '"Start": {"properties": {"type": {"const": "session.start"},'
        '"sessionId": {"type": "string"}}},'
        '"Stop": {"properties": {"type": {"const": "tool.execution_complete"},'
        '"toolCallId": {"type": "string"}}}}}'
    )
    evs = d.upstream_copilot_events(schema)
    check(evs is not None and {"session.start", "tool.execution_complete"} <= evs, f"kinds: {evs}")
    ns = d.upstream_copilot_namespaces(schema)
    check(ns is not None and {"session", "tool"} <= ns, f"namespaces: {ns}")
    fields = d.upstream_copilot_field_names(schema)
    check(fields is not None and {"sessionId", "toolCallId"} <= fields, f"fields: {fields}")

    # An unrecognised document must not parse to a NON-EMPTY set: that is what
    # would report every name we depend on as GONE. Empty and None are both safe
    # here — the believability gate turns either into probe health — so this
    # asserts falsiness, not which of the two.
    for name, fn in (
        ("events", d.upstream_copilot_events),
        ("namespaces", d.upstream_copilot_namespaces),
        ("fields", d.upstream_copilot_field_names),
    ):
        got = fn('{"definitions": {"Unrelated": {}}}')
        check(not got, f"copilot {name}: an unrecognised schema must not parse to a set, got {got}")


def test_every_source_check_fires_on_a_vanish_and_stays_silent_otherwise() -> None:
    """The gate #941 asks for: every source's check, both directions, offline.

    Each document is BUILT from the set we actually register, so adding an event
    cannot make the test stale — and it is built in the UPSTREAM spelling, which
    is the trap that hid codewhale's arm during review (upstream declares
    `MessageSubmit`, we register `message_submit`).
    """
    real = d.fetch
    try:
        rep0 = d.Report()
        ours = d.read_our_names(rep0)
        check(not rep0.blind, f"the fragments load: {rep0.blind}")

        def pascal(name: str) -> str:
            return "".join(p.title() for p in name.split("_"))

        # source -> (url, how upstream spells the set, doc template)
        cases = [
            ("reasonix", d.REASONIX_HOOK_URL, lambda n: n,
             lambda ns: "const (\n" + "".join(f'    {n} Event = "{n}"\n' for n in ns) + ")\n"),
            ("codewhale", d.CODEWHALE_HOOK_URL, pascal,
             lambda ns: "pub enum HookEvent {\n" + "".join(f"    {n},\n" for n in ns) + "}\n"),
            ("codex", d.CODEX_PROTOCOL_URL, lambda n: n,
             lambda ns: "pub enum HookEventName {\n" + "".join(f"    {n},\n" for n in ns) + "}\n"),
            ("hermes", d.HERMES_PLUGINS_URL, lambda n: n,
             lambda ns: "VALID_HOOKS: Set[str] = {\n" + "".join(f'    "{n}",\n' for n in ns) + "}\n"),
            ("grok", d.GROK_HOOK_URL, lambda n: n,
             lambda ns: "pub enum HookEventName {\n" + "".join(f"    {n},\n" for n in ns) + "}\n"),
            ("openclaw", d.OPENCLAW_HOOK_TYPES_URL, lambda n: n,
             lambda ns: "export type PluginHookName =\n" + "".join(f'  | "{n}"\n' for n in ns)),
            ("cursor", d.CURSOR_HOOKS_URL, lambda n: n,
             lambda ns: "### Hook events\n\n" + "".join(f"#### {n}\n\n" for n in ns)),
            ("kimi", d.KIMI_HOOKS_URL, lambda n: n,
             lambda ns: "hook_event_name\n\n" + "".join(f"| `{n}` | x |\n" for n in ns)),
            ("cc", d.CC_HOOKS_URL, lambda n: n,
             lambda ns: "# Hooks reference\n\n| Event | When it fires |\n|---|---|\n"
                        + "".join(f"| `{n}` | x |\n" for n in ns)),
        ]
        check(len(cases) >= 8, "every source with a name-set check needs a row")

        for source, url, spell, render in cases:
            names = sorted(getattr(ours, source))
            check(bool(names), f"{source}: the fragment supplies a set")
            full = render([spell(n) for n in names])

            def drive(body: str) -> list[str]:
                # OFFLINE: every non-target URL raises rather than going to the
                # network. `else real(u)` made `just lint` issue 208 live
                # requests per run — and `lint` joins its jobs with `wait`, so a
                # blackholing network would hang preflight and pre-push at
                # 30s/request (justfile:1327).
                def stub(u: str, _b: str = body, _u: str = url) -> str:
                    if u == _u:
                        return _b
                    raise urllib.error.URLError("offline: not this case's document")

                d.fetch = stub
                rep = d.Report()
                d.run_checks(d.read_our_names(rep), report=rep)
                return [x for x in rep.breaking if source.split("-")[0] in x.lower()]

            # The believability gate refuses a parse smaller than what we handle,
            # so the vanish arm must ADD a name as it drops one — otherwise the
            # check is skipped as probe health and this test proves nothing.
            victim = names[0]
            # Filler derived from a SURVIVING name so it always matches that parser's
            # own character class — codex wants PascalCase, cursor rejects
            # underscores, and a filler the parser drops would shrink the set
            # below the believability floor and skip the check entirely.
            keep = [spell(n) for n in names[1:]]
            # …and in that name's own CASE STYLE: hermes' parser takes only
            # `[a-z_]`, so a `Pxd` suffix would be dropped just like a
            # wrong-class one.
            # …in that name's own case style AND without digits: hermes' parser
            # takes only `[a-z_]` and cursor's headings only `[A-Za-z]`, so a
            # mis-shaped filler is dropped exactly like a wrong-class one — and a
            # dropped filler shrinks the set below the floor, skipping the check.
            lower = keep[0].islower()
            extra = [keep[0] + ("_pxd" if lower else "Pxd"),
                     keep[0] + ("_pxdb" if lower else "Pxdb")]
            gone = render(keep + extra)

            check(not drive(full), f"{source}: an intact document must stay silent")
            fired = drive(gone)
            check(
                any(victim in x for x in fired),
                f"{source}: a vanished `{victim}` must fire; got {fired}",
            )
    finally:
        d.fetch = real


def main() -> int:
    # Derived, not hand-listed: a test missing from the runner is inert while
    # the suite still prints "all checks passed". Derivation cannot see the OTHER
    # half — a second `def` of the same name replaces the binding — so that is an
    # AST scan, and it runs HERE rather than as a test because a test detecting
    # duplicate names is itself shadowable by a duplicate.
    defined = [
        n.name
        for n in ast.parse(pathlib.Path(__file__).read_text()).body
        if isinstance(n, ast.FunctionDef) and n.name.startswith("test_")
    ]
    if dupes := sorted({n for n in defined if defined.count(n) > 1}):
        print(f"DRIFT SELFTEST FAILED:\n  - test name defined twice, shadowing: {dupes}")
        return 1
    tests = tuple(o for n, o in list(globals().items()) if n.startswith("test_"))
    for t in tests:
        t()
    if FAILS:
        print("DRIFT SELFTEST FAILED:")
        for f in FAILS:
            print(f"  - {f}")
        return 1
    print("drift selftest: all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
