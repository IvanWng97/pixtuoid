//! Corpus check — real transcripts in, "did we parse it AND would the UI show
//! it" out.
//!
//! The committed fixtures answer that question for a dozen curated files. This
//! answers it for the whole corpus on the machine, and it is the ONE shell of
//! the four that closes the loop all the way to the render layer:
//!
//! ```text
//!   harness::Drive (decode → reduce)  →  FloorSession::observe → SimFrame.characters
//! ```
//!
//! The first half is the shared pipeline every other driver runs — same
//! decoders, same first-sight seed, same reducer — so a difference here is a
//! difference in the BYTES, never in the harness. `observe` is the documented
//! headless seam: its `characters` are the fully resolved sprites the painter
//! would draw, so a non-empty set is the honest "it reached the UI layer" — no
//! pixel buffer, no terminal, no timing.
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

use pixtuoid_core::harness::{Drive, LineFailure};
use pixtuoid_core::source::decoder::TailActivity;
use pixtuoid_core::source::registry;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_scene::embedded_pack::load_sprite_pack;
use pixtuoid_scene::floor::{FloorMeta, FloorSession};

/// The instant the whole census runs at — the drive's fold and the observe
/// below MUST share it, or every sprite is judged mid-entry-walk.
fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

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
    /// The decoder PANICKED — the never-panic contract violated.
    panics: Vec<LineFailure>,
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
fn check_file(source: &str, path: &Path, pack: &Pack) -> Verdict {
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
    v.newest_activity = newest_activity(source, body.as_bytes());

    // `.seeded()` is what makes the rest meaningful: the watcher's
    // `emit_first_sight` is what registers a transcript in production, and a
    // JSONL event for an unknown id is a documented no-op. The seed is keyed by
    // the source's OWN registry row, so it lands on the same `AgentId` the
    // decoded lines do — this harness's first run reported 0/4376 registered
    // for want of exactly that.
    let Some(drive) = Drive::transcript(source, &path.to_string_lossy()) else {
        return v;
    };
    let driven = drive.seeded().at(now()).lines(body.lines());

    v.lines = driven.lines;
    v.unparseable = driven.unparseable;
    v.decode_errors = driven.decode_errors.len();
    v.events = driven.events.len();
    v.registered = driven.registered();
    v.panics = driven.panics;
    if v.registered == 0 {
        return v;
    }

    // The UI-layer half: `observe` is the headless seam whose `characters` are
    // the resolved sprites the painter would draw. One `FloorSession` per file
    // so no cross-file render state can carry a verdict.
    let mut session = FloorSession::new();
    v.drawn = session
        .observe(&driven.scene, pack, 192, 80, FloorMeta::ground(), now())
        .map_or(0, |frame| frame.characters.len());
    v
}

/// The newest turn this file's SESSION wrote, epoch seconds — read with the
/// source's own `ActivityRecency` over the raw bytes, exactly as the first-sight
/// gate reads a tail (it returns the newest stamp across whatever buffer it is
/// given, so the whole body yields the file's newest turn).
///
/// Only CC has a published answer today (its `ACTIVITY_TYPES`); every other
/// source reports no stamp, which shows up as an empty provenance column rather
/// than a wrong one. Kept in this shell rather than the registry because it has
/// exactly one consumer besides the watcher and it feeds a REPORT, not a
/// contract — when a second source publishes an activity clock, the row is the
/// place for it (see `registry`'s header on what earns a column).
fn newest_activity(source: &str, body: &[u8]) -> Option<u64> {
    if source != pixtuoid_core::source::claude_code::SOURCE_NAME {
        return None;
    }
    match pixtuoid_core::source::claude_code::cc_activity_recency(body) {
        TailActivity::At(secs) => Some(secs),
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

    // Refuse up front for the same reason the fuzz shell does: a source with no
    // transcript decoder would otherwise report a clean census of nothing.
    if Drive::transcript(source, "/probe.jsonl").is_none() {
        let known: Vec<&str> = registry::registered_source_names().collect();
        eprintln!(
            "error: {source:?} is not a transcript-bearing registered source \
             (registered: {})",
            known.join(", ")
        );
        std::process::exit(2);
    }

    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    if files.is_empty() {
        eprintln!("error: no .jsonl under {}", root.display());
        std::process::exit(2);
    }

    let pack = load_sprite_pack(None).expect("embedded pack");
    let mut totals = Verdict::default();
    let mut registered_files = 0usize;
    let mut drawn_files = 0usize;
    let mut gap_buckets: BTreeMap<&str, usize> = BTreeMap::new();
    let mut error_files: Vec<(PathBuf, usize)> = Vec::new();
    let mut panic_files: Vec<(PathBuf, LineFailure)> = Vec::new();

    for f in &files {
        let v = check_file(source, f, &pack);
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
        if let Some(p) = v.panics.first() {
            panic_files.push((f.clone(), p.clone()));
        }
        totals.panics.extend(v.panics.iter().cloned());
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
            r#"{{"source":"{source}","files":{},"lines":{},"unparseable":{},"decode_errors":{},"panics":{},"events":{},"registered_files":{registered_files},"drawn_files":{drawn_files}}}"#,
            files.len(),
            totals.lines,
            totals.unparseable,
            totals.decode_errors,
            totals.panics.len(),
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
        println!("  PANICS            {}  <- must be 0", totals.panics.len());
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
        // Paths + line SHAPES only — a transcript line carries real prose/code.
        for (p, n) in error_files.iter().take(10) {
            println!("  decode error x{n}: {}", p.display());
        }
        for (p, f) in panic_files.iter().take(10) {
            println!("  PANIC {}: {f}", p.display());
        }
    }

    // The ONLY hard failures: a decoder that PANICS on bytes its own source
    // wrote (it would take the whole watcher down), or one that returns Err
    // there (the contract is log-and-continue). Everything else is a census,
    // because corpus content is unbounded.
    if totals.decode_errors > 0 || !totals.panics.is_empty() {
        std::process::exit(1);
    }
}
