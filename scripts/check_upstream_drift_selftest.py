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
import traceback
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
    d.HERMES_SHELL_HOOK_URL: '_BLOCKING_EVENTS = frozenset({"pre_tool_call"})\n',
    d.OMP_SESSION_ENTRIES_URL: 'export type SessionEntry = { type: "session" }\n',
    d.OMP_EXIT_DIAG_URL: 'const SESSION_EXIT_CUSTOM_TYPE = "session_exit";\n',
    d.OMP_AI_TYPES_URL: 'export type Block = { type: "toolCall" };\n',
    d.OMP_ASK_URL: 'export class AskTool { name = "ask"; }\n',
    d.CODEX_ROLLOUT_ITEM_URL: "pub enum RolloutItem {\n    SessionMeta,\n}\n",
    d.CODEX_MODELS_URL: "pub enum ResponseItem {\n    FunctionCall,\n}\n",
    d.GROK_HOOK_URL: "pub enum HookEventName {\n    SessionStart,\n}\n",
    d.GROK_NOTIFICATION_URL: "pub enum SessionUpdate {\n    HookExecution,\n}\n",
    d.GROK_SESSION_STORAGE_URL: 'const XAI_SESSION_UPDATE_METHOD: &str = "_x.ai/session/update";\n',
    d.OPENCLAW_HOOK_TYPES_URL: 'export type PluginHookName =\n  | "agent_end"\n',
    d.OPENCLAW_PATHS_URL: "export const DEFAULT_GATEWAY_PORT = 18789;\n",
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


def test_the_hermes_blocking_watch_fires_on_an_appearance_not_a_vanish() -> None:
    """The one APPEARANCE watch among the name checks, so the vanish census cannot
    cover it: exit 0 on a blocking approval hook ANSWERS the prompt (invariant #5).
    """
    real = d.fetch
    try:
        rep0 = d.Report()
        ours = d.read_our_names(rep0)
        unsafe = sorted(d.HERMES_BLOCKING_UNSAFE & (ours.hermes or set()))
        check(bool(unsafe), f"we register something the watch calls unsafe: {unsafe}")

        def drive(members: list[str]) -> d.Report:
            body = "_BLOCKING_EVENTS = frozenset({%s})\n" % ", ".join(
                f'"{m}"' for m in members
            )

            def stub(u: str, _b: str = body) -> str:
                if u == d.HERMES_SHELL_HOOK_URL:
                    return _b
                raise urllib.error.URLError("offline: not this case's document")

            d.fetch = stub
            rep = d.Report()
            d.run_checks(d.read_our_names(rep), report=rep)
            return rep

        quiet = drive(["pre_tool_call"])
        check(not quiet.breaking, f"an observer-only set stays silent: {quiet.breaking}")
        loud = drive(["pre_tool_call", unsafe[0]])
        check(any(unsafe[0] in x for x in loud.breaking),
              f"`{unsafe[0]}` turning blocking must fire; got {loud.breaking}")

        # The OTHER appearance direction on the plugins document: a hook we
        # register being reclassified as unservable by a shell hook.
        registered = sorted(ours.hermes or ())
        check(bool(registered), "the fragment supplies hermes' registration set")

        def plugins(unservable: list[str]) -> d.Report:
            body = (
                "VALID_HOOKS: Set[str] = {\n"
                + "".join(f'    "{h}",\n' for h in registered)
                + "}\n"
                + "SHELL_UNSUPPORTED_HOOKS: Set[str] = {\n"
                + "".join(f'    "{h}",\n' for h in unservable)
                + "}\n"
            )

            def stub(u: str, _b: str = body) -> str:
                if u == d.HERMES_PLUGINS_URL:
                    return _b
                raise urllib.error.URLError("offline: not this case's document")

            d.fetch = stub
            rep = d.Report()
            d.run_checks(d.read_our_names(rep), report=rep)
            return rep

        ok = plugins(["transform_api_error_classification"])
        check(not ok.breaking, f"a hook we do not register stays silent: {ok.breaking}")
        bad = plugins([registered[0]])
        check(any(registered[0] in x for x in bad.breaking),
              f"`{registered[0]}` turning unservable must fire; got {bad.breaking}")
        # The anchor is the declaration itself, so a moved set is probe health.
        d.fetch = lambda _u: "BLOCK_EXIT_CODE = 2\n"
        gone = d.Report()
        d.run_checks(d.read_our_names(gone), report=gone)
        check(not gone.breaking, f"a moved declaration is not drift: {gone.breaking}")
    finally:
        d.fetch = real


