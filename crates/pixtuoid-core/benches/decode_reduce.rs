//! Wire→state benchmark: the decode→reduce half that runs on every transcript
//! append and every hook envelope, driven through `harness::Drive` so it can't
//! measure a decoder production doesn't use. The render benchmark next door
//! costs a FRAME; this costs an EVENT, and the two scale with different things.
//!
//! Both transports are measured because their costs are structurally different:
//! the JSONL arm parses the fatter envelope, the hook arm carries the
//! `recent_hook_tool_uses` dedup bookkeeping.
//!
//! Lines are synthesized rather than read from `tests/sources/fixtures/`, which
//! is the conformance harness's scanned population — one transcript per scenario
//! dir, every dir a registered source — so a bench-shaped fixture there would be
//! mis-scanned and panic.

use criterion::{criterion_group, criterion_main, Criterion};
use pixtuoid_core::harness::Drive;

const SESSION: &str = "01000000-0000-7000-8000-0000000000cc";
const CWD: &str = "/home/user/demo-project";
const TRANSCRIPT: &str = "/p/01000000-0000-7000-8000-0000000000cc.jsonl";
/// One turn is a tool call and its result — two lines on either transport.
const TURNS: usize = 200;

fn jsonl_turns(n: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        out.push(format!(
            r#"{{"type":"assistant","cwd":"{CWD}","sessionId":"{SESSION}","timestamp":"2026-01-01T00:00:00.000Z","uuid":"00000000-0000-4000-8000-{i:012x}","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_{i:016x}","name":"Glob","input":{{"pattern":"crates/**/*.rs"}}}}]}}}}"#
        ));
        out.push(format!(
            r#"{{"type":"user","cwd":"{CWD}","sessionId":"{SESSION}","timestamp":"2026-01-01T00:00:00.500Z","uuid":"10000000-0000-4000-8000-{i:012x}","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_{i:016x}","content":"(matches)"}}]}}}}"#
        ));
    }
    out
}

fn hook_turns(n: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(n * 2 + 1);
    out.push(format!(
        r#"{{"hook_event_name":"SessionStart","session_id":"{SESSION}","transcript_path":"{TRANSCRIPT}","cwd":"{CWD}","source":"startup"}}"#
    ));
    for i in 0..n {
        out.push(format!(
            r#"{{"hook_event_name":"PreToolUse","session_id":"{SESSION}","transcript_path":"{TRANSCRIPT}","cwd":"{CWD}","tool_name":"Glob","tool_input":{{"pattern":"crates/**/*.rs"}},"tool_use_id":"toolu_{i:016x}"}}"#
        ));
        out.push(format!(
            r#"{{"hook_event_name":"PostToolUse","session_id":"{SESSION}","transcript_path":"{TRANSCRIPT}","tool_name":"Glob","tool_use_id":"toolu_{i:016x}"}}"#
        ));
    }
    out
}

fn decode_reduce(c: &mut Criterion) {
    let jsonl = jsonl_turns(TURNS);
    let hooks = hook_turns(TURNS);
    let transcript = Drive::transcript("claude-code", TRANSCRIPT)
        .expect("claude-code is transcript-bearing")
        .seeded();
    let hook = Drive::hooks();

    // A silently-rejected line would leave this benchmarking the decoder's error
    // path, which is not the thing being measured.
    for (what, driven) in [
        ("jsonl", transcript.lines(&jsonl)),
        ("hooks", hook.lines(&hooks)),
    ] {
        driven.assert_clean(what);
        assert_eq!(driven.registered(), 1, "{what}: one session must register");
        assert!(
            driven.wire_events() >= TURNS,
            "{what}: the wire must carry at least one event per turn"
        );
    }

    let mut group = c.benchmark_group("decode_reduce");
    group.bench_function(format!("jsonl_{TURNS}_turns"), |b| {
        b.iter(|| transcript.lines(&jsonl))
    });
    group.bench_function(format!("hook_{TURNS}_turns"), |b| {
        b.iter(|| hook.lines(&hooks))
    });
    group.finish();
}

criterion_group!(benches, decode_reduce);
criterion_main!(benches);
