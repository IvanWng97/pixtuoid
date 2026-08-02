//! Corpus check — real transcripts in, "did we parse it AND would the UI show
//! it" out.
//!
//! The committed fixtures answer that question for a dozen curated files. This
//! answers it for the whole corpus on the machine: every transcript a source
//! actually wrote, driven through the SAME path production uses —
//!
//! ```text
//!   file bytes → the registry's line_decoder → a real Reducer
//!              → FloorSession::observe → SimFrame.characters
//! ```
//!
//! `observe` is the documented headless seam: its `characters` are the fully
//! resolved sprites the painter would draw, so a non-empty set is the honest
//! "it reached the UI layer" — no pixel buffer, no terminal, no timing.
//!
//! It REPORTS rather than asserts. Corpus content is unbounded and partly
//! historical, so a failing file is not automatically a bug — the value is the
//! census: decode errors and panics (which ARE bugs, always), how many
//! transcripts register, how many actually render, and the provenance spread
//! that the mtime-vs-newest-turn column exposes.
//!
//! Usage: `cargo run --release -p pixtuoid-scene --example corpus_check -- \
//!         <source> <root> [--json]`

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::registry;
use pixtuoid_core::state::SceneState;
use pixtuoid_core::{AgentEvent, Reducer, Transport};
use pixtuoid_scene::embedded_pack::load_sprite_pack;
use pixtuoid_scene::floor::{FloorMeta, FloorSession};

/// One transcript's verdict. Every field answers a piece of "did we parse it
/// and would the UI show it".
#[derive(Default)]
struct Verdict {
    lines: usize,
    /// Lines that were not valid JSON at all — a torn write, not our problem.
    unparseable: usize,
    /// The decoder returned Err. ALWAYS a defect: the contract is
    /// log-and-continue, never a hard error on a line the source wrote.
    decode_errors: usize,
    events: usize,
    registered: usize,
    /// Sprites the painter would draw for this transcript's agents.
    drawn: usize,
    /// Newest agent-activity stamp the file carries, epoch seconds.
    newest_activity: Option<u64>,
    mtime: Option<u64>,
}

impl Verdict {
    /// The provenance gap the ghost-session class lives in: how far the file's
    /// mtime runs ahead of anything the SESSION itself wrote. A large gap means
    /// something other than the owning session touched the file.
    fn provenance_gap_secs(&self) -> Option<u64> {
        Some(self.mtime?.saturating_sub(self.newest_activity?))
    }
}

fn epoch(t: SystemTime) -> Option<u64> {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// Drive one transcript through the production path.
fn check_file(
    source: &str,
    path: &Path,
    decode: pixtuoid_core::source::decoder::LineDecoder,
) -> Verdict {
    let mut v = Verdict {
        mtime: std::fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(epoch),
        ..Verdict::default()
    };

    let Ok(body) = std::fs::read_to_string(path) else {
        return v;
    };
    let logical = path.to_string_lossy().into_owned();

    // The watcher's `emit_first_sight` is what registers a transcript in
    // production — a JSONL event for an unknown id is a documented no-op. Seed
    // the SAME SessionStart it would emit, keyed by the source's own id
    // derivation, or every file reports "parsed but never reached the UI" for a
    // reason that has nothing to do with the decoder.
    let mut events: Vec<AgentEvent> = vec![AgentEvent::SessionStart {
        agent_id: seed_id(source, path),
        source: source.to_string(),
        session_id: "corpus-check-seed".to_string(),
        cwd: PathBuf::from("/corpus/check"),
        parent_id: None,
    }];
    for line in body.lines().filter(|l| !l.trim().is_empty()) {
        v.lines += 1;
        let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
            v.unparseable += 1;
            continue;
        };
        // The newest turn stamp, read the same way the first-sight gate reads it.
        if let Some(secs) = activity_stamp(source, &val) {
            v.newest_activity = Some(v.newest_activity.map_or(secs, |n: u64| n.max(secs)));
        }
        match decode(&logical, source, val) {
            Ok(decoded) => events.extend(decoded),
            Err(_) => v.decode_errors += 1,
        }
    }
    v.events = events.len();

    // Fold through a real Reducer. A transcript-only source never carries its
    // own SessionStart on every line, so registration is whatever the wire
    // genuinely produces — that IS the thing under test.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let mut scene = SceneState::uniform(8);
    let mut reducer = Reducer::new();
    for ev in events {
        reducer.apply(&mut scene, ev, now, Transport::Jsonl);
    }
    reducer.tick(&mut scene, now + Duration::from_secs(2));
    v.registered = scene.agents.len();
    if v.registered == 0 {
        return v;
    }

    // The UI-layer half: `observe` is the headless seam whose `characters` are
    // the resolved sprites the painter would draw.
    let pack = load_sprite_pack(None).expect("embedded pack");
    let mut session = FloorSession::new();
    v.drawn = session
        .observe(&scene, &pack, 192, 80, FloorMeta::ground(), now)
        .map_or(0, |frame| frame.characters.len());
    v
}

