#!/usr/bin/env python3
"""Self-test for check_upstream_drift.py — the drift watcher had ZERO tests, yet
a regex-parser regression = a SILENT monitor death (it returns empty / raises and
the weekly job either alarms on junk or watches nothing). This pins:

  1. `try_fetch` failure classification — the PR that added it fixed the
     `HTTPError ⊂ URLError ⊂ FETCH_ERRORS` swallow that bucketed a permanent 404
     as transient. A 404/410/451 MUST be breaking; 5xx/429/timeouts transient.
  2. The `read_*_events` source parsers still find a non-empty, well-shaped set
     (catches "the regex broke" / "it grabbed the wrong block").
  3. The `upstream_*` parsers extract names from a representative snippet.
  4. The CC doc-marker DETECTION (`cc_doc_marker_findings`) fires in BOTH
     directions — depended markers on VANISH, surface markers on APPEARANCE
     (the #541 burn-tier watches ship with teeth, not just parsers).
  5. The anchor gate, in BOTH directions, for EVERY document in `ANCHORS`: a
     pure upstream refactor yields probe health, and an intact anchor still
     hands the body to the caller's sweep. That a real rename STILL yields drift
     is proven end-to-end for CodeWhale (`test_793_…`), the #793 source. The
     refactor direction is the one that had no test and is exactly what #793
     escaped through — the watcher reported three working CodeWhale env vars as
     renamed because a stale pin still fetched 200.
     NB the samples are hand-written to satisfy their patterns, so this pair
     proves the GATE works — never that any anchor is CORRECT against the live
     document. A typo'd anchor passes here and shows up as a permanent weekly
     probe-health line (self-reporting, which is why that is acceptable).
  6. The report keeps the two dispositions under separate headings, so a
     "we could not verify" line can never be read as "upstream changed".

Run: `python3 scripts/check_upstream_drift_selftest.py` (exit 0 = pass).
No pytest dependency on purpose — the repo has no Python test harness.
"""

from __future__ import annotations

import io
import pathlib
import re
import sys
import urllib.error

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
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
        # Permanent HTTP → PROBE HEALTH (our pin is wrong, so the watch is dark
        # for this source). Deliberately not `breaking`: a 404 says nothing about
        # whether upstream renamed anything, only that we are looking in the
        # wrong place.
        for code in (404, 410, 451):
            d.fetch = lambda _u, c=code: (_ for _ in ()).throw(_http_error(c))
            bl: list[str] = []
            er: list[str] = []
            out = d.try_fetch("https://x/y", "T", bl, er)
            check(out is None, f"{code}: returns None")
            check(len(bl) == 1 and not er, f"{code}: -> blind (got blind={bl} errors={er})")
            check(str(code) in (bl[0] if bl else ""), f"{code}: message names the status")

        # Transient HTTP (server/throttle) → errors, NOT probe health.
        for code in (500, 502, 503, 429, 403):
            d.fetch = lambda _u, c=code: (_ for _ in ()).throw(_http_error(c))
            bl, er = [], []
            d.try_fetch("https://x/y", "T", bl, er)
            check(not bl and len(er) == 1, f"{code}: -> transient (got blind={bl} errors={er})")

        # Network-layer failure → transient.
        d.fetch = lambda _u: (_ for _ in ()).throw(urllib.error.URLError("conn refused"))
        bl, er = [], []
        d.try_fetch("https://x/y", "T", bl, er)
        check(not bl and len(er) == 1, f"URLError -> transient (got blind={bl} errors={er})")

        # Success → returns the body, no buckets touched.
        d.fetch = lambda _u: "BODY"
        bl, er = [], []
        out = d.try_fetch("https://x/y", "T", bl, er)
        check(out == "BODY" and not bl and not er, "success returns body, no buckets")
    finally:
        d.fetch = real