def test_enum_body_survives_struct_variants_and_indentation() -> None:
    """`(.*?)\\}` stopped at the first `}` (a struct variant truncated the enum);
    `(.*?)\\n\\}` additionally demanded a column-0 closing brace, so an indented
    enum read as "moved upstream" (#793).
    """
    struct_variant = (
        "pub enum HookEvent {\n"
        "    SessionStart,\n"
        "    ToolCallBefore { name: String, args: Value },\n"
        "    SessionEnd,\n"
        "}\n"
    )
    got = d.upstream_codewhale_hooks(struct_variant)
    check(
        got is not None and "session_end" in got,
        f"a variant AFTER a struct variant survives (no first-`}}` truncation): {got}",
    )

    # The OVER-capture direction: `upstream_codex_hooks` scrapes every CamelCase
    # word, so a spill into the following `impl` becomes phantom variants — and a
    # phantom is worse than a miss, because it makes a real rename look present.
    indented = (
        "mod outer {\n"
        "    pub enum HookEventName {\n"
        "        PreToolUse,\n"
        "    }\n"
        "\n"
        "    impl HookEventName {\n"
        "        fn f(&self) -> PhantomVariant { PhantomVariant }\n"
        "    }\n"
        "}\n"
    )
    got = d.upstream_codex_hooks(indented)
    check(got is not None and "PreToolUse" in got, f"the real variant is found: {got}")
    check(
        got is not None and "PhantomVariant" not in got,
        f"the body STOPS at the enum's own brace — no spill into the impl: {got}",
    )

    commented = (
        "pub enum HookEvent {\n"
        "    /// Fires like `Foo { bar }` does.\n"
        "    SessionStart,\n"
        "    SessionEnd,\n"
        "}\n"
    )
    got = d.upstream_codewhale_hooks(commented)
    check(
        got is not None and {"session_start", "session_end"} <= got,
        f"a brace in a comment does not truncate the body: {got}",
    )

    # An empty set would report every registered event as GONE.
    check(d.upstream_codewhale_hooks("struct Other {}") is None, "absent enum -> None")
    check(d.upstream_codex_hooks("struct Other {}") is None, "absent enum -> None")
    check(
        d.upstream_codewhale_hooks("pub enum HookEventName {\n    PreToolUse,\n}") is None,
        "`HookEvent` must not match `enum HookEventName`",
    )


def test_every_block_reader_strips_comments_before_counting_braces() -> None:
    """The rule two of the three readers state in their own docstrings, and the
    third re-broke: a brace inside a comment moves the bounds. The dangerous
    outcome is not the empty parse (loud) but the TRUNCATED one — the size floor
    still passes and the tail of the set silently leaves the sweep."""
    clean = 'VALID_HOOKS: Set[str] = {\n    "a_one",\n    "b_two",\n}\n'
    want = {"a_one", "b_two"}
    check(d.python_set_literal(clean, "VALID_HOOKS: Set[str] = {") == want, "clean baseline")
    for label, comment in (
        ("unmatched open", '    # returns {"action": "continue"\n'),
        ("stray close", "    # anything else } lets the turn finish\n"),
        ("both", '    # {"a": 1} and a trailing }\n'),
    ):
        poisoned = 'VALID_HOOKS: Set[str] = {\n    "a_one",\n' + comment + '    "b_two",\n}\n'
        got = d.python_set_literal(poisoned, "VALID_HOOKS: Set[str] = {")
        check(
            got == want,
            f"a comment with an {label} brace must not move the bounds: got {sorted(got)}",
        )