/// The agent id the WATCHER would key this transcript on — each source derives
/// it from the path its own way, and a mismatch here would split one transcript
/// into two agents exactly as it does in production.
fn seed_id(source: &str, path: &Path) -> pixtuoid_core::AgentId {
    use pixtuoid_core::AgentId;
    match source {
        "claude-code" => AgentId::from_parts(
            source,
            &pixtuoid_core::source::claude_code::cc_id_from_path(path),
        ),
        "codex" => AgentId::from_parts(
            source,
            &pixtuoid_core::source::codex::codex_id_from_path(path),
        ),
        "grok" => AgentId::from_parts(
            source,
            &pixtuoid_core::source::grok::grok_id_from_path(path),
        ),
        _ => AgentId::from_transcript_path(&path.to_string_lossy()),
    }
}

/// The per-source "is this line the SESSION speaking, and when" read. Only CC
/// has a published answer today (its `ACTIVITY_TYPES`); every other source
/// reports no stamp, which shows up as an empty provenance column rather than a
/// wrong one.
fn activity_stamp(source: &str, v: &serde_json::Value) -> Option<u64> {
    if source != pixtuoid_core::source::claude_code::SOURCE_NAME {
        return None;
    }
    let line = serde_json::to_vec(v).ok()?;
    let mut buf = line;
    buf.push(b'\n');
    match pixtuoid_core::source::claude_code::cc_activity_recency(&buf) {
        pixtuoid_core::source::decoder::TailActivity::At(secs) => Some(secs),
        _ => None,
    }
}

fn walk(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("jsonl") {
            out.push(p);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json = args.iter().any(|a| a == "--json");
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if positional.len() < 2 {
        eprintln!("usage: corpus_check <source> <root> [--json]");
        std::process::exit(2);
    }
    let (source, root) = (positional[0].as_str(), PathBuf::from(positional[1]));

    let Some(decode) = registry::descriptor_for(source).and_then(|d| d.line_decoder()) else {
        eprintln!("error: {source:?} is not a transcript-bearing registered source");
        std::process::exit(2);
    };

    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    if files.is_empty() {
        eprintln!("error: no .jsonl under {}", root.display());
        std::process::exit(2);
    }

    let mut totals = Verdict::default();
    let mut registered_files = 0usize;
    let mut drawn_files = 0usize;
    let mut gap_buckets: BTreeMap<&str, usize> = BTreeMap::new();
    let mut error_files: Vec<(PathBuf, usize)> = Vec::new();

    for f in &files {
        let v = check_file(source, f, decode);
        totals.lines += v.lines;
        totals.unparseable += v.unparseable;
        totals.decode_errors += v.decode_errors;
        totals.events += v.events;
        if v.registered > 0 {
            registered_files += 1;
        }
        if v.drawn > 0 {
            drawn_files += 1;
        }
        if v.decode_errors > 0 {
            error_files.push((f.clone(), v.decode_errors));
        }
        // Provenance census: how far mtime runs ahead of the newest turn.
        let bucket = match v.provenance_gap_secs() {
            None => "no-stamp",
            Some(s) if s < 3600 => "<1h",
            Some(s) if s < 86_400 => "<1d",
            Some(s) if s < 7 * 86_400 => "<7d",
            Some(_) => ">=7d",
        };
        *gap_buckets.entry(bucket).or_default() += 1;
    }

    if json {
        println!(
            r#"{{"source":"{source}","files":{},"lines":{},"unparseable":{},"decode_errors":{},"events":{},"registered_files":{registered_files},"drawn_files":{drawn_files}}}"#,
            files.len(),
            totals.lines,
            totals.unparseable,
            totals.decode_errors,
            totals.events
        );
    } else {
        println!("corpus: {source}  root={}", root.display());
        println!("  files             {}", files.len());
        println!("  lines             {}", totals.lines);
        println!(
            "  unparseable       {} (torn writes; not ours)",
            totals.unparseable
        );
        println!("  DECODE ERRORS     {}  <- must be 0", totals.decode_errors);
        println!("  events decoded    {}", totals.events);
        println!(
            "  registered        {registered_files}/{} files produced >=1 slot",
            files.len()
        );
        println!(
            "  reached the UI    {drawn_files}/{registered_files} registered files would paint a sprite"
        );
        println!("  provenance (mtime ahead of the newest turn):");
        for (k, n) in &gap_buckets {
            println!("      {k:<9} {n}");
        }
        for (p, n) in error_files.iter().take(10) {
            println!("  decode error x{n}: {}", p.display());
        }
    }

    // The ONLY hard failure: a decoder returning Err on bytes its own source
    // wrote. Everything else is a census, because corpus content is unbounded.
    if totals.decode_errors > 0 {
        std::process::exit(1);
    }
}