def test_source_parsers_find_nonempty_well_shaped_sets() -> None:
    # (reader, a shape regex every member must match, floor) — non-empty + shape
    # catches a broken regex / wrong-block grab WITHOUT coupling to the exact event
    # set (which legitimately grows as sources are added). floor is 2 for real
    # event sets; the dispatch-tool name set is a legitimate SINGLETON since CC
    # dropped the `Task` alias in 0.12.0 (only `Agent` remains — see the subagent
    # sharp edge in crates/pixtuoid-core/CLAUDE.md), so it floors at 1.
    cases = [
        (d.read_codex_events, r"^[A-Za-z]\w+$", 2),
        (d.read_codex_rollout_outers, r"^[a-z][a-z_]*$", 4),
        (d.read_cc_events, r"^[A-Za-z]\w+$", 2),
        (d.read_dispatch_names, r"^[A-Za-z]\w+$", 1),
        (d.read_reasonix_events, r"^[A-Za-z]\w+$", 2),
        (d.read_codewhale_events, r"^[a-z][a-z_]*$", 2),
        (d.read_openclaw_events, r"^[a-z][a-z_]*$", 2),
        (d.read_opencode_events, r"^[a-z][a-z0-9._]*$", 2),
        (d.read_copilot_events, r"^[a-z][a-z0-9._]*$", 2),
        (d.read_copilot_namespaces, r"^[a-z][a-z_]*$", 20),
        (d.read_omp_entry_types, r"^[a-z]+$", 3),
        (d.read_omp_known_types, r"^[a-z][a-z_]*$", 10),
        (d.read_acp_tags, r"^[a-z][a-z_]*$", 10),
        (d.read_cursor_events, r"^[a-zA-Z]\w+$", 2),
        (d.read_hermes_events, r"^[a-z][a-z_]*$", 2),
        (d.read_kimi_events, r"^[A-Za-z]\w+$", 2),
    ]
    for reader, shape, floor in cases:
        name = reader.__name__
        got = reader()
        check(isinstance(got, set) and len(got) >= floor, f"{name}: non-empty (>={floor}), got {got!r}")
        bad = [m for m in got if not re.match(shape, m)]
        check(not bad, f"{name}: members match {shape}; offenders={bad}")

    # read_codex_rollout_types returns a (event_msg, response_item) TUPLE, not a
    # set, so it rides its own check: both halves non-empty + snake_case, and the
    # known task_started / function_call present (a decoder refactor that drops
    # the ("event_msg"|"response_item", …) arms would empty these → RuntimeError).
    ev, ri = d.read_codex_rollout_types()
    check(len(ev) >= 2 and len(ri) >= 2, f"read_codex_rollout_types non-empty: ev={ev!r} ri={ri!r}")
    check("task_started" in ev, f"codex event_msg has task_started: {ev!r}")
    check("function_call" in ri, f"codex response_item has function_call: {ri!r}")
    offenders = [m for m in (ev | ri) if not re.match(r"^[a-z][a-z_]*$", m)]
    check(not offenders, f"codex rollout members are snake_case; offenders={offenders}")

    # `openclaw_plugin_default_port` returns a single VALUE, not a set, so it rides
    # its own check (the read_codex_rollout_types precedent). It exists because the
    # port literal is COPIED into the plugin (un-importable from OpenClaw's state
    # dir) and the live comparison is PR-gated off, so renaming the const would leave
    # the weekly check comparing nothing. THIS is what notices — and the workflow
    # lists the template in its PR `paths`, so it fires on the renaming PR itself.
    port = d.openclaw_plugin_default_port()
    check(
        port is not None and port.isdigit(),
        f"openclaw_plugin_default_port reads a numeric literal from the plugin, got {port!r}",
    )


