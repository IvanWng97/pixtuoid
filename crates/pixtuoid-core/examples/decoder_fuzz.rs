//! Real-corpus never-panic harness for a source's transcript LINE decoder.
//!
//! Reads JSONL lines on stdin and runs EVERY line through the `line_decoder()`
//! of the source named on the command line — resolved through the registry
//! (`registry::descriptor_for(<source>).line_decoder()`), inside `catch_unwind`,
//! asserting the **never-panic** invariant (workspace invariant #5 / the "log +
//! continue, never panic" decoder contract).
//!
//! The source is an ARGUMENT, not sniffed from line shape. The caller already
//! knows which source a corpus is (one dir per invocation), and inferring it
//! from `type`/field shape silently MISROUTED newer sources whose shape didn't
//! match a hard-coded predicate (grok's `method` envelope, omp's bare `type`)
//! to `decode_cc_line`, reporting a false-green "0 panics" having exercised the
//! WRONG decoder. Registry dispatch makes coverage structural — a new source is
//! reachable the moment it has a `SourceDescriptor` row, with no edit here.
//!
//! It is a TOOL, not a committed corpus: point it at any JSONL tree —
//! ```
//! just fuzz claude-code ~/.claude/projects        # your own CC sessions (newest formats)
//! just fuzz codex ~/.codex/sessions               # your own Codex rollouts
//! just fuzz grok ~/.grok/sessions                 # grok ACP transcripts
//! just fuzz omp ~/.omp/agent/sessions             # omp sessions
//! just fuzz copilot ~/.copilot/session-state      # your own Copilot CLI sessions
//! git clone https://github.com/daaain/claude-code-log /tmp/cc \
//!   && just fuzz claude-code /tmp/cc/test_data/real_projects   # a public real-world CC corpus
//! ```
//! Nothing is committed or redistributed, so there's no license / size /
//! sanitization concern — the public sessions are a target, not a dependency.
//! Exits non-zero if any line panics (so `just fuzz` fails loudly).
//!
//! Decode `Err` is allowed (the watcher logs + skips malformed lines); only a
//! PANIC is a contract violation. Hook-only sources have no transcript line
//! decoder; their never-panic contract is covered by the in-crate proptest
//! `every_hook_and_presence_decoder_never_panics`.

use std::io::BufRead;

use pixtuoid_core::source::registry;

fn main() {
    let source = std::env::args().nth(1).unwrap_or_default();
    let Some(desc) = registry::descriptor_for(&source) else {
        let known: Vec<&str> = registry::registered_source_names().collect();
        eprintln!(
            "decoder_fuzz: unknown source {source:?} — pass a registered source name \
             (one of: {})",
            known.join(", ")
        );
        std::process::exit(2);
    };
    let Some(decode) = desc.line_decoder() else {
        eprintln!(
            "decoder_fuzz: source {source:?} has no transcript line decoder (hook-only or \
             daemon) — its never-panic contract is covered by the \
             `every_hook_and_presence_decoder_never_panics` proptest, not this corpus tool"
        );
        std::process::exit(2);
    };

    // A placeholder transcript path: each decoder folds it into an AgentId, but
    // the never-panic contract is path-independent, so one stand-in is fine.
    let path = "/fuzz/session.jsonl";

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

        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            decode(path, &source, v.clone())
        }));

        match res {
            Ok(Ok(evs)) => events += evs.len() as u64,
            Ok(Err(_)) => errs += 1,
            Err(_) => {
                panics += 1;
                if panic_shapes.len() < 10 {
                    // Print STRUCTURE only (top-level keys), never the content —
                    // a transcript line carries real prose/code.
                    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
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
        "decoder_fuzz[{source}]: {lines} lines, {parsed} parsed, {events} events, {errs} decode-err, {panics} PANIC"
    );
    for s in &panic_shapes {
        eprintln!("  PANIC on: {s}");
    }
    if panics > 0 {
        std::process::exit(1);
    }
}
