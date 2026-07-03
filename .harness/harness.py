#!/usr/bin/env python3
"""pixtuoid harness — flight recorder + action gate for Claude Code (dogfood v0).

A PreToolUse hook that (1) DECIDES allow/deny for a proposed tool action against a
declarative per-repo policy (`.harness/policy.toml`), matched on the REAL
`tool_input` (provenance = environment — the model cannot fake which call it is
making), (2) appends a HASH-CHAINED ledger line either way
(`.harness/ledger.jsonl`), and (3) BLOCKS a deny with exit 2 (fires before the
permission check).

This is EVIDENCE-first, not prevention. A same-uid agent can still bypass (disable
the hook, obfuscated bash). The value is that a bypass or a *loosened guardrail*
becomes DETECTABLE: the hash-chain makes ledger tampering evident, `replay` goes
RED the moment a rule stops denying a should-deny action, and `canary` fails LOUD
if the gate has gone inert. Prevention is impossible against a same-privilege
agent; detection is not. See `.harness/README.md`.

Subcommands:
  gate      read a PreToolUse JSON on stdin -> decide, append ledger, exit 2 on deny
  replay    re-decide every labeled fixture under the CURRENT policy; RED on mismatch
  canary    fire the known-deny fixtures; LOUD fail if any now allows (gate inert)
  verify    check the ledger hash-chain integrity (tamper-evident)
  selftest  internal matcher + hash-chain unit checks (the gate on the gate)

Escape hatch: HARNESS_GATE_OFF=1 makes `gate` record a 'BYPASS' ledger line and
exit 0 — an AUDITED hole, not a silent one.

Stdlib only (tomllib needs Python >= 3.11). macOS / POSIX first.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import sys
import tomllib
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath

HARNESS_DIR = Path(__file__).resolve().parent
POLICY_PATH = HARNESS_DIR / "policy.toml"
LEDGER_PATH = HARNESS_DIR / "ledger.jsonl"
FIXTURES_PATH = HARNESS_DIR / "fixtures.jsonl"
REPO_ROOT = HARNESS_DIR.parent

# 64 hex zeros — the prev_hash of the first ledger line (chain anchor).
GENESIS = "0" * 64
EDIT_TOOLS = ("Edit", "MultiEdit", "Write", "NotebookEdit")


@dataclass(frozen=True)
class Rule:
    id: str
    severity: str  # "block" (only blocks can deny) | "warn" (advisory, never denies)
    tools: tuple[str, ...]
    path_glob: tuple[str, ...]
    path_exclude: tuple[str, ...]
    content_re: re.Pattern | None
    command_re: re.Pattern | None
    reason: str


def load_policy(path: Path = POLICY_PATH) -> list[Rule]:
    data = tomllib.loads(path.read_text())
    rules: list[Rule] = []
    for r in data.get("rule", []):
        rules.append(
            Rule(
                id=r["id"],
                severity=r.get("severity", "block"),
                tools=tuple(r.get("tools", [])),
                path_glob=tuple(r.get("path_glob", [])),
                path_exclude=tuple(r.get("path_exclude", [])),
                content_re=re.compile(r["content_regex"]) if r.get("content_regex") else None,
                command_re=re.compile(r["command_regex"]) if r.get("command_regex") else None,
                reason=r["reason"],
            )
        )
    return rules


def _extract(event: dict) -> tuple[str, list[str], str, str | None]:
    """(tool_name, target_paths, new_content, bash_command) from a hook event."""
    tool = event.get("tool_name", "") or ""
    ti = event.get("tool_input") or {}
    paths: list[str] = []
    content: list[str] = []
    command: str | None = None
    if tool in EDIT_TOOLS:
        fp = ti.get("file_path") or ti.get("notebook_path")
        if fp:
            paths.append(str(fp))
        if tool == "Write":
            content.append(ti.get("content", "") or "")
        elif tool == "Edit":
            content.append(ti.get("new_string", "") or "")
        elif tool == "MultiEdit":
            for e in ti.get("edits", []) or []:
                content.append(e.get("new_string", "") or "")
        elif tool == "NotebookEdit":
            content.append(ti.get("new_source", "") or "")
    elif tool == "Bash":
        command = ti.get("command", "") or ""
    return tool, paths, "\n".join(content), command


def _candidates(p: str, root: Path) -> list[str]:
    """Path forms to test a glob against: repo-relative, ~-expanded, and raw.

    Live CC hands an absolute file_path (matched via the repo-relative form);
    fixtures carry repo-relative paths (matched via the raw form). Both covered.
    """
    cands: list[str] = []
    try:
        cands.append(PurePosixPath(Path(p).resolve().relative_to(root)).as_posix())
    except Exception:
        pass
    cands.append(PurePosixPath(os.path.expanduser(p)).as_posix())
    cands.append(PurePosixPath(p).as_posix())
    return cands


def _glob_hit(cands: list[str], glob: str) -> bool:
    for c in cands:
        try:
            if PurePosixPath(c).full_match(glob):
                return True
        except Exception:
            pass
    return False


def _path_match(paths: list[str], rule: Rule, root: Path) -> bool:
    for p in paths:
        cands = _candidates(p, root)
        if any(_glob_hit(cands, g) for g in rule.path_glob):
            if not any(_glob_hit(cands, g) for g in rule.path_exclude):
                return True
    return False


@dataclass(frozen=True)
class Decision:
    decision: str  # "allow" | "deny"
    rule_id: str | None
    reason: str | None
    target: str


def decide(event: dict, rules: list[Rule], root: Path | None = None) -> Decision:
    root = root or Path(event.get("cwd") or REPO_ROOT).resolve()
    tool, paths, content, command = _extract(event)
    target = paths[0] if paths else (command[:80] if command else tool)
    for rule in rules:
        if rule.severity != "block" or tool not in rule.tools:
            continue
        hit = False
        if rule.path_glob:
            hit = _path_match(paths, rule, root)
            if hit and rule.content_re is not None:
                hit = bool(rule.content_re.search(content))
        elif rule.command_re is not None and command is not None:
            hit = bool(rule.command_re.search(command))
        elif rule.content_re is not None:
            hit = bool(rule.content_re.search(content))
        if hit:
            return Decision("deny", rule.id, rule.reason, target)
    return Decision("allow", None, None, target)


# --- ledger (hash-chained, append-only, tamper-EVIDENT) ----------------------


def _canonical(obj: object) -> str:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def _read_ledger(ledger: Path) -> list[dict]:
    if not ledger.exists():
        return []
    return [json.loads(x) for x in ledger.read_text().splitlines() if x.strip()]


def ledger_append(decision: Decision, event: dict, tool: str, ledger: Path = LEDGER_PATH) -> dict:
    rows = _read_ledger(ledger)
    prev: str = str(rows[-1]["line_hash"]) if rows else GENESIS
    seq = (int(rows[-1]["seq"]) + 1) if rows else 0
    ti = event.get("tool_input") or {}
    payload = {
        "seq": seq,
        "ts": datetime.now(timezone.utc).isoformat(),
        "session_id": event.get("session_id"),
        "iteration_id": event.get("prompt_id"),
        "span_id": event.get("tool_use_id"),
        "agent_id": event.get("agent_id"),
        "tool": tool,
        "decision": decision.decision,
        "rule_id": decision.rule_id,
        "reason": decision.reason,
        "target": decision.target,
        "args_hash": hashlib.sha256(_canonical(ti).encode()).hexdigest(),
        "prev_hash": prev,
    }
    payload["line_hash"] = hashlib.sha256((prev + _canonical(payload)).encode()).hexdigest()
    with ledger.open("a") as f:
        f.write(_canonical(payload) + "\n")
    return payload


def ledger_verify(ledger: Path = LEDGER_PATH) -> tuple[bool, int | None, int]:
    prev: str = GENESIS
    n = 0
    for i, rec in enumerate(_read_ledger(ledger)):
        stored = str(rec.get("line_hash"))
        payload = {k: v for k, v in rec.items() if k != "line_hash"}
        recomputed = hashlib.sha256((prev + _canonical(payload)).encode()).hexdigest()
        if rec.get("prev_hash") != prev or recomputed != stored:
            return False, rec.get("seq", i), n
        prev = stored
        n += 1
    return True, None, n


# --- subcommands -------------------------------------------------------------


def cmd_gate() -> int:
    raw = sys.stdin.read()
    try:
        event = json.loads(raw) if raw.strip() else {}
    except Exception as e:  # noqa: BLE001 — never let a malformed payload run un-gated
        sys.stderr.write(f"[harness] gate: bad stdin JSON ({e}); failing closed\n")
        return 2
    tool, *_ = _extract(event)
    if os.environ.get("HARNESS_GATE_OFF") == "1":
        try:
            ledger_append(Decision("allow", "BYPASS", "HARNESS_GATE_OFF=1", tool), event, tool)
        except Exception:  # noqa: BLE001 — bypass must not itself block
            pass
        sys.stderr.write("[harness] GATE BYPASSED via HARNESS_GATE_OFF=1 (recorded)\n")
        return 0
    try:
        d = decide(event, load_policy())
        ledger_append(d, event, tool)
    except Exception as e:  # noqa: BLE001 — a gate bug fails CLOSED and LOUD, not open
        sys.stderr.write(f"[harness] gate ERROR (failing closed): {e}\n")
        return 2
    if d.decision == "deny":
        sys.stderr.write(f"[harness] BLOCKED by rule '{d.rule_id}': {d.reason}\n  target: {d.target}\n")
        return 2
    return 0


def _load_fixtures() -> list[dict]:
    return [json.loads(x) for x in FIXTURES_PATH.read_text().splitlines() if x.strip()]


def cmd_replay() -> int:
    rules = load_policy()
    fx = _load_fixtures()
    bad = []
    for f in fx:
        d = decide(f["event"], rules, REPO_ROOT)
        ok = d.decision == f["expect"]
        if not ok:
            bad.append((f, d))
        print(f"  [{'ok ' if ok else 'RED'}] {f['label']}: expect={f['expect']} got={d.decision} rule={d.rule_id}")
    if bad:
        print(f"\nREPLAY RED: {len(bad)}/{len(fx)} fixture(s) changed behavior — a guardrail moved:")
        for f, d in bad:
            print(f"  - {f['label']}: expected {f['expect']}, got {d.decision} (rule={d.rule_id}). {f.get('why', '')}")
        return 1
    print(f"\nreplay green: {len(fx)}/{len(fx)} fixtures hold.")
    return 0


def cmd_canary() -> int:
    rules = load_policy()
    fx = [f for f in _load_fixtures() if f.get("canary")]
    dead = [f for f in fx if decide(f["event"], rules, REPO_ROOT).decision != "deny"]
    if dead:
        print("!!! HARNESS CANARY FAILED — the gate is INERT for:")
        for f in dead:
            print(f"  - {f['label']} (rule '{f.get('rule')}') now returns ALLOW")
        print("The gate's teeth are gone. Do NOT trust the ledger until this is fixed.")
        return 1
    print(f"canary ok: {len(fx)} known-deny actions still blocked.")
    return 0


def cmd_verify() -> int:
    ok, seq, n = ledger_verify()
    if not ok:
        print(f"LEDGER TAMPERED — hash-chain breaks at seq={seq} ({n} line(s) intact before the break).")
        return 1
    print(f"ledger intact: {n} line(s), hash-chain unbroken.")
    return 0


def cmd_selftest() -> int:
    import tempfile

    rules = load_policy()
    root = REPO_ROOT
    fails: list[str] = []

    def ev(tool: str, **ti: object) -> dict:
        return {"tool_name": tool, "tool_input": ti}

    checks = [
        ("core-no-tui blocks ratatui in core", ev("Write", file_path="crates/pixtuoid-core/src/x.rs", content="use ratatui::X;"), "deny", "core-no-tui"),
        ("ratatui allowed in the binary crate", ev("Write", file_path="crates/pixtuoid/src/x.rs", content="use ratatui::X;"), "allow", None),
        ("no-prod-println blocks println in src", ev("Edit", file_path="crates/pixtuoid/src/run.rs", new_string='println!("x");'), "deny", "no-prod-println"),
        ("println allowed under a tests/ dir", ev("Edit", file_path="crates/pixtuoid/src/tests/t.rs", new_string='println!("x");'), "allow", None),
        ("self-protect blocks editing the policy", ev("Edit", file_path=".harness/policy.toml", new_string="x"), "deny", "self-protect"),
        ("bash write to ~/.claude/settings.json blocked", {"tool_name": "Bash", "tool_input": {"command": "echo x >> ~/.claude/settings.json"}}, "deny", "no-user-settings-write"),
        ("ordinary edit allowed", ev("Write", file_path="crates/pixtuoid/src/ok.rs", content="fn main() {}"), "allow", None),
    ]
    for label, event, want_dec, want_rule in checks:
        d = decide(event, rules, root)
        if d.decision != want_dec or (want_rule is not None and d.rule_id != want_rule):
            fails.append(f"{label}: got {d.decision}/{d.rule_id}, want {want_dec}/{want_rule}")

    # hash-chain roundtrip + tamper detection on a scratch ledger.
    with tempfile.TemporaryDirectory() as td:
        led = Path(td) / "ledger.jsonl"
        for i in range(3):
            ledger_append(Decision("deny" if i == 1 else "allow", "r", "why", f"t{i}"), {"tool_input": {"i": i}}, "Write", led)
        ok, _, n = ledger_verify(led)
        if not ok or n != 3:
            fails.append(f"hash-chain roundtrip broken (ok={ok}, n={n})")
        lines = led.read_text().splitlines()
        rec = json.loads(lines[1])
        rec["decision"] = "allow"  # tamper: flip a deny to allow
        lines[1] = _canonical(rec)
        led.write_text("\n".join(lines) + "\n")
        ok2, seq2, _ = ledger_verify(led)
        if ok2 or seq2 != 1:
            fails.append(f"tamper not detected (ok={ok2}, seq={seq2})")

    if fails:
        print("SELFTEST FAILED:")
        for m in fails:
            print(f"  - {m}")
        return 1
    print(f"selftest ok: {len(checks)} decision checks + hash-chain roundtrip + tamper-detection.")
    return 0


def main(argv: list[str]) -> int:
    cmds = {
        "gate": cmd_gate,
        "replay": cmd_replay,
        "canary": cmd_canary,
        "verify": cmd_verify,
        "selftest": cmd_selftest,
    }
    if len(argv) < 2 or argv[1] not in cmds:
        sys.stderr.write("usage: harness.py {gate|replay|canary|verify|selftest}\n")
        return 2
    return cmds[argv[1]]()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