def test_upstream_parsers_extract_from_a_snippet() -> None:
    # Codex HookEventName enum snippet.
    codex = 'pub enum HookEventName {\n    SessionStart,\n    PreToolUse,\n    Stop,\n}'
    up = d.upstream_codex_hooks(codex)
    check(up is not None and {"SessionStart", "PreToolUse"} <= up, f"codex enum parse: {up}")

    # Copilot schema: definitions[*].properties.type.const.
    schema = (
        '{"definitions":{"A":{"properties":{"type":{"const":"session.start"}}},'
        '"B":{"properties":{"type":{"const":"tool.execution_start"}}}}}'
    )
    up = d.upstream_copilot_events(schema)
    check(up is not None and {"session.start", "tool.execution_start"} <= up, f"copilot schema parse: {up}")

    # A malformed schema → None (signals "restructured", handled as breaking upstream).
    check(d.upstream_copilot_events("not json") is None, "copilot bad json -> None")

    # Copilot NAMESPACES — scoped to the SessionEvent.anyOf union. A nested-content
    # def that shares the `type.const` shape (`Blob`→"blob") must NOT leak a phantom
    # family: the result is EXACTLY {session, tool}, proving the anyOf scoping.
    ns_schema = (
        '{"definitions":{'
        '"SessionEvent":{"anyOf":[{"$ref":"#/definitions/SessStart"},{"$ref":"#/definitions/ToolStart"}]},'
        '"SessStart":{"properties":{"type":{"const":"session.start"}}},'
        '"ToolStart":{"properties":{"type":{"const":"tool.execution_start"}}},'
        '"Blob":{"properties":{"type":{"const":"blob"}}}}}'
    )
    up = d.upstream_copilot_namespaces(ns_schema)
    check(up == {"session", "tool"}, f"copilot namespaces scoped to SessionEvent union (no Blob leak): {up}")
    check(d.upstream_copilot_namespaces("not json") is None, "copilot namespaces bad json -> None")
    check(d.upstream_copilot_namespaces('{"definitions":{}}') is None, "copilot namespaces no union -> None")

    # omp entry types — direct `type: "x"` literals PLUS `type: typeof CONST`
    # refs resolved via a `CONST = "x"` binding (the title / title_change slots).
    omp_ts = (
        'const SESSION_TITLE_SLOT_ENTRY_TYPE = "title";\n'
        'interface Msg { type: "message"; }\n'
        'interface Slot { type: typeof SESSION_TITLE_SLOT_ENTRY_TYPE; }\n'
    )
    up = d.upstream_omp_entry_types(omp_ts)
    check(up == {"message", "title"}, f"omp entry types (literal + typeof-resolved): {up}")
    check(d.upstream_omp_entry_types("no types here") is None, "omp entry types none -> None")

    # ACP v1 SessionUpdate tags — each `$defs.SessionUpdate.oneOf` member's inline
    # `sessionUpdate.const`, with a `$ref` member resolved.
    acp_schema = (
        '{"$defs":{"SessionUpdate":{"oneOf":['
        '{"properties":{"sessionUpdate":{"const":"tool_call"}}},'
        '{"properties":{"sessionUpdate":{"const":"user_message_chunk"}}},'
        '{"$ref":"#/$defs/PlanUpd"}]},'
        '"PlanUpd":{"properties":{"sessionUpdate":{"const":"plan_update"}}}}}'
    )
    up = d.upstream_acp_session_update_tags(acp_schema)
    check(
        up == {"tool_call", "user_message_chunk", "plan_update"},
        f"acp tags (inline const + $ref resolved): {up}",
    )
    check(d.upstream_acp_session_update_tags("not json") is None, "acp bad json -> None")
    check(d.upstream_acp_session_update_tags('{"$defs":{}}') is None, "acp no SessionUpdate -> None")

    # Copilot FIELD-NAME union — every `properties` key at ANY depth (envelope
    # `agentId` AND the nested `data.properties` `toolCallId`).
    copilot_fields = '{"definitions":{"A":{"properties":{"agentId":{},"data":{"properties":{"toolCallId":{}}}}}}}'
    up = d.upstream_copilot_field_names(copilot_fields)
    check(up is not None and {"agentId", "toolCallId"} <= up, f"copilot field union (recursive): {up}")
    check(d.upstream_copilot_field_names("not json") is None, "copilot fields bad json -> None")

    # CC hook-event summary table — the MOST complex parser (anchors to the
    # "| Event |" header + separator, extracts the backtick-quoted first cell).
    # A wrong-but-non-None match here would silently miss a renamed event, so pin
    # both a real table and the no-table -> None case.
    cc_md = (
        "| Event | When it fires |\n"
        "|---|---|\n"
        "| `PreToolUse` | before a tool call |\n"
        "| `PostToolUse` | after a tool call |\n"
    )
    up = d.upstream_cc_hook_events(cc_md)
    check(up is not None and {"PreToolUse", "PostToolUse"} <= up, f"cc table parse: {up}")
    check(d.upstream_cc_hook_events("no table here") is None, "cc no table -> None")

    # Reasonix Go consts: `Ident Event = "Wire"`.
    reasonix_go = 'const (\n\tPreToolUse Event = "PreToolUse"\n\tStop Event = "Stop"\n)'
    up = d.upstream_reasonix_hooks(reasonix_go)
    check(up is not None and {"PreToolUse", "Stop"} <= up, f"reasonix consts parse: {up}")
    check(d.upstream_reasonix_hooks("no consts here") is None, "reasonix none -> None")

    # CodeWhale Rust enum → snake_case wire names (serde rename_all = snake_case).
    codewhale_rs = "pub enum HookEvent {\n    SessionStart,\n    PreToolUse,\n}"
    up = d.upstream_codewhale_hooks(codewhale_rs)
    check(up is not None and {"session_start", "pre_tool_use"} <= up, f"codewhale enum parse: {up}")
    check(d.upstream_codewhale_hooks("no enum here") is None, "codewhale none -> None")

    # grok HookEventName — BOTH declaration shapes, because upstream moved from a
    # plain enum to a `hook_events!` macro TABLE and the enum-only regex then read
    # empty, reporting the 15 unchanged variants as "not found at the pinned path"
    # (#793). The macro-def arm is the trap: its body carries a literal
    # `pub enum HookEventName {` with `$variant` placeholders and no real variants,
    # so a parser must fall THROUGH it to the invocation rather than stop there.
    grok_plain = "pub enum HookEventName {\n    SessionStart,\n    PreToolUse,\n}"
    up = d.upstream_grok_hooks(grok_plain)
    check(up is not None and {"SessionStart", "PreToolUse"} <= up, f"grok plain enum parse: {up}")
    grok_macro = (
        "macro_rules! hook_events {\n"
        "    ($($variant:ident { display: $d:literal, }),* $(,)?) => {\n"
        "        pub enum HookEventName {\n"
        "            $($variant),*\n"
        "        }\n"
        "    };\n"
        "}\n"
        "\n"
        "hook_events! {\n"
        "    SessionStart {\n"
        '        display: "session_start",\n'
        '        aliases: ["SessionStart", "session_start"],\n'
        "        traits: (Observe, Tested, true),\n"
        "    },\n"
        "    /// A doc comment on a row must not be read as a variant.\n"
        "    SubagentEnd {\n"
        '        display: "subagent_stop",\n'
        '        aliases: ["SubagentEnd"],\n'
        "        traits: (Stop, Tested, true),\n"
        "    },\n"
        "}\n"
    )
    up = d.upstream_grok_hooks(grok_macro)
    check(
        up is not None and up == {"SessionStart", "SubagentEnd"},
        f"grok macro-table parse (exact, no alias/trait leakage): {up}",
    )
    # A macro DEFINITION with no invocation must read None (a real "it moved"),
    # never the placeholder-only enum body.
    grok_def_only = grok_macro.split("hook_events! {")[0]
    check(d.upstream_grok_hooks(grok_def_only) is None, "grok macro def without invocation -> None")
    check(d.upstream_grok_hooks("no enum here") is None, "grok none -> None")

    # Codex EventMsg / ResponseItem: #[serde(tag="type", rename_all="snake_case")]
    # enums. snake_case(variant) + explicit rename/alias, with nested tuple/struct
    # bodies stripped so a CamelCase field TYPE isn't mistaken for a variant.
    codex_enum = (
        "pub enum EventMsg {\n"
        '    #[serde(rename = "task_started", alias = "turn_started")]\n'
        "    TurnStarted(TurnStartedEvent),\n"
        "    ExecCommandEnd(ExecCommandEndEvent),\n"
        "    SessionConfigured { model: ModelInfo, cwd: PathBuf },\n"
        "    Other,\n"
        "}"
    )
    up = d.upstream_codex_enum_types(codex_enum, "EventMsg")
    check(
        up is not None and {"task_started", "turn_started", "exec_command_end"} <= up,
        f"codex EventMsg parse (rename+alias+snake): {up}",
    )
    # A struct-field TYPE (ModelInfo/PathBuf) must NOT leak in as a variant.
    check(up is not None and "model_info" not in up and "path_buf" not in up, f"codex struct-field type leaked: {up}")
    check(d.upstream_codex_enum_types("no enum here", "EventMsg") is None, "codex enum none -> None")

    # Codex ResponseItem::FunctionCall inline-struct FIELD extraction.
    fc_struct = "FunctionCall {\n    name: String,\n    arguments: String,\n    call_id: String,\n}"
    up = d.codex_function_call_fields(fc_struct)
    check(up is not None and {"name", "arguments"} <= up, f"codex FunctionCall fields: {up}")
    # A tuple variant (external struct) → None = GRACEFUL SKIP, not a false alarm.
    check(
        d.codex_function_call_fields("FunctionCall(FunctionCallItem),") is None,
        "codex FunctionCall tuple variant -> None (graceful skip, not an alarm)",
    )
    # TurnContextItem (burn tier, #541): field idents extracted incl. the two
    # the decoder depends on; a moved/renamed struct → None (caller alarms).
    tc_struct = (
        "pub struct TurnContextItem {\n"
        "    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n"
        "    pub turn_id: Option<String>,\n"
        "    pub cwd: AbsolutePathBuf,\n"
        "    pub model: String,\n"
        "    #[serde(skip_serializing_if = \"Option::is_none\")]\n"
        "    pub effort: Option<ReasoningEffortConfig>,\n"
        "}\n"
    )
    up = d.codex_turn_context_fields(tc_struct)
    check(up is not None and {"model", "effort"} <= up, f"codex TurnContextItem fields: {up}")
    check(
        d.codex_turn_context_fields("pub enum RolloutItem { TurnContext(TurnContextItem) }") is None
        or "model" not in (d.codex_turn_context_fields("pub enum RolloutItem { TurnContext(TurnContextItem) }") or set()),
        "codex TurnContextItem absent -> None (caller alarms)",
    )


