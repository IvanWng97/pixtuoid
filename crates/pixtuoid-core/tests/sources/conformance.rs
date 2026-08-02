//! Golden-fixture decode + coalescing harness.
//!
//! One of the four `harness::Drive` shells: bytes come from the COMMITTED
//! fixture dirs (`tests/sources/fixtures/<source>/<scenario>/`) and the verdict
//! is an assertion — the full decoded `AgentEvent` sequence is snapshotted
//! (insta yaml), and every decoded event must share ONE `AgentId` (the
//! hook↔JSONL coalescing contract that keeps regressing; a mismatch = two
//! sprites for one session).
//!
//! Each transport is one drive: the transcript through `Drive::transcript`
//! (the source's registry `LineDecoder`, `Transport::Jsonl`) and the hook
//! payloads through `Drive::hooks` (the shared dispatcher, `Transport::Hook`).
//! Neither is SEEDED — this harness asserts what the WIRE alone produces, so a
//! transcript that registers nothing on its own is a fact the snapshot shows
//! rather than one a seed hides.
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

use pixtuoid_core::harness::{Drive, Driven};
use pixtuoid_core::source::{registry, AgentEvent};

/// Hook-only-ness comes from the registry row (`line_decoder()` is `None`), never
/// a harness-side list — a second list could mark a JSONL source hook-only and
/// pass the harness without its LineDecoder ever running ("registration is
/// not coverage").
fn is_hook_only(source: &str) -> bool {
    registry::descriptor_for(source).is_some_and(|d| d.line_decoder().is_none())
}

/// Daemon sources (`SourceKind::Daemon` in the registry) decode to ZERO
/// AgentEvents — their `presence_decoder` claims all but presence rides a sibling
/// channel into `SceneState::daemons` (the OpenClaw daemon fixture). The
/// coalesce-to-one-AgentId contract doesn't apply (no agent slots).
fn is_daemon(source: &str) -> bool {
    registry::descriptor_for(source).is_some_and(|d| d.is_daemon())
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

/// One fixture's drives, split by transport so the test can assert each side
/// actually contributed (a degenerate all-no-op transcript must not pass
/// coalescing on hooks alone).
struct Decoded {
    /// The transcript drive — `None` for a hook-only source (no transcript).
    jsonl: Option<Driven>,
    /// The hook drive — `None` when the scenario ships no `hook-payloads.jsonl`.
    hooks: Option<Driven>,
    /// The hook payload LINES as committed (after `{{TRANSCRIPT_PATH}}`
    /// substitution), so a presence-only source can pin its OWN field-reading
    /// decoder against the byte-real fixture — a daemon's `hooks` drive decodes
    /// to zero `AgentEvent`s by design.
    hook_lines: Vec<String>,
}

impl Decoded {
    /// Every decoded event, transcript side first — the snapshot's order.
    fn events(&self) -> Vec<AgentEvent> {
        self.jsonl
            .iter()
            .chain(self.hooks.iter())
            .flat_map(|d| d.events.iter().cloned())
            .collect()
    }
}

/// A committed fixture is bytes WE control, so every line must parse and decode
/// cleanly — an `Err` here is a decoder regression and a panic is the
/// never-panic contract broken, both of which a count-only harness would let
/// through as "fewer events than expected".
fn assert_drove_cleanly(d: &Driven, what: &str, at: &std::path::Path) {
    assert_eq!(
        d.unparseable,
        0,
        "{what} in {}: {} line(s) are not valid JSON",
        at.display(),
        d.unparseable
    );
    assert!(
        d.decode_errors.is_empty(),
        "{what} in {}: decode error(s): {:?}",
        at.display(),
        d.decode_errors
    );
    assert!(
        d.panics.is_empty(),
        "{what} in {}: the decoder PANICKED (never-panic contract): {:?}",
        at.display(),
        d.panics
    );
}

/// Decode one fixture dir, feeding the decoders the fixture's *relative* path as
/// the transcript key — `AgentId` is a deterministic FNV hash of that key, so
/// snapshots stay machine-independent.
/// A scenario's transcripts: the non-hook `.jsonl` files, sorted. Exactly one
/// for a JSONL-bearing source — two would make selection (and the snapshot)
/// depend on `read_dir` order, zero would skip its LineDecoder entirely — and
/// ZERO for a hook-only source (`transcript: None` in its registry row), which
/// is the only kind that may ship none.
fn transcripts_in(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().and_then(|s| s.to_str()) == Some("jsonl")
                && p.file_name().and_then(|s| s.to_str()) != Some("hook-payloads.jsonl")
        })
        .collect();
    out.sort();
    out
}

