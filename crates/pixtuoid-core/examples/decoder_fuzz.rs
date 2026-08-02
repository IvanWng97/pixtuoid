//! Real-corpus never-panic harness for a source's transcript LINE decoder.
//!
//! The thinnest of the four `harness::Drive` shells: bytes come from **stdin**
//! and the verdict is **exit non-zero on any panic**. Everything between —
//! parse, decode under `catch_unwind`, fold — is the shared pipeline, so this
//! tool can no longer drift from what the fixture/corpus/render drivers run.
//!
//! The source is an ARGUMENT, not sniffed from line shape. The caller already
//! knows which source a corpus is (one dir per invocation), and inferring it
//! from `type`/field shape silently MISROUTED newer sources whose shape didn't
//! match a hard-coded predicate (grok's `method` envelope, omp's bare `type`)
//! to `decode_cc_line`, reporting a false-green "0 panics" having exercised the
//! WRONG decoder. `Drive::transcript` resolves the decoder through the registry
//! and REFUSES a source that has none, so coverage stays structural — a new
//! source is reachable the moment it has a `SourceDescriptor` row.
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
//!
//! Decode `Err` is allowed (the watcher logs + skips malformed lines); only a
//! PANIC is a contract violation. Hook-only sources have no transcript line
//! decoder; their never-panic contract is covered by the in-crate proptest
//! `every_hook_and_presence_decoder_never_panics`.

use std::io::BufRead;

use pixtuoid_core::harness::Drive;
use pixtuoid_core::source::registry;

/// Panic reports are capped: a systematically broken decoder panics on every
/// line, and the point is the SHAPE, which repeats.
const MAX_REPORTED_PANICS: usize = 10;

fn main() {
    let source = std::env::args().nth(1).unwrap_or_default();
    if registry::descriptor_for(&source).is_none() {
        let known: Vec<&str> = registry::registered_source_names().collect();
        eprintln!(
            "decoder_fuzz: unknown source {source:?} — pass a registered source name \
             (one of: {})",
            known.join(", ")
        );
        std::process::exit(2);
    }
    // A placeholder transcript path: each decoder folds it into an AgentId, but
    // the never-panic contract is path-independent, so one stand-in is fine.
    // Unseeded for the same reason — registration is not what is under test.
    let Some(drive) = Drive::transcript(&source, "/fuzz/session.jsonl") else {
        eprintln!(
            "decoder_fuzz: source {source:?} has no transcript line decoder (hook-only or \
             daemon) — its never-panic contract is covered by the \
             `every_hook_and_presence_decoder_never_panics` proptest, not this corpus tool"
        );
        std::process::exit(2);
    };

    // A non-UTF-8 line is skipped, not fatal — the corpus is whatever is on
    // disk, and stopping would silently truncate the run.
    let driven = drive.lines(std::io::stdin().lock().lines().map_while(Result::ok));

    eprintln!(
        "decoder_fuzz[{source}]: {} lines, {} parsed, {} events, {} decode-err, {} PANIC",
        driven.lines,
        driven.lines - driven.unparseable,
        driven.events.len(),
        driven.decode_errors.len(),
        driven.panics.len()
    );
    // STRUCTURE only, never content — a transcript line carries real prose/code.
    for f in driven.panics.iter().take(MAX_REPORTED_PANICS) {
        eprintln!("  PANIC on: {f}");
    }
    if !driven.panics.is_empty() {
        std::process::exit(1);
    }
}