def test_cc_doc_marker_detection_fires_both_directions() -> None:
    # A doc carrying every depended marker and no surface marker is quiet.
    quiet = "\n".join(d.CC_DEPENDED_DOC_MARKERS)
    got = d.cc_doc_marker_findings(quiet)
    check(got == [], f"quiet doc -> no findings: {got!r}")

    # VANISH direction: drop the effort surface → exactly its finding.
    missing = "\n".join(m for m in d.CC_DEPENDED_DOC_MARKERS if m != "CLAUDE_EFFORT")
    got = d.cc_doc_marker_findings(missing)
    check(len(got) == 1 and "CLAUDE_EFFORT" in got[0], f"vanish fires: {got!r}")

    # APPEARANCE direction: an ultra marker shows up in the docs → its finding.
    got = d.cc_doc_marker_findings(quiet + "\nultra_effort_exit\n")
    check(len(got) == 1 and "ultra_effort_exit" in got[0], f"appearance fires: {got!r}")


def test_const_array_parser_ignores_words_quoted_inside_comments() -> None:
    """A quoted word in a comment must NOT be read as a registered event.

    The shape assertions above cannot catch this: a phantom scraped out of a
    comment is an ordinary-looking word that passes every shape regex. It shipped
    for real — a WHY comment inside CODEX_EVENTS mentioning the SessionEnd
    payload's `reason const "other"` made the watcher report a phantom breaking
    drift, auto-file an issue, and fail the run.
    """
    src = '''
const DEMO_EVENTS: &[&str] = &[
    "SessionStart",
    // upstream's payload carries a reason const "other"; the field is
    // `hook_event_name:"SessionEnd"` — neither is a registered event here
    /* a block comment naming "PreToolUse" is not a registration either */
    "SessionEnd",
];
'''
    got = d.parse_rust_const_str_array(src, "DEMO_EVENTS")
    check(got == {"SessionStart", "SessionEnd"}, f"comments excluded: {got!r}")

    # And an absent const is reported as such, not as an empty set — an empty
    # set would read as "nothing registered" and silence every check downstream.
    check(
        d.parse_rust_const_str_array(src, "NOPE_EVENTS") is None,
        "a missing const returns None, never an empty set",
    )