/// The one transcript a JSONL-bearing scenario ships.
fn transcript_in(dir: &Path) -> PathBuf {
    let mut t = transcripts_in(dir);
    assert_eq!(t.len(), 1, "{} must ship one transcript", dir.display());
    t.remove(0)
}

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
    let transcripts = transcripts_in(dir);
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

    let jsonl = transcripts.first().map(|transcript| {
        let drive = Drive::transcript(source, &logical).unwrap_or_else(|| {
            panic!(
                "fixture source {source:?} has no line_decoder — add/extend its \
                 SourceDescriptor row in source/registry.rs"
            )
        });
        let driven = drive.lines(read_lines(transcript));
        assert_drove_cleanly(&driven, "transcript", transcript);
        driven
    });

    let hooks_path = dir.join("hook-payloads.jsonl");
    // `{{TRANSCRIPT_PATH}}` lets a path-keyed hook (CC) line up with its
    // transcript; Codex carries it too, to prove it's ignored.
    let hook_lines: Vec<String> = if hooks_path.exists() {
        read_lines(&hooks_path)
            .into_iter()
            .map(|l| l.replace("{{TRANSCRIPT_PATH}}", &logical))
            .collect()
    } else {
        Vec::new()
    };
    // A scenario with no hook file drives nothing; an EMPTY one still drives
    // (and the daemon arm's non-empty check below is what catches it).
    let hooks = hooks_path.exists().then(|| {
        // One payload can decode to multiple events (Identity attached ahead of
        // a tool/permission event, #221).
        let driven = Drive::hooks(source).lines(&hook_lines);
        assert_drove_cleanly(&driven, "hook payloads", &hooks_path);
        driven
    });

    Decoded {
        jsonl,
        hooks,
        hook_lines,
    }
}

/// The WATCHER half of coalescing, over the same byte-real fixtures: a
/// first-sight seed keyed by the source's registry row must land on the SAME
/// `AgentId` that source's own decoder derives from the transcript it is
/// reading. A row wired to the wrong deriver registers one agent while every
/// decoded line lands on another — two sprites for one session, and (in an
/// offline driver) a census that reports "parsed but never rendered".
///
/// `all_source_fixtures_decode_and_coalesce` cannot see this: it drives the
/// wire ALONE, so the seed — the thing production actually registers with — is
/// never exercised. This is also the windows-test catch for the path-key fold
/// (`transcript_at`): on Unix `normalize_path_key` is the identity, so a raw
/// vs normalized key divergence is invisible locally.
#[test]
fn a_seeded_drive_coalesces_with_each_transcripts_own_decoder() {
    let root = fixtures_root();
    let mut ran = 0;
    for source_dir in sorted_dirs(&root) {
        let source = source_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if is_hook_only(&source) || is_daemon(&source) {
            continue;
        }
        for scenario_dir in sorted_dirs(&source_dir) {
            let transcript = transcript_in(&scenario_dir);
            let driven = Drive::transcript_at(&source, &transcript)
                .unwrap_or_else(|| panic!("{source}: no transcript decoder"))
                .seeded()
                .lines(read_lines(&transcript));
            assert_drove_cleanly(&driven, "seeded transcript", &transcript);

            let ids: BTreeSet<_> = driven.events.iter().map(AgentEvent::agent_id).collect();
            assert!(
                driven.events.len() >= 2,
                "{source}: the seed plus this transcript's own events, got {:?}",
                driven.events
            );
            assert_eq!(
                ids.len(),
                1,
                "{source}/{}: the first-sight seed and the decoded lines must be ONE agent \
                 — the registry row's id deriver disagrees with this source's own decoder \
                 keying, got {ids:?}",
                scenario_dir.file_name().unwrap().to_string_lossy(),
            );
            assert_eq!(
                driven.registered(),
                1,
                "{source}: a seeded transcript must register exactly one slot"
            );
            ran += 1;
        }
    }
    assert!(
        ran > 0,
        "no transcript fixtures found under {}",
        root.display()
    );
}

