//! Golden-fixture decode + coalescing harness.
//!
//! For each `tests/sources/fixtures/<source>/<scenario>/` directory, decode the
//! transcript lines (via the source's `LineDecoder`) and the hook payloads
//! (via `decode_hook_payload`), then:
//!   1. snapshot the full decoded `AgentEvent` sequence (insta yaml), and
//!   2. assert every decoded event shares ONE `AgentId` — the hook↔JSONL
//!      coalescing contract that keeps regressing (a mismatch = two sprites
//!      for one session).
//!
//! Adding a CLI = drop a fixture dir; the decoder comes from the source's
//! `SourceDescriptor` row in `source/registry.rs` — no harness edit. Run
//! `cargo insta review` to accept the new snapshot.
//!
//! Snapshots stay portable because the decoder is fed the fixture's *relative*
//! path (a stable logical key), not the machine-specific absolute path —
//! `AgentId` is a deterministic FNV-1a hash of that key.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use pixtuoid_core::source::decoder::decode_hook_payload;
use pixtuoid_core::source::jsonl::LineDecoder;
use pixtuoid_core::source::{registry, AgentEvent, REGISTERED_SOURCES};

/// A fixture source's JSONL line decoder, from the source registry. A
/// hook-only source (`line_decoder: None`) ships no transcript and never
/// reaches this fn (`is_hook_only` gates the transcript requirement).
fn decoder_for(source: &str) -> LineDecoder {
    registry::descriptor_for(source)
        .and_then(|d| d.line_decoder)
        .unwrap_or_else(|| {
            panic!(
                "fixture source {source:?} has no line_decoder — add/extend its \
                 SourceDescriptor row in source/registry.rs"
            )
        })
}

/// Hook-only-ness comes from the registry row (`line_decoder: None`), never a
/// harness-side list — a second list could mark a JSONL source hook-only and
/// pass the harness without its LineDecoder ever running ("registration is
/// not coverage").
fn is_hook_only(source: &str) -> bool {
    registry::descriptor_for(source).is_some_and(|d| d.line_decoder.is_none())
}

fn fixtures_root() -> PathBuf {
    // Conformance scenarios ONLY — every dir here must be a registered source
    // (decode_fixture asserts it). Single-owner fixtures (decode's hooks/jsonl,
    // codex's lifecycle payloads) live with their module, NOT here.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sources/fixtures")
}

fn read_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .lines()
        .map(str::to_string)
        .filter(|l| !l.trim().is_empty())
        .collect()
}

fn sorted_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

/// One fixture's decoded events, split by transport so the test can assert each
/// side actually contributed (a degenerate all-no-op transcript must not pass
/// coalescing on hooks alone).
struct Decoded {
    jsonl: Vec<AgentEvent>,
    hooks: Vec<AgentEvent>,
    had_hook_file: bool,
}

/// Decode one fixture dir, feeding the decoders the fixture's *relative* path as
/// the transcript key — `AgentId` is a deterministic FNV hash of that key, so
/// snapshots stay machine-independent.
fn decode_fixture(source: &str, dir: &Path) -> Decoded {
    // Catch the dir-name-typo / removed-source cases up front — otherwise
    // they'd be misdiagnosed as "JSONL-bearing, found 0" (a false claim about
    // an unregistered name) or "add a SourceDescriptor row" (when the right
    // action is deleting the stale dir).
    assert!(
        registry::descriptor_for(source).is_some(),
        "fixture dir {source:?} matches no SourceDescriptor row — dir-name typo, \
         or a removed source whose fixtures should be deleted"
    );
    // The transcript is the lone non-hook .jsonl in the dir. Exactly one for a
    // JSONL-bearing source — two would make selection (and the snapshot)
    // depend on read_dir order, zero would skip its LineDecoder entirely. A
    // hook-only source (`line_decoder: None` in its registry row) must ship
    // ZERO transcripts — and ONLY it may.
    let mut transcripts: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().and_then(|s| s.to_str()) == Some("jsonl")
                && p.file_name().and_then(|s| s.to_str()) != Some("hook-payloads.jsonl")
        })
        .collect();
    transcripts.sort();
    let expected = if is_hook_only(source) { 0 } else { 1 };
    assert_eq!(
        transcripts.len(),
        expected,
        "{} must contain exactly {expected} transcript .jsonl (source {source:?} is {}), found {}",
        dir.display(),
        if expected == 0 {
            "hook-only"
        } else {
            "JSONL-bearing"
        },
        transcripts.len()
    );

    // Hook-only scenarios key the {{TRANSCRIPT_PATH}} substitution on the
    // scenario dir instead (stable + machine-independent, same property).
    // Separators are normalized to '/' so the key — and therefore every
    // AgentId hash baked into the snapshots — is byte-identical on Windows
    // (where strip_prefix yields backslash-separated components).
    let logical = transcripts
        .first()
        .map_or(dir, PathBuf::as_path)
        .strip_prefix(fixtures_root())
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");

    let mut jsonl = Vec::new();
    if let Some(transcript) = transcripts.first() {
        let decode = decoder_for(source);
        for line in read_lines(transcript) {
            let v: serde_json::Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("bad json in {}: {e}", transcript.display()));
            match decode(&logical, source, v) {
                Ok(evs) => jsonl.extend(evs),
                Err(e) => panic!("decode error in {}: {e}", transcript.display()),
            }
        }
    }

    let hooks_path = dir.join("hook-payloads.jsonl");
    let had_hook_file = hooks_path.exists();
    let mut hooks = Vec::new();
    if had_hook_file {
        for line in read_lines(&hooks_path) {
            // `{{TRANSCRIPT_PATH}}` lets a path-keyed hook (CC) line up with its
            // transcript; Codex carries it too, to prove it's ignored.
            let line = line.replace("{{TRANSCRIPT_PATH}}", &logical);
            let v: serde_json::Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("bad hook json in {}: {e}", hooks_path.display()));
            match decode_hook_payload(v) {
                Ok(ev) => hooks.push(ev),
                Err(e) => panic!("hook decode error in {}: {e}", hooks_path.display()),
            }
        }
    }
    Decoded {
        jsonl,
        hooks,
        had_hook_file,
    }
}