def test_parser_never_drops_a_real_event() -> None:
    """The failing-OPEN direction, which is the worse one.

    A phantom is loud: the watcher alarms on a name upstream does not have. A
    DROPPED registration is silent — that name simply stops being checked, and
    nothing says so. Both regex formulations of this parser had that bug, so
    each case below is a construct that made one of them swallow a real entry.
    """
    cases = [
        # `//` inside a string literal: a line-comment strip runs to end of line
        # and takes every later entry with it.
        ('"SessionStart", "a//b", "SessionEnd"', {"SessionStart", "SessionEnd"}),
        ('"Alpha", "http://x", "Charlie"', {"Alpha", "Charlie"}),
        # Nested block comments are legal Rust; a non-greedy `/\\*.*?\\*/` closes
        # at the FIRST `*/` and re-admits words from the comment's tail.
        ('"Alpha", /* outer /* inner */ names "Phantom" */ "B"', {"Alpha", "B"}),
        # A `/*` that only ever appears inside a line comment must not open a
        # block that swallows the entries after it.
        ('"Alpha",\n // uses /* as a marker\n "Beta", /* real */', {"Alpha", "Beta"}),
        # Escaped quotes must not end the string early.
        (r'"Alpha", "say \"Beta\"", "Gamma"', {"Alpha", "Gamma"}),
        # A URL in a COMMENT is the benign twin of case 2 — still excluded.
        ('"Alpha",\n // see https://x "Notification"\n "Beta"', {"Alpha", "Beta"}),
    ]
    for body, want in cases:
        got = d.parse_rust_const_str_array(f"const E: &[&str] = &[{body}];", "E")
        check(got == want, f"no real event dropped from `{body}`: {got!r} != {want!r}")


def test_block_scrape_is_bounded_to_the_decoder() -> None:
    """A scrape bounded to one block must not see code that follows it.

    `read_codex_rollout_types` used to scan the WHOLE of source/codex.rs, so a
    `#[cfg(test)] mod tests` constructing the same tuple shape would leak a type
    the decoder does not depend on — and a phantom makes the watcher alarm on a
    name upstream never had to have.

    This exists because the sibling comment-safe parser shipped exactly such a
    case one commit earlier and this bounding fix did not: the negative control
    was RUN and then left out of the suite, which is the failure this whole
    branch is about.
    """
    src = """
fn decode(v: Value) -> Vec<Event> {
    let out = match (outer, inner) {
        ("event_msg", "task_started") => vec![start()],
        ("response_item", "function_call") => { let f = |x| { x }; vec![f(call())] }
        _ => vec![],
    };
    out
}

#[cfg(test)]
mod tests {
    fn planted() { let _ = ("event_msg", "PHANTOM"); }
}
"""
    block = d.rust_block_after(d.strip_rust_comments(src), r"match \(outer, inner\)")
    check(block is not None, "the anchor's block is found")
    got = set(re.findall(r'\(\s*"event_msg"\s*,\s*"(\w+)"\s*\)', block or ""))
    # Nested braces from the closure/vec! must not close the block early, and the
    # planted test tuple after it must not be visible.
    check(got == {"task_started"}, f"bounded to the decoder's arms: {got!r}")
    check(
        "function_call" in (block or ""),
        "the nested-brace arm is INSIDE the block (it did not close early)",
    )

    # A missing anchor is None, so the caller raises loudly rather than silently
    # scraping nothing — a decoder refactor must break the watcher, not blind it.
    check(
        d.rust_block_after("fn unrelated() { }", r"match \(outer, inner\)") is None,
        "a missing anchor returns None, never an empty block",
    )


def test_every_const_array_reader_uses_the_shared_parser() -> None:
    """No reader may hand-roll the scrape the shared parser exists to own.

    This is the mechanical form of the lesson, not a second copy of it: the
    original migration was a HAND-LISTED sweep of nine readers and it missed one
    (`read_kimi_events`), which is the N-1-of-N class this repo's review prompt
    calls its most-recurrent escape. A checklist cannot enforce itself; this can.
    """
    src = (pathlib.Path(__file__).parent / "check_upstream_drift.py").read_text()
    offenders = [
        ln.strip()
        for ln in src.splitlines()
        if 'findall(r\'"(\\w+)"\'' in ln and "strip_rust_comments" not in ln
    ]
    check(
        not offenders,
        "every `\"(\\\\w+)\"` scrape must route through strip_rust_comments; "
        f"unrouted: {offenders!r}",
    )


