//! Real-corpus never-panic harness for the per-source decoders.
//!
//! Reads JSONL lines on stdin and runs each through the decoder its shape
//! selects — `decode_hook_payload` for a hook payload (`hook_event_name`
//! present), `decode_codex_line` for a Codex rollout line (`type` is
//! `event_msg`/`response_item`/`session_meta`/`turn_context`), else
//! `decode_cc_line` for a Claude Code transcript line — inside
//! `catch_unwind`, and asserts the **never-panic** invariant (workspace
//! invariant #5 / the "log + continue, never panic" decoder contract).
//!
//! It is a TOOL, not a committed corpus: point it at any JSONL tree —
//! ```
//! just fuzz ~/.claude/projects                 # your own CC sessions (newest formats)
//! just fuzz ~/.codex/sessions                  # your own Codex rollouts
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

use pixtuoid_core::source::claude_code::decode_cc_line;
use pixtuoid_core::source::codex::decode_codex_line;
use pixtuoid_core::source::decoder::decode_hook_payload;

fn main() {
    // A placeholder path: the decoders fold it into an AgentId but the
    // never-panic contract is path-independent, so one stand-in is fine.
    let cc_path = "/home/u/.claude/projects/p/session.jsonl";
    let codex_path =
        "/home/u/.codex/sessions/2026/01/01/rollout-1-019e7762-9ded-7e33-be41-946ecf105bf4.jsonl";

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

        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if is_hook {
                decode_hook_payload(v.clone()).map(|e| vec![e])
            } else if is_codex {
                decode_codex_line(codex_path, "codex", v.clone())
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
