//! Golden-fixture decode + coalescing harness: bytes come from the COMMITTED
//! fixture dirs (`tests/sources/fixtures/<source>/<scenario>/`), the decoded
//! `AgentEvent` sequence is snapshotted (insta yaml), and every decoded event
//! must share ONE `AgentId` (a mismatch = two sprites for one session).
//!
//! Neither drive is SEEDED — this harness asserts what the WIRE alone produces,
//! so a transcript that registers nothing on its own is a fact the snapshot
//! shows rather than one a seed hides.
//!
//! Adding a CLI = drop a fixture dir; the decoder comes from the source's
//! `SourceDescriptor` row in `source/registry.rs` — no harness edit. Snapshots
//! stay portable because the decoder is fed the fixture's *relative* path, and
//! `AgentId` is a deterministic hash of that key.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use pixtuoid_core::harness::{Drive, Driven};
use pixtuoid_core::source::{registry, AgentEvent};

/// Hook-only-ness comes from the registry row, never a harness-side list — a
/// second list could mark a JSONL source hook-only and pass the harness without
/// its LineDecoder ever running.
fn is_hook_only(source: &str) -> bool {
    registry::descriptor_for(source).is_some_and(|d| d.line_decoder().is_none())
}

/// Daemon sources decode to ZERO AgentEvents — presence rides a sibling channel
/// into `SceneState::daemons`, so the coalesce-to-one-AgentId contract doesn't
/// apply (no agent slots).
fn is_daemon(source: &str) -> bool {
    registry::descriptor_for(source).is_some_and(|d| d.is_daemon())
}

fn fixtures_root() -> PathBuf {
    // Conformance scenarios ONLY — single-owner fixtures live with their
    // module, NOT here.
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

struct Decoded {
    jsonl: Option<Driven>,
    hooks: Option<Driven>,
    /// The hook payload LINES as committed, so a presence-only source can pin
    /// its own decoder against the byte-real fixture (its `hooks` drive decodes
    /// to zero `AgentEvent`s by design).
    hook_lines: Vec<String>,
}

impl Decoded {
    fn events(&self) -> Vec<AgentEvent> {
        self.jsonl
            .iter()
            .chain(self.hooks.iter())
            .flat_map(|d| d.events.iter().cloned())
            .collect()
    }
}

/// A scenario's transcripts: the non-hook `.jsonl` files, sorted. Exactly one
/// for a JSONL-bearing source — two would make selection (and the snapshot)
/// depend on `read_dir` order, zero would skip its LineDecoder entirely.
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

fn transcript_in(dir: &Path) -> PathBuf {
    let mut t = transcripts_in(dir);
    assert_eq!(t.len(), 1, "{} must ship one transcript", dir.display());
    t.remove(0)
}

fn decode_fixture(source: &str, dir: &Path) -> Decoded {
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

    // Hook-only scenarios key the substitution on the scenario dir instead.
    // Separators are normalized to '/' so the key — and therefore every AgentId
    // hash baked into the snapshots — is byte-identical on Windows.
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
        driven.assert_clean(&format!("transcript {}", transcript.display()));
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
    let hooks = hooks_path.exists().then(|| {
        let driven = Drive::hooks().lines(&hook_lines);
        driven.assert_clean(&format!("hook payloads {}", hooks_path.display()));
        driven
    });

    Decoded {
        jsonl,
        hooks,
        hook_lines,
    }
}

/// The WATCHER half of coalescing: `all_source_fixtures_decode_and_coalesce`
/// drives the wire ALONE, so the first-sight seed — the thing production
/// actually registers with — is never exercised there. This is also the
/// windows-only catch for the path-key fold: on Unix `normalize_path_key` is
/// the identity, so a raw vs normalized key divergence is invisible locally.
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
            driven.assert_clean(&format!("seeded transcript {}", transcript.display()));

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

/// Without this, `all_source_fixtures_decode_and_coalesce` only covers sources
/// that happen to have a dir — a contributor could register a new CLI and ship a
/// broken decoder while the harness stays green. Registration is not coverage.
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

/// Hook-only sources with no recorded scenario. The list only SHRINKS — a hook
/// event is transient, so for these the fixture is the only wire evidence there
/// will ever be, and a new hook-only CLI must not join it by default.
const NO_WIRE_EVIDENCE_YET: &[&str] = &["codewhale", "openclaw", "reasonix"];

/// The scenarios that predate the provenance rule and read like real sessions
/// without a capture record to say so. `unknown` is an admission, not a third
/// way to leave a fixture unexplained, so the set is pinned: a NEW fixture is
/// `recorded` or `composed`, and re-recording one of these deletes its entry.
const UNVERIFIED_PROVENANCE: &[&str] = &[
    "antigravity/tool-run",
    "copilot/permission",
    "copilot/tool-run",
    "omp/2026-07-10T18-00-00-000Z_01990000-0000-7000-8000-000000000001",
    "omp/ask-round",
    "omp/tool-run",
];

/// Every scenario declares where its bytes came from, because nothing IN them
/// separates a capture from a composition — a redacted cwd and an invented one
/// look alike. A composed fixture pins its author's belief and the decoder then
/// agrees with it: kimi's shipped four confident per-call ids for a field kimi
/// never sends.
#[test]
fn every_scenario_declares_its_provenance() {
    let root = fixtures_root();
    let mut recorded: BTreeSet<String> = BTreeSet::new();
    let mut unknown: BTreeSet<String> = BTreeSet::new();
    for source_dir in sorted_dirs(&root) {
        let source = source_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        for scenario_dir in sorted_dirs(&source_dir) {
            let path = scenario_dir.join("provenance.json");
            let body = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!(
                    "{}: {e}\n  every scenario declares an origin: recorded | composed | unknown",
                    path.display()
                )
            });
            let doc: serde_json::Value =
                serde_json::from_str(&body).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let origin = doc
                .get("origin")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let required: &[&str] = match origin {
                "recorded" => &["cli", "version", "captured", "command"],
                "composed" | "unknown" => &["note"],
                other => panic!(
                    "{}: origin {other:?} is not recorded | composed | unknown",
                    path.display()
                ),
            };
            for key in required {
                assert!(
                    doc.get(key)
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|s| !s.trim().is_empty()),
                    "{}: origin {origin:?} requires a non-empty {key:?}",
                    path.display()
                );
            }
            match origin {
                "recorded" => {
                    recorded.insert(source.clone());
                }
                "unknown" => {
                    let name = scenario_dir.file_name().unwrap().to_string_lossy();
                    unknown.insert(format!("{source}/{name}"));
                }
                _ => {}
            }
        }
    }
    assert_eq!(
        unknown,
        UNVERIFIED_PROVENANCE
            .iter()
            .map(|s| (*s).to_string())
            .collect::<BTreeSet<_>>(),
        "the `unknown` set is pinned — record one and drop its entry, or explain a new \
         fixture as recorded|composed"
    );
    for src in registry::registered_source_names().filter(|s| is_hook_only(s)) {
        assert_eq!(
            NO_WIRE_EVIDENCE_YET.contains(&src),
            !recorded.contains(src),
            "{src} is hook-only with recorded={}; either record a scenario and drop its \
             NO_WIRE_EVIDENCE_YET entry, or add one",
            recorded.contains(src)
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

            insta::assert_yaml_snapshot!(format!("{source}__{scenario}"), events);

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