# One snippet per anchored document that MUST satisfy its anchor. Test data, and
# a record of the shape each anchor expects. A new ANCHORS entry with no sample
# here fails `test_anchor_gate_fires_in_both_directions` — the N-of-N tooth, so
# the sweep below can't quietly cover 15 of 16 documents.
ANCHOR_SAMPLES: dict[str, str] = {
    d.CODEWHALE_EXECUTOR_URL: "pub fn to_env_vars(&self) -> HashMap<String, String> {",
    d.OPENCODE_EVENT_URLS[0]: "\nexport const Event = {\n  Created,\n}",
    d.OPENCODE_EVENT_URLS[1]: "\nexport const Event = {\n  Asked,\n}",
    d.GROK_HOOK_URL: "pub struct HookEventEnvelope {\n    pub cwd: String,\n}",
    d.GROK_NOTIFICATION_URL: "pub enum SessionUpdate {\n    SubagentSpawned,\n}",
    d.GROK_ACTIVE_SESSIONS_URL: "pub struct ActiveSession {\n    pub pid: u32,\n}",
    d.OMP_SESSION_ENTRIES_URL: "export type SessionEntry = MessageEntry | CustomEntry;",
    d.OMP_EXIT_DIAG_URL: 'export const SESSION_EXIT_CUSTOM_TYPE = "session_exit";',
    d.OMP_AI_TYPES_URL: "export type Message = UserMessage | AssistantMessage;",
    d.OMP_ASK_URL: "export class AskTool extends Tool {}",
    d.CURSOR_HOOKS_URL: '"hook_event_name": "beforeShellExecution"',
    d.OPENCLAW_HOOK_TYPES_URL: 'export type PluginHookName =\n  | "gateway_start"',
    d.HERMES_HOOK_URL: "_DEFAULT_PAYLOADS = {\n    'on_session_start': {},\n}",
    d.HERMES_SHELL_HOOK_URL: "def _serialize_payload(event: str) -> str:",
    d.KIMI_HOOKS_URL: '"hook_event_name": "PreToolUse"',
    d.CC_TOOLS_URL: "\n# Tools reference\n",
    d.CC_HOOKS_URL: "\n# Hooks reference\n",
}

# A document that satisfies NO anchor — the "upstream reorganized this file"
# stand-in. Deliberately prose-shaped: a re-export facade or a restructured docs
# page is exactly this, content that fetches fine and owns nothing.
_UNANCHORED = "mod config;\nmod executor;\npub use config::*;\n"


def test_anchor_gate_fires_in_both_directions() -> None:
    """Every anchored document, both ways.

    The direction nobody writes is the second one, and it is precisely what #793
    needed: a PURE UPSTREAM REFACTOR must produce probe-health, never drift. The
    watcher reported three working CodeWhale env vars as renamed because the
    stale pin still returned 200 and an unanchored sweep read the facade's
    silence as absence.

    Table-driven over `ANCHORS` itself so the pair cannot cover N-1 of N.
    """
    real = d.fetch
    try:
        # A floor, because everything below iterates `ANCHORS`: an emptied table
        # would run zero loop bodies and this whole test would pass VACUOUSLY
        # while the watch swept nothing. The count only has to move when a
        # document is added or dropped, which is exactly when a human should
        # look at this test.
        check(len(d.ANCHORS) >= 17, f"ANCHORS covers every swept document, got {len(d.ANCHORS)}")
        missing_samples = sorted(set(d.ANCHORS) - set(ANCHOR_SAMPLES))
        check(not missing_samples, f"every ANCHORS entry needs a sample: {missing_samples}")

        for url, anchor in sorted(d.ANCHORS.items()):
            served: list[str] = []

            # (a) anchor ABSENT (a refactor moved the declaration away) -> the
            #     document is not swept at all, and the finding is probe health.
            d.fetch = lambda u, _s=served: (_s.append(u), _UNANCHORED)[1]
            blind: list[str] = []
            errors: list[str] = []
            out = d.fetch_anchored(url, "T", blind, errors)
            check(served == [url], f"{anchor.owns}: the stubbed fetch actually ran")
            check(out is None, f"{anchor.owns}: no anchor -> the sweep is skipped")
            check(len(blind) == 1 and not errors, f"{anchor.owns}: -> blind, got {blind}")
            # Indexing is guarded, not assumed: a regressed gate leaves `blind`
            # empty and this test must REPORT that, not abort the suite before
            # the #793 regression test below ever runs.
            check(
                "NOT evidence that upstream changed" in (blind[0] if blind else ""),
                f"{anchor.owns}: the line disclaims upstream causation, got {blind!r}",
            )

            # (b) anchor PRESENT -> the body comes back so the caller's presence
            #     sweep can run and report a REAL rename.
            sample = ANCHOR_SAMPLES.get(url, "")
            d.fetch = lambda _u, _s=sample: _s
            blind, errors = [], []
            out = d.fetch_anchored(url, "T", blind, errors)
            check(
                out == sample and not blind and not errors,
                f"{anchor.owns}: anchor present -> body returned, got blind={blind}",
            )
    finally:
        d.fetch = real