def test_an_undecoded_sibling_is_reported_and_a_stranger_is_not() -> None:
    """#933's direction: upstream GAINING a name we do not decode.

    Scoped to siblings of what we already handle, because "upstream grew" alone
    is not a signal — codex's `EventMsg` adds names constantly. `custom_tool_call`
    beside `function_call` is the shape that cost four tool calls a turn.
    """
    real = d.fetch
    saved = dict(d.CODEX_KNOWN_OMITTED)
    try:
        rep0 = d.Report()
        ours = d.read_our_names(rep0)
        decoded = sorted(ours.codex_response_item or ())
        check(bool(decoded), "the fragment supplies the response_item set")

        def drive(extra: list[str]) -> d.Report:
            rows = "".join(
                f'    #[serde(rename = "{n}")]\n    Pxv{i},\n'
                for i, n in enumerate(decoded + extra)
            )
            body = f"pub enum ResponseItem {{\n{rows}}}\n"

            def stub(u: str, _b: str = body) -> str:
                if u == d.CODEX_MODELS_URL:
                    return _b
                raise urllib.error.URLError("offline: not this case's document")

            d.fetch = stub
            rep = d.Report()
            d.run_checks(d.read_our_names(rep), report=rep)
            return rep

        d.CODEX_KNOWN_OMITTED.clear()
        quiet = drive([])
        check(not quiet.review, f"decoding everything upstream has stays silent: {quiet.review}")

        family = decoded[0].rsplit("_", 1)[-1]
        sibling = f"pxd_new_{family}"
        loud = drive([sibling])
        check(any(sibling in x for x in loud.review),
              f"a new `_{family}` sibling must be reported; got {loud.review}")

        stranger = drive(["pxd_unrelated_thing"])
        check(not any("pxd_unrelated" in x for x in stranger.review),
              f"an unrelated addition must NOT be reported; got {stranger.review}")

        d.CODEX_KNOWN_OMITTED[sibling] = "test"
        ledgered = drive([sibling])
        check(not any(sibling in x for x in ledgered.review),
              f"a ledgered sibling goes quiet; got {ledgered.review}")
    finally:
        d.fetch = real
        d.CODEX_KNOWN_OMITTED.clear()
        d.CODEX_KNOWN_OMITTED.update(saved)


def test_rust_comment_strip_is_not_confused_by_lifetimes() -> None:
    """The stripper's docstring named this test before the test existed.

    Tracking `'` as a string opener swallows the comment AFTER an odd number of
    lifetimes into a fake literal, re-admitting every word inside it. An EVEN
    count cannot show this — the fake span closes before the comment — so a
    two-lifetime fixture passes under both implementations and proves nothing.
    """
    odd = "fn f<'a>(x: &'a str) -> &'static u8 { /* PxGhost */ }"
    check("PxGhost" not in d.strip_rust_comments(odd),
          f"lifetimes must not open a string: {d.strip_rust_comments(odd)}")

    # `//` inside a string literal: a blind `//[^\n]*` eats the rest of the line.
    in_string = 'const U: &str = "https://x/y"; const K: &str = "PxKeep";'
    check(d.strip_rust_comments(in_string) == in_string,
          f"a `//` inside a string is not a comment: {d.strip_rust_comments(in_string)}")

    # NESTED block comments: `/\\*.*?\\*/` stops at the first `*/` and re-admits
    # `PxGhost` from inside the comment.
    nested = 'A, /* outer /* inner */ PxGhost */ B,'
    stripped = d.strip_rust_comments(nested)
    check("PxGhost" not in stripped, f"a nested comment leaks: {stripped}")
    check("A," in stripped and "B," in stripped, f"code around it survives: {stripped}")


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


