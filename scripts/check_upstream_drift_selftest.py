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
        # Permanent HTTP → breaking (the URL moved; watch blind).
        for code in (404, 410, 451):
            d.fetch = lambda _u, c=code: (_ for _ in ()).throw(_http_error(c))
            br: list[str] = []
            er: list[str] = []
            out = d.try_fetch("https://x/y", "T", br, er)
            check(out is None, f"{code}: returns None")
            check(len(br) == 1 and not er, f"{code}: -> breaking (got breaking={br} errors={er})")
            check(str(code) in br[0], f"{code}: message names the status")

        # Transient HTTP (server/throttle) → errors, NOT breaking.
        for code in (500, 502, 503, 429, 403):
            d.fetch = lambda _u, c=code: (_ for _ in ()).throw(_http_error(c))
            br, er = [], []
            d.try_fetch("https://x/y", "T", br, er)
            check(not br and len(er) == 1, f"{code}: -> transient (got breaking={br} errors={er})")

        # Network-layer failure → transient.
        d.fetch = lambda _u: (_ for _ in ()).throw(urllib.error.URLError("conn refused"))
        br, er = [], []
        d.try_fetch("https://x/y", "T", br, er)
        check(not br and len(er) == 1, f"URLError -> transient (got breaking={br} errors={er})")

        # Success → returns the body, no buckets touched.
        d.fetch = lambda _u: "BODY"
        br, er = [], []
        out = d.try_fetch("https://x/y", "T", br, er)
        check(out == "BODY" and not br and not er, "success returns body, no buckets")
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
        (d.read_cc_events, r"^[A-Za-z]\w+$", 2),
        (d.read_dispatch_names, r"^[A-Za-z]\w+$", 1),
        (d.read_reasonix_events, r"^[A-Za-z]\w+$", 2),
        (d.read_codewhale_events, r"^[a-z][a-z_]*$", 2),
        (d.read_openclaw_events, r"^[a-z][a-z_]*$", 2),
        (d.read_opencode_events, r"^[a-z][a-z0-9._]*$", 2),
        (d.read_copilot_events, r"^[a-z][a-z0-9._]*$", 2),
        (d.read_omp_entry_types, r"^[a-z]+$", 3),
        (d.read_cursor_events, r"^[a-zA-Z]\w+$", 2),
        (d.read_hermes_events, r"^[a-z][a-z_]*$", 2),
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


def main() -> int:
    for t in (
        test_try_fetch_classifies_permanent_vs_transient,
        test_source_parsers_find_nonempty_well_shaped_sets,
        test_upstream_parsers_extract_from_a_snippet,
        test_cc_doc_marker_detection_fires_both_directions,
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