def test_793_stale_pin_reads_as_probe_health_not_three_renames() -> None:
    """The #793 regression, end to end through `run_checks`, both directions.

    CodeWhale split `crates/tui/src/hooks.rs` into a module directory. The old
    path kept returning 200 as a `mod`/`pub use` facade, so the fetch succeeded
    and all three `DEEPSEEK_*` names were absent — reported as three upstream
    renames under "decoder will silently drop events". Acting on it would have
    renamed three working env vars.
    """
    real = d.fetch
    config_rs = "pub enum HookEvent {\n    SessionStart,\n    ToolCallBefore,\n}"
    facade = "mod config;\nmod executor;\npub use config::HookEvent;\n"
    executor = (
        "impl HookContext {\n"
        "    pub fn to_env_vars(&self) -> HashMap<String, String> {\n"
        '        env.insert("DEEPSEEK_WORKSPACE".to_string(), ws);\n'
        '        env.insert("DEEPSEEK_TOOL_NAME".to_string(), name);\n'
        '        env.insert("DEEPSEEK_TOOL_ARGS".to_string(), args);\n'
        "    }\n"
        "}\n"
    )

    def drive(executor_body: str) -> tuple[list[str], list[str], list[str]]:
        served: list[str] = []

        def stub(url: str) -> str:
            served.append(url)
            if url == d.CODEWHALE_HOOK_URL:
                return config_rs
            if url == d.CODEWHALE_EXECUTOR_URL:
                return executor_body
            raise urllib.error.URLError("not stubbed")  # -> transient, ignored below

        d.fetch = stub
        breaking: list[str] = []
        review: list[str] = []
        blind: list[str] = []
        errors: list[str] = []
        d.run_checks(
            None, None, None, None, None,
            {"session_start", "tool_call_before"},  # codewhale_ours
            None, None, None, None, None, None, None, None,
            breaking, review, blind, errors,
        )
        check(
            d.CODEWHALE_EXECUTOR_URL in served,
            "the executor fetch actually ran (the injected fault fired)",
        )
        cw = lambda xs: [x for x in xs if "DEEPSEEK" in x or "CodeWhale" in x]  # noqa: E731
        return cw(breaking), cw(blind), served

    # (a) THE BUG: the pin lands on the facade. Zero drift claims, one probe-health
    #     line — the report must not name a single env var as renamed.
    br, bl, _ = drive(facade)
    check(not br, f"a stale pin must claim NO upstream change, got {br}")
    check(len(bl) == 1, f"a stale pin is one probe-health line, got {bl}")
    # Guarded index: a regressed anchor gate leaves `bl` empty, and that must
    # fail as a message rather than abort the rest of the suite.
    check(
        all(v not in "".join(bl) for v in ("DEEPSEEK_WORKSPACE", "DEEPSEEK_TOOL_NAME")),
        f"the probe-health line must not name env vars as renamed: {bl!r}",
    )

    # (b) The check still has teeth: same anchor, one env var genuinely removed.
    br, bl, _ = drive(executor.replace('        env.insert("DEEPSEEK_WORKSPACE".to_string(), ws);\n', ""))
    check(
        len(br) == 1 and "DEEPSEEK_WORKSPACE" in br[0],
        f"a REAL rename must still be breaking drift, got breaking={br} blind={bl}",
    )
    check(not bl, f"a readable document produces no probe-health noise, got {bl}")

    # (c) The unchanged upstream is silent in both buckets.
    br, bl, _ = drive(executor)
    check(not br and not bl, f"unchanged upstream -> silence, got breaking={br} blind={bl}")
    d.fetch = real


def test_every_swept_url_declares_an_anchor() -> None:
    """A presence sweep may not run on an unproven document.

    An undeclared URL is REPORTED, not raised (see `fetch_anchored`) — this is
    the static half: every swept URL must appear in `ANCHORS`. The regex below
    resolves the loop form too (`fetch_anchored(u, …)` over a URL tuple), which
    an earlier `[A-Z_]+`-only version missed — the copy-ready shape for the next
    multi-file source, and the one call site the N-of-N guard could not see.
    """
    # An undeclared URL must be REPORTED, never raise. `run_checks` is wrapped in
    # `except Exception` that routes bugs to the TRANSIENT bucket (exit 2,
    # warn-only), so a bare KeyError would turn "someone shipped an unproven
    # sweep" into a green-ish warning — the fail-open shape (#454) this change
    # exists to remove.
    real = d.fetch
    try:
        d.fetch = lambda _u: "irrelevant body"
        blind: list[str] = []
        errors: list[str] = []
        out = d.fetch_anchored("https://example.invalid/undeclared", "New", blind, errors)
        check(out is None, "an undeclared URL is not swept")
        check(len(blind) == 1 and not errors, f"undeclared -> blind, not transient: {blind}")
        check(
            "ANCHORS" in (blind[0] if blind else ""),
            f"the line names the fix (add an ANCHORS entry): {blind!r}",
        )
    finally:
        d.fetch = real

    src = (pathlib.Path(__file__).parent / "check_upstream_drift.py").read_text()
    swept = set(re.findall(r"fetch_anchored\(\s*(\w+(?:\[\d\])?)\s*,", src))
    # A lowercase first arg is a loop variable; resolve it to the tuple it
    # iterates so OPENCODE_EVENT_URLS-style sources are covered too.
    for loop_var in {n for n in swept if not n[0].isupper()}:
        swept.discard(loop_var)
        swept |= {
            f"{m}[{i}]"
            for m in re.findall(rf"for {loop_var} in ([A-Z_]+)", src)
            for i in range(len(getattr(d, m)))
        }
    declared = {
        name
        for name in swept
        if (base := name.split("[")[0]) and hasattr(d, base)
    }
    check(swept == declared, f"a swept URL is not a module constant: {swept - declared}")
    for name in sorted(swept):
        url = eval(f"d.{name}")  # noqa: S307 (module constants matched by the regex above)
        check(url in d.ANCHORS, f"{name} is swept but declares no ANCHORS entry")