def test_the_floor_is_what_we_already_handle() -> None:
    """A parse returning fewer names than we already decode is broken or degraded,
    and must file probe health rather than five ⛔ against working decoders —
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
    check(bool(callers), "the gate still has callers")
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
    check(bool(urls), "the *_URL declaration shape moved — this census reads nothing")
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


def test_codex_enum_reader_drops_field_types_and_keeps_renames() -> None:
    """`_strip_nested` is what stops a struct field's CamelCase TYPE reading as a
    variant. A phantom is invisible to the vanish census — the check is
    one-directional — so it needs a reader-level case: a phantom makes a REAL
    rename look present, which is the masked half of the same failure.
    """
    body = (
        "pub enum EventMsg {\n"
        "    TaskStarted { detail: PhantomType, n: usize },\n"
        "    ExecCommandEnd(PhantomTuple),\n"
        "    #[serde(rename = \"renamed_wire_name\")]\n"
        "    TokenCount,\n"
        "}\n"
    )
    got = d.upstream_codex_enum_types(body, "EventMsg")
    check(got is not None, "the enum parses")
    got = got or set()
    check("task_started" in got and "exec_command_end" in got, f"variants convert: {sorted(got)}")
    check("renamed_wire_name" in got, f"a rename attr is read verbatim: {sorted(got)}")
    check("phantom_type" not in got and "phantom_tuple" not in got,
          f"a field TYPE is not a variant: {sorted(got)}")


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
    """The gate #941 asks for: EVERY surface row's check, both directions, offline.

    Each document is BUILT from the set we actually depend on, so adding an event
    cannot make the test stale — and it is built in the UPSTREAM spelling, which
    is the trap that hid codewhale's arm during review (upstream declares
    `MessageSubmit`, we register `message_submit`).
    """
    real = d.fetch
    try:
        rep0 = d.Report()
        ours = d.read_our_names(rep0)
        check(not rep0.blind, f"the fragments load: {rep0.blind}")
        full: dict[str, list[str]] = {
            f: sorted(getattr(ours, f) or ()) for f, *_ in d.SURFACE_ROWS
        }

        def pascal(name: str) -> str:
            return "".join(p.title() for p in name.split("_"))

        def bare_enum(enum: str, names: list[str]) -> str:
            return f"pub enum {enum} {{\n" + "".join(f"    {n},\n" for n in names) + "}\n"

        def tagged_enum(enum: str, names: list[str]) -> str:
            # Shaped like the REAL codex enums, in the two dimensions the parsers
            # defend: a STRUCT variant (so `_enum_body` must brace-balance rather
            # than stop at the first `}`) and PascalCase idents whose snake_case
            # tags derive — the path that delivers 7/7 of `ResponseItem` and 3/3
            # of `RolloutItem` upstream, where a `rename` attr delivers almost
            # none. A unit-variant-only fixture leaves both untested.
            rows = "".join(
                f"    {pascal(n)} {{ pxd: PxdType }},\n" if i == 0 else f"    {pascal(n)},\n"
                for i, n in enumerate(names)
            )
            return f"pub enum {enum} {{\n{rows}}}\n"

        def acp_schema(tags: list[str], statuses: list[str] | None = None) -> str:
            # `x-method` present + the method literal are the ACP arm's own anchor.
            return json.dumps({
                "x-method": "session/update",
                "$defs": {
                    "SessionUpdate": {"oneOf": [
                        {"properties": {"sessionUpdate": {"const": t}}} for t in tags]},
                    "ToolCallStatus": {"oneOf": [
                        {"const": c} for c in (
                            statuses if statuses is not None else full["acp_terminal_statuses"]
                        )]},
                },
            })

        def copilot_schema(events: list[str], fields: list[str]) -> str:
            defs: dict = {f"E{i}": {"properties": {"type": {"const": e}}}
                          for i, e in enumerate(events)}
            defs["SessionEvent"] = {"anyOf": [
                {"$ref": f"#/definitions/E{i}"} for i in range(len(events))]}
            defs["Payload"] = {"properties": dict.fromkeys(fields, {})}
            return json.dumps({"definitions": defs})

        # field -> (upstream spelling, {url: document} built from the given names).
        # Two fields sharing one URL (codex/codex_event_msg, copilot/copilot_fields)
        # each render the OTHER's set in full, so a case proves its own arm rather
        # than riding its neighbour's blind.
        cases = [
            ("reasonix", str, lambda ns: {
                d.REASONIX_HOOK_URL:
                    "const (\n" + "".join(f'    {n} Event = "{n}"\n' for n in ns) + ")\n"}),
            ("codewhale", pascal, lambda ns: {
                d.CODEWHALE_HOOK_URL: bare_enum("HookEvent", ns)}),
            ("codex", str, lambda ns: {
                d.CODEX_PROTOCOL_URL: bare_enum("HookEventName", ns)
                + tagged_enum("EventMsg", full["codex_event_msg"])}),
            ("codex_event_msg", str, lambda ns: {
                d.CODEX_PROTOCOL_URL: bare_enum("HookEventName", full["codex"])
                + tagged_enum("EventMsg", ns)}),
            ("codex_response_item", str, lambda ns: {
                d.CODEX_MODELS_URL: tagged_enum("ResponseItem", ns)}),
            # The check reads the UNION of codex's two documents, so a name has to
            # leave both to count as gone. Each document keeps its own anchor.
            ("codex_escalation", str, lambda ns: {
                d.CODEX_PROTOCOL_URL: bare_enum("HookEventName", full["codex"])
                + "".join(f'const {n.upper()}: &str = "{n}";\n' for n in ns),
                d.CODEX_MODELS_URL: tagged_enum(
                    "ResponseItem", full["codex_response_item"]
                )}),
            ("codex_outers", str, lambda ns: {
                d.CODEX_ROLLOUT_ITEM_URL: tagged_enum("RolloutItem", ns)}),
            ("hermes", str, lambda ns: {
                d.HERMES_PLUGINS_URL:
                    "VALID_HOOKS: Set[str] = {\n" + "".join(f'    "{n}",\n' for n in ns) + "}\n"}),
            # Upstream GENERATES the enum from a `hook_events!` table: the
            # plain-enum regex matches the macro DEFINITION and reads `$variant`
            # placeholders as zero names, so the real document is only parsed by
            # the `rust_block_after` fall-through. A plain-enum fixture would
            # test the branch upstream does not use.
            ("grok", str, lambda ns: {
                d.GROK_HOOK_URL:
                    "macro_rules! hook_events {\n"
                    "    ($($variant:ident { $($rest:tt)* }),* $(,)?) => {\n"
                    "        pub enum HookEventName {\n"
                    "            $($variant,)*\n"
                    "        }\n"
                    "    };\n"
                    "}\n\n"
                    "hook_events! {\n"
                    + "".join(f'    {n} {{ alias: "x" }},\n' for n in ns) + "}\n"}),
            # A VALUE row: the vanish arm is a DIFFERENT port upstream, not an
            # absent one, so the generic filler drives it by declaring the fillers
            # instead of ours.
            ("openclaw_gateway_port", str, lambda ns: {
                d.OPENCLAW_PATHS_URL: f"export const DEFAULT_GATEWAY_PORT = {ns[0]};\n"}),
            ("openclaw", str, lambda ns: {
                d.OPENCLAW_HOOK_TYPES_URL:
                    "export type PluginHookName =\n" + "".join(f'  | "{n}"\n' for n in ns)}),
            ("opencode", str, lambda ns: dict.fromkeys(
                d.OPENCODE_EVENT_URLS,
                "export const Event = {\n"
                + "".join(f'  Pxe{i}: {{ type: "{n}" }},\n' for i, n in enumerate(ns)) + "}\n")),
            # A VALUE row: upstream declares the const ONCE, so the document
            # carries one declaration and a vanish is a different value in it.
            ("grok_xai_method", str, lambda ns: {
                d.GROK_SESSION_STORAGE_URL:
                    f'const XAI_SESSION_UPDATE_METHOD: &\'static str = "{ns[0]}";\n'}),
            ("grok_xai_tags", pascal, lambda ns: {
                d.GROK_NOTIFICATION_URL:
                    "pub enum SessionUpdate {\n" + "".join(f"    {n},\n" for n in ns) + "}\n"}),
            # Split across two documents like the real vocabulary: `ask` is only
            # in ask.ts, so a case serving one file could not fire for it.
            ("omp_message_vocab", str, lambda ns: {
                d.OMP_AI_TYPES_URL:
                    "export type Block = {\n"
                    + "".join(f'  | "{n}"\n' for n in ns if n != "ask") + "};\n",
                d.OMP_ASK_URL:
                    "export class AskTool {\n"
                    + "".join(f'  name = "{n}";\n' for n in ns if n == "ask") + "}\n"}),
            ("omp_exit_marker", str, lambda ns: {
                d.OMP_EXIT_DIAG_URL: "".join(
                    f'const SESSION_EXIT_CUSTOM_TYPE = "{n}";\n' for n in ns)}),
            ("omp", str, lambda ns: {
                d.OMP_SESSION_ENTRIES_URL:
                    "export type SessionEntry =\n"
                    + "".join(f"\t| Pxe{i}Entry\n" for i in range(len(ns)))
                    + "".join(f'export interface Pxe{i}Entry {{ type: "{n}" }}\n'
                              for i, n in enumerate(ns))}),
            ("cursor", str, lambda ns: {
                d.CURSOR_HOOKS_URL: "### Hook events\n\n" + "".join(f"#### {n}\n\n" for n in ns)}),
            ("kimi", str, lambda ns: {
                d.KIMI_HOOKS_URL: "hook_event_name\n\n" + "".join(f"| `{n}` | x |\n" for n in ns)}),
            ("cc", str, lambda ns: {
                d.CC_HOOKS_URL: "# Hooks reference\n\n| Event | When it fires |\n|---|---|\n"
                + "".join(f"| `{n}` | x |\n" for n in ns)}),
            ("dispatch_names", str, lambda ns: {
                d.CC_TOOLS_URL: "# Tools reference\n\n" + "".join(f"| `{n}` | x |\n" for n in ns)}),
            ("acp_decoded_tags", str, lambda ns: dict.fromkeys(
                (d.ACP_V1_SCHEMA_URL, d.ACP_V1_SCHEMA_UNSTABLE_URL), acp_schema(ns))),
            ("acp_terminal_statuses", str, lambda ns: dict.fromkeys(
                (d.ACP_V1_SCHEMA_URL, d.ACP_V1_SCHEMA_UNSTABLE_URL),
                acp_schema(full["acp_decoded_tags"], ns))),
            ("copilot", str, lambda ns: {
                d.COPILOT_SCHEMA_URL: copilot_schema(ns, full["copilot_fields"])}),
            ("copilot_fields", str, lambda ns: {
                d.COPILOT_SCHEMA_URL: copilot_schema(full["copilot"], ns)}),
        ]
        covered = {c[0] for c in cases}
        rows = {f for f, *_ in d.SURFACE_ROWS}
        check(covered == rows, f"every surface row needs a case; differ: {covered ^ rows}")

        def drive(docs: dict[str, str]) -> d.Report:
            # OFFLINE: every URL this case does not serve raises rather than going
            # to the network. `else real(u)` made `just lint` issue 208 live
            # requests per run — and `lint` joins its jobs with `wait`, so a
            # blackholing network would hang preflight and pre-push at
            # 30s/request (justfile:1327).
            def stub(u: str, _d: dict[str, str] = docs) -> str:
                if u in _d:
                    return _d[u]
                raise urllib.error.URLError("offline: not this case's document")

            d.fetch = stub
            rep = d.Report()
            d.run_checks(d.read_our_names(rep), report=rep)
            return rep

        for field, spell, build in cases:
            names = full[field]
            check(bool(names), f"{field}: the fragment supplies a set")
            # The tolerated alias can never fire, so it must not be the victim.
            pool = [n for n in names
                    if field != "opencode" or n not in d.OPENCODE_TOLERATED]
            # `dispatch_names` is an ANY-of check — one surviving documented name
            # clears it — so its vanish arm has to take them all.
            victims = pool if field == "dispatch_names" else pool[:1]
            # The believability floor refuses a parse smaller than what we handle,
            # so the vanish arm ADDS names as it drops one — shaped like the name
            # it extends, since a filler this parser's character class rejects
            # shrinks the set below the floor and SKIPS the check, proving nothing.
            # `_` and not case is the axis: cursor's headings take `[a-z][A-Za-z]+`
            # and its lone all-lowercase `stop` made an `islower()` test pick the
            # snake_case suffix, silently skipping the whole cursor arm.
            stem = spell(names[-1])
            if stem.isdigit():
                # A numeric VALUE row: a `Pxd` suffix still leaves our digits at
                # the front, where the reader's `(\d+)` finds them and the arm
                # reads as unchanged. Extra digits are what make it a real vanish.
                sfx = ("0", "00")
            elif "_" in stem:
                sfx = ("_pxd", "_pxdb")
            else:
                sfx = ("Pxd", "Pxdb")
            kept = [spell(n) for n in names if n not in victims]

            intact = drive(build([spell(n) for n in names]))
            check(not intact.breaking,
                  f"{field}: an intact document must stay silent; got {intact.breaking}")
            rep = drive(build(kept + [stem + s for s in sfx]))
            check(any(victims[0] in x for x in rep.breaking),
                  f"{field}: a vanished `{victims[0]}` must fire; got {rep.breaking} "
                  f"(skipped as probe health? {rep.blind})")
    finally:
        d.fetch = real


def main() -> int:
    # Derived, not hand-listed: a test missing from the runner is inert while
    # the suite still prints "all checks passed". Derivation cannot see the OTHER
    # half — a second `def` of the same name replaces the binding — so that is an
    # AST scan, and it runs HERE rather than as a test because a test detecting
    # duplicate names is itself shadowable by a duplicate.
    # `ast.walk`, not `.body`: a top-level-only scan misses an `async def`, a
    # nested `def`, and a rebind — each of which shadows a real test while the
    # suite prints "all checks passed".
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
        # A raise past a recorded `check()` would otherwise replace the whole
        # FAILS list — including the line that already explains the failure —
        # with a traceback from the code that ran on after it.
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
