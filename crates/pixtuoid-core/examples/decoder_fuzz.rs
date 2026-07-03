//! Real-corpus never-panic harness for the per-source decoders.
//!
//! Reads JSONL lines on stdin and runs each through the decoder its shape
//! selects — `decode_hook_payload` for a hook payload (`hook_event_name`
//! present), `decode_codex_line` for a Codex rollout line (`type` is
//! `event_msg`/`response_item`/`session_meta`/`turn_context`),
//! `decode_copilot_line` for a Copilot transcript line (dotted `type`, e.g.
//! `session.start`/`tool.execution_start`), `decode_ag_line` for an Antigravity
//! line (integer `step_index`), else `decode_cc_line` for a Claude Code
//! transcript line — inside `catch_unwind`, and asserts the **never-panic**
//! invariant (workspace invariant #5 / the "log + continue, never panic"
//! decoder contract). Every transcript-bearing source's decoder is reachable —
//! a new one must add its shape route here.
//!
//! It is a TOOL, not a committed corpus: point it at any JSONL tree —
//! ```
//! just fuzz ~/.claude/projects                 # your own CC sessions (newest formats)
//! just fuzz ~/.codex/sessions                  # your own Codex rollouts
//! just fuzz ~/.copilot/session-state           # your own Copilot CLI sessions
//! git clone https://github.com/daaain/claude-code-log /tmp/cc \
//!   && just fuzz /tmp/cc/test_data/real_projects   # a public real-world CC corpus
//! ```
//! Nothing is committed or redistributed, so there's no license / size /
//! sanitization concern — the public sessions are a target, not a dependency.
//! Exits non-zero if any line panics (so `just fuzz` fails loudly).
//!
//! Decode `Err` is allowed (the watcher logs + skips malformed lines); only a
//! PANIC is a contract violation.

use std::io::BufRead;

use pixtuoid_core::source::antigravity::decode_ag_line;
use pixtuoid_core::source::claude_code::decode_cc_line;
use pixtuoid_core::source::codex::decode_codex_line;
use pixtuoid_core::source::copilot::decode_copilot_line;
use pixtuoid_core::source::decoder::decode_hook_payload;

fn main() {
    // A placeholder path: the decoders fold it into an AgentId but the
    // never-panic contract is path-independent, so one stand-in is fine.
    let cc_path = "/home/u/.claude/projects/p/session.jsonl";
    let codex_path =
        "/home/u/.codex/sessions/2026/01/01/rollout-1-019e7762-9ded-7e33-be41-946ecf105bf4.jsonl";
    // Copilot derives its id from the events.jsonl PARENT dir (the stem is the
    // constant `events`); Antigravity folds its path into the AgentId. The
    // never-panic contract is path-independent, so these stand-ins are fine.
    let copilot_path =
        "/home/u/.copilot/session-state/019e7762-9ded-7e33-be41-946ecf105bf4/events.jsonl";
    let ag_path = "/home/u/.antigravity/sessions/019e7762/transcript.jsonl";

    let (mut lines, mut parsed, mut events, mut errs, mut panics) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut panic_shapes: Vec<String> = Vec::new();

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        lines += 1;
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue; // non-JSON: the watcher skips it — outside the decoder contract
        };
        parsed += 1;

        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let is_hook = v.get("hook_event_name").is_some();
        let is_codex = matches!(
            ty,
            "event_msg" | "response_item" | "session_meta" | "turn_context"
        );
        // Copilot transcript lines carry a DOTTED `type` (session.start,
        // tool.execution_start, permission.requested, subagent.started, …);
        // Antigravity lines carry an integer `step_index`. Both shapes are
        // disjoint from CC (bare `type`: user/assistant/…) and Codex, so a
        // corpus of either now reaches its OWN decoder instead of silently
        // falling through to decode_cc_line (which reported a false-green
        // never-panic pass having exercised the WRONG decoder).
        let is_copilot = ty.contains('.');
        let is_ag = v.get("step_index").and_then(|s| s.as_i64()).is_some();

        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if is_hook {
                decode_hook_payload(v.clone())
            } else if is_codex {
                decode_codex_line(codex_path, "codex", v.clone())
            } else if is_copilot {
                decode_copilot_line(copilot_path, "copilot", v.clone())
            } else if is_ag {
                decode_ag_line(ag_path, "antigravity", v.clone())
            } else {
                decode_cc_line(cc_path, "claude-code", v.clone())
            }
        }));

        match res {
            Ok(Ok(evs)) => events += evs.len() as u64,
            Ok(Err(_)) => errs += 1,
            Err(_) => {
                panics += 1;
                if panic_shapes.len() < 10 {
                    // Print STRUCTURE only (top-level keys), never the content —
                    // a transcript line carries real prose/code.
                    let keys = v
                        .as_object()
                        .map(|o| o.keys().cloned().collect::<Vec<_>>().join(","))
                        .unwrap_or_else(|| "<non-object>".into());
                    panic_shapes.push(format!("type={ty:?} keys=[{keys}]"));
                }
            }
        }
    }

    eprintln!(
        "decoder_fuzz: {lines} lines, {parsed} parsed, {events} events, {errs} decode-err, {panics} PANIC"
    );
    for s in &panic_shapes {
        eprintln!("  PANIC on: {s}");
    }
    if panics > 0 {
        std::process::exit(1);
    }
}