/// Every registered source MUST ship a coalescing fixture. Without this,
/// `all_source_fixtures_decode_and_coalesce` only covers sources that happen to
/// have a dir — a contributor could register a new CLI (decoder + label prefix)
/// and ship a broken decoder while the harness stays green. Registration is not
/// coverage; this makes the fixture mandatory.
#[test]
fn every_registered_source_has_a_coalescing_fixture() {
    let root = fixtures_root();
    for src in REGISTERED_SOURCES {
        let dir = root.join(src);
        let shape = if is_hook_only(src) {
            "hook-payloads.jsonl ONLY (hook-only row)"
        } else {
            "transcript.jsonl [+ hook-payloads.jsonl]"
        };
        assert!(
            dir.is_dir(),
            "registered source {src:?} has no fixture dir {} — add a coalescing fixture ({shape})",
            dir.display()
        );
        assert!(
            !sorted_dirs(&dir).is_empty(),
            "registered source {src:?} fixture dir {} has no scenario subdir",
            dir.display()
        );
    }
}

#[test]
fn all_source_fixtures_decode_and_coalesce() {
    let root = fixtures_root();
    let mut ran = 0;
    for source_dir in sorted_dirs(&root) {
        let source = source_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        for scenario_dir in sorted_dirs(&source_dir) {
            let scenario = scenario_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let d = decode_fixture(&source, &scenario_dir);

            // Each present transport must actually contribute — else a
            // degenerate fixture (e.g. all-no-op JSONL) could pass coalescing
            // on hooks alone, silently skipping the keying path this guards.
            // A hook-only source ships no transcript and must then ship hooks.
            if is_hook_only(&source) {
                assert!(
                    d.had_hook_file && !d.hooks.is_empty(),
                    "{source}/{scenario}: a hook-only source's scenario must ship a non-empty hook-payloads.jsonl"
                );
            } else {
                assert!(
                    !d.jsonl.is_empty(),
                    "{source}/{scenario}: transcript decoded to ZERO events"
                );
            }
            if d.had_hook_file {
                assert!(
                    !d.hooks.is_empty(),
                    "{source}/{scenario}: hook-payloads.jsonl decoded to ZERO events"
                );
            }

            let events: Vec<AgentEvent> = d.jsonl.iter().chain(d.hooks.iter()).cloned().collect();

            // Contract 1: the decoded event sequence is stable (golden snapshot).
            insta::assert_yaml_snapshot!(format!("{source}__{scenario}"), events);

            // Contract 2: hook + JSONL events for one session coalesce to ONE
            // AgentId. This is the dup-sprite bug class — assert it directly.
            let ids: BTreeSet<_> = events.iter().map(|e| e.agent_id()).collect();
            assert_eq!(
                ids.len(),
                1,
                "{source}/{scenario}: hook+JSONL events must coalesce to ONE agent_id, got {}: {:?}",
                ids.len(),
                ids
            );
            ran += 1;
        }
    }
    assert!(ran > 0, "no fixtures found under {}", root.display());
}