/// Every registered source MUST ship a coalescing fixture. Without this,
/// `all_source_fixtures_decode_and_coalesce` only covers sources that happen to
/// have a dir — a contributor could register a new CLI (decoder + label prefix)
/// and ship a broken decoder while the harness stays green. Registration is not
/// coverage; this makes the fixture mandatory.
#[test]
fn every_registered_source_has_a_coalescing_fixture() {
    let root = fixtures_root();
    for src in registry::registered_source_names() {
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
            let events = d.events();

            // DAEMON (OpenClaw): the presence_decoder claims every event but
            // emits ZERO AgentEvents — presence rides a sibling channel into
            // SceneState::daemons. The fixture must still ship hooks (so the
            // decoder runs + can't panic) and decode to NO AgentEvents (the
            // by-design emptiness `is_daemon` guards). The contribution +
            // coalesce contracts below don't apply (no agent slots).
            if is_daemon(&source) {
                assert!(
                    !d.hook_lines.is_empty(),
                    "{source}/{scenario}: a daemon source must ship a NON-EMPTY \
                     hook-payloads.jsonl (an empty fixture passes the zero-events check vacuously)"
                );
                assert!(
                    events.is_empty(),
                    "{source}/{scenario}: a daemon source must decode to ZERO AgentEvents \
                     (presence rides the sibling channel), got {events:?}"
                );
                // Byte-real PIN for the field-reading presence decoder (openclaw is
                // the only presence-only source): the captured fixture must decode
                // to a non-empty set of presence deltas — so a wire field rename
                // (`runId`→`run_id`) FAILS here, not just the synthetic units that
                // hardcode the same names. Matches the byte-real-pin standard
                // (Copilot #294 / CodeWhale #276).
                // The PRESENCE lane, not the agent one: these payloads ride the
                // sibling channel, so they are decoded here from the committed
                // lines rather than through `Drive` (whose `AgentEvent` output
                // for a daemon is empty by construction, asserted just above).
                let decoded: Vec<_> = d
                    .hook_lines
                    .iter()
                    .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                    .filter_map(|v| {
                        pixtuoid_core::source::openclaw::decode_openclaw_hook_payload(&v).ok()
                    })
                    .collect();
                assert!(
                    decoded.iter().any(|d| !d.updates.is_empty()),
                    "{source}/{scenario}: the byte-real fixture decoded to ZERO presence deltas \
                     — the presence decoder's field names drifted from the captured wire format"
                );
                // The gateway IDENTITY is part of that byte-real pin: every event
                // must resolve to ONE real instance (never the stale-plugin
                // fallback), so a `gatewayPort` rename fails here too — and a
                // fixture that forgot the field can't pass vacuously.
                let instances: std::collections::BTreeSet<_> =
                    decoded.iter().map(|d| d.instance.as_str()).collect();
                assert_eq!(
                    instances.len(),
                    1,
                    "{source}/{scenario}: one captured gateway must resolve to exactly one \
                     instance id, got {instances:?}"
                );
                assert!(
                    instances
                        .iter()
                        .all(|i| i.chars().all(|c| c.is_ascii_digit())),
                    "{source}/{scenario}: the captured wire must carry a real gateway port \
                     (got {instances:?} — the stale-plugin fallback is not a byte-real pin)"
                );
                insta::assert_yaml_snapshot!(format!("{source}__{scenario}"), events);
                ran += 1;
                continue;
            }

            // Each present transport must actually contribute — else a
            // degenerate fixture (e.g. all-no-op JSONL) could pass coalescing
            // on hooks alone, silently skipping the keying path this guards.
            // A hook-only source ships no transcript and must then ship hooks.
            if is_hook_only(&source) {
                assert!(
                    d.hooks.as_ref().is_some_and(|h| !h.events.is_empty()),
                    "{source}/{scenario}: a hook-only source's scenario must ship a non-empty hook-payloads.jsonl"
                );
            } else {
                assert!(
                    d.jsonl.as_ref().is_some_and(|j| !j.events.is_empty()),
                    "{source}/{scenario}: transcript decoded to ZERO events"
                );
            }
            if let Some(hooks) = &d.hooks {
                assert!(
                    !hooks.events.is_empty(),
                    "{source}/{scenario}: hook-payloads.jsonl decoded to ZERO events"
                );
            }

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