def test_report_separates_verified_change_from_probe_health() -> None:
    """The two dispositions must not share a heading — that WAS the bug.

    #793's report filed "our probe missed" under "Breaking drift — decoder will
    silently drop events", which is a claim about upstream the script never had
    evidence for. The heading is the instruction; a reader who trusts it edits a
    decoder.
    """
    real_run, real_read = d.run_checks, d.read_codex_events
    try:
        def fake(*a: object, **k: object) -> None:
            a[-4].append("VERIFIED-CHANGE-LINE")  # type: ignore[union-attr]
            a[-2].append("PROBE-HEALTH-LINE")  # type: ignore[union-attr]

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
        # The probe-health line must sit UNDER the probe heading, not the drift
        # one. `find` (not `index`) so a missing heading REPORTS rather than
        # raising -1-free: -1 breaks the ordering chain and fails cleanly.
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

        # A blind-ONLY report must still exit 1. This is the single load-bearing
        # behavioural claim of the disposition split, and the case above cannot
        # test it: injecting a breaking line too means `breaking` carries the
        # exit code and `blind`'s contribution is never exercised. Deleting
        # `or blind` from main() left the whole suite green until this existed.
        # If it regressed, a report saying the watch is DARK would exit 0, both
        # `exit == '1'` workflow steps would skip, and the weekly run would go
        # green — #454's fail-open, in the file that exists because of #454.
        def blind_only(*a: object, **k: object) -> None:
            a[-2].append("PROBE-HEALTH-ONLY")  # type: ignore[union-attr]

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
        d.run_checks, d.read_codex_events = real_run, real_read


def test_enum_body_survives_struct_variants_and_indentation() -> None:
    """The two positional assumptions `_enum_body` replaced, both directions.

    `(.*?)\\}` stopped at the first `}` (a struct variant truncated the enum);
    `(.*?)\\n\\}` additionally demanded a column-0 closing brace, so grok's enum
    reading as "moved upstream" was really just indentation (#793).
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

    # The OVER-capture direction: an indented enum's own `}` is not at column 0,
    # so the old `\n\}` form ran past it to the next top-level brace and swallowed
    # the following `impl` block. `upstream_codex_hooks` scrapes every CamelCase
    # word, so those idents became phantom variants — and a phantom is worse than
    # a miss, because it makes a real rename look present. Measured on grok's real
    # event.rs the over-capture was 2324 chars.
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

    # A brace inside a comment must not unbalance the count...
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

    # ...and a genuinely absent enum still reads None, never an empty set (which
    # would report every registered event as GONE).
    check(d.upstream_codewhale_hooks("struct Other {}") is None, "absent enum -> None")
    check(d.upstream_codex_hooks("struct Other {}") is None, "absent enum -> None")
    # A prefix name must not bind to a longer enum.
    check(
        d.upstream_codewhale_hooks("pub enum HookEventName {\n    PreToolUse,\n}") is None,
        "`HookEvent` must not match `enum HookEventName`",
    )


def test_report_h1_is_the_issue_title_and_carries_the_disposition() -> None:
    """The H1 is a CROSS-FILE contract with upstream-drift.yml.

    The workflow titles the GitHub issue with `head -1 | sed 's/^# //'` instead
    of keeping its own copy of these strings, so this pins both halves: the
    per-disposition text here, and the fact that the YAML still reads it that
    way. #793's title said "drift detected" for a report whose five drift lines
    were every one of them false positives — with the title carrying the
    disposition, a wrong one mis-signals the issue list itself.
    """
    real = d.run_checks
    cases = [
        ("breaking", -4, "Upstream CLI wire-format drift detected"),
        ("review", -3, "New upstream events to review"),
        ("blind", -2, "Upstream drift watch could not verify — repin needed"),
        ("clean", None, "Upstream wire-format watch: no drift"),
    ]
    try:
        for name, slot, want in cases:
            def fake(*a: object, _s: int | None = slot, **k: object) -> None:
                if _s is not None:
                    a[_s].append("LINE")  # type: ignore[union-attr]

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

    # The consumer half: if the workflow stops reading the H1 (or goes back to
    # grepping the section headings) this contract is silently one-way again.
    wf = (pathlib.Path(__file__).parents[1] / ".github/workflows/upstream-drift.yml").read_text()
    check(
        "head -1 drift-report.md" in wf and "s/^# //" in wf,
        "upstream-drift.yml still titles the issue from the report's H1",
    )
    check(
        "Verified upstream change" not in wf and "could not verify — repin" not in wf,
        "upstream-drift.yml keeps NO second copy of the report's strings",
    )


def main() -> int:
    for t in (
        test_try_fetch_classifies_permanent_vs_transient,
        test_source_parsers_find_nonempty_well_shaped_sets,
        test_upstream_parsers_extract_from_a_snippet,
        test_cc_doc_marker_detection_fires_both_directions,
        test_const_array_parser_ignores_words_quoted_inside_comments,
        test_parser_never_drops_a_real_event,
        test_block_scrape_is_bounded_to_the_decoder,
        test_every_const_array_reader_uses_the_shared_parser,
        test_anchor_gate_fires_in_both_directions,
        test_793_stale_pin_reads_as_probe_health_not_three_renames,
        test_every_swept_url_declares_an_anchor,
        test_report_separates_verified_change_from_probe_health,
        test_enum_body_survives_struct_variants_and_indentation,
        test_report_h1_is_the_issue_title_and_carries_the_disposition,
    ):
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
