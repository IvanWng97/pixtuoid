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
    let mut out = Vec::new();
    push_transcripts(dir, &mut out);
    out.sort();
    out
}

/// Recurses, because a path-keyed source needs the fixture to REPRODUCE the
/// shape its id comes from: grok keys on the parent-dir name, so a flat
/// `<scenario>/updates.jsonl` yields the scenario name as the session id, and
/// the composed fixture that "passed" the coalesce assertion did so only by
/// declaring `sessionId: "permission-flow"` in its hooks to match.
fn push_transcripts(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().filter_map(Result::ok) {
        let p = e.path();
        if p.is_dir() {
            push_transcripts(&p, out);
        } else if p.extension().and_then(|s| s.to_str()) == Some("jsonl")
            && p.file_name().and_then(|s| s.to_str()) != Some("hook-payloads.jsonl")
        {
            out.push(p);
        }
    }
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
const NO_WIRE_EVIDENCE_YET: &[&str] = &[];

/// The scenarios that predate the provenance rule and read like real sessions
/// without a capture record to say so. `unknown` is an admission, not a third
/// way to leave a fixture unexplained, so the set is pinned: a NEW fixture is
/// `recorded` or `composed`, and re-recording one of these deletes its entry.
const UNVERIFIED_PROVENANCE: &[&str] = &[
    // Not re-recordable: its cwd is a real Windows path with a space and parens,
    // which is what makes it the Windows arm of the cwd-extractor test.
    "copilot/tool-run",
];

/// Sources whose `unknown` scenarios now sit BESIDE a recorded one, so the
/// decoder is pinned against bytes nobody composed even where the older fixture's
/// origin stays unprovable.
const UNKNOWN_BUT_BACKED_BY_A_CAPTURE: &[&str] = &["copilot"];

/// A `recorded` fixture whose bytes were EDITED must say so. Nothing in the
/// bytes separates a capture from a composition — which is the whole reason
/// provenance exists — so a redaction sentinel with a silent `note` is the one
/// state the mechanism cannot tolerate. Sentinels, not a `/Users/dev` grep: the
/// sweep that keyed on that alone missed kimi's, whose redaction is the owner
/// column inside a captured `ls -la` and is the case the README names.
#[test]
fn a_recorded_capture_that_was_edited_says_so() {
    // A word ending the clause right before "redact" that inverts it.
    const NEGATIONS: &[&str] = &["no", "not", "nothing", "never", "without"];
    const SENTINELS: &[&str] = &[
        "/Users/dev",
        " dev  wheel",
        " dev  staff",
        "[redacted",
        "dev@",
    ];
    let mut silent = Vec::new();
    let sources = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sources");
    let mut stack = vec![sources.clone()];
    while let Some(dir) = stack.pop() {
        let prov = dir.join("provenance.json");
        if let Ok(body) = std::fs::read_to_string(&prov) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                if v.get("origin").and_then(|o| o.as_str()) == Some("recorded") {
                    let note = v
                        .get("note")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    // "verbatim" USED to satisfy this, which let a note assert the
                    // OPPOSITE of what the bytes show. A bare `contains` re-opens
                    // that hole one word over — "unredacted", "nothing redacted".
                    let declares = note.match_indices("redact").any(|(at, _)| {
                        let before = note[..at].trim_end();
                        !before.ends_with("un") && !NEGATIONS.iter().any(|n| before.ends_with(n))
                    });
                    // RECURSIVE: a parent-dir-keyed source nests its transcript one
                    // level down, and `a_parent_dir_keyed_transcript_is_nested_under_
                    // its_session_id` FORCES new fixtures into that shape — so a flat
                    // read_dir was blind to exactly the fixtures the sibling gate makes.
                    let mut bytes = transcripts_in(&dir);
                    bytes.push(dir.join("hook-payloads.jsonl"));
                    let edited = bytes.iter().any(|p| {
                        std::fs::read_to_string(p)
                            .map(|b| SENTINELS.iter().any(|s| b.contains(s)))
                            .unwrap_or(false)
                    });
                    if edited && !declares {
                        silent.push(dir.strip_prefix(&sources).unwrap().to_path_buf());
                    }
                }
            }
        }
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            }
        }
    }
    assert!(
        silent.is_empty(),
        "these `recorded` fixtures carry a redaction sentinel and no `note` claiming a \
         redaction — a reader cannot tell an edited capture from an untouched one, and a \
         note calling these bytes verbatim is worse than a silent one: {silent:?}"
    );
}

/// A dotted-run major at or above this reads as a YEAR/date token rather than a
/// semver major. MIRRORS `doctor::parse_version`'s const of the same name; the
/// pair is pinned by `banner_version_matches_doctors_documented_cases`.
const IMPLAUSIBLE_MAJOR: u64 = 1000;

/// THE version in a `--version` banner, by `doctor::parse_version`'s rule:
/// prefer a `v`-prefixed run, else the first with a plausible major, else the
/// first. One token, not a set — `{Hermes, Agent, 0.20.1, 2026.8.13}` let a
/// registry pin the DATE, a git sha (`e8db854`), or the word `Agent`.
fn banner_version(line: &str) -> Option<&str> {
    let mut runs: Vec<(bool, u64, &str)> = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        let run = line[start..i].trim_end_matches('.');
        if !run.contains('.') {
            continue;
        }
        let Ok(major) = run.split('.').next().unwrap_or("").parse::<u64>() else {
            continue;
        };
        let v_prefixed = start > 0 && matches!(bytes[start - 1], b'v' | b'V');
        runs.push((v_prefixed, major, run));
    }
    runs.iter()
        .find(|(vp, ..)| *vp)
        .or_else(|| runs.iter().find(|(_, maj, _)| *maj < IMPLAUSIBLE_MAJOR))
        .or_else(|| runs.first())
        .map(|(.., run)| *run)
}

/// The cases `doctor::parse_version`'s own doc names, so the mirror above cannot
/// drift from the parser it copies.
#[test]
fn banner_version_matches_doctors_documented_cases() {
    for (banner, want) in [
        ("Built 2026.06.04 — v1.2.3", Some("1.2.3")),
        ("2026.06.04", Some("2026.06.04")),
        ("codex-cli 0.147.0", Some("0.147.0")),
        ("Hermes Agent v0.20.1 (2026.8.13)", Some("0.20.1")),
        ("grok 0.2.102 (ab5ebf69acec) [stable]", Some("0.2.102")),
        ("2026.08.11-e8db854", Some("2026.08.11")),
        ("omp/17.3.4", Some("17.3.4")),
        ("no version here", None),
    ] {
        assert_eq!(banner_version(banner), want, "{banner:?}");
    }
    let doctor = Path::new(env!("CARGO_MANIFEST_DIR")).join("../pixtuoid/src/doctor.rs");
    let body = std::fs::read_to_string(&doctor).expect("doctor.rs");
    assert!(
        body.contains(&format!("IMPLAUSIBLE_MAJOR: u64 = {IMPLAUSIBLE_MAJOR};")),
        "doctor.rs's IMPLAUSIBLE_MAJOR drifted from this mirror"
    );
}

/// `verified_version` means "the version whose wire we have SEEN". A recorded
/// scenario IS that sighting, so the two must not disagree — `doctor`'s
/// "newer than verified" warning is structurally silent at `unknown` and would
/// otherwise warn a user running the exact version we hold a capture from.
#[test]
fn a_recorded_capture_anchors_its_sources_verified_version() {
    let root = fixtures_root();
    for source_dir in sorted_dirs(&root) {
        let source = source_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let Some(d) = registry::descriptor_for(&source) else {
            continue;
        };
        // (captured, version), sorted so the newest sighting is last. ISO dates
        // sort lexically.
        let mut recorded: Vec<(String, String)> = Vec::new();
        let mut scenarios_with_a_recorded_origin = 0usize;
        for scenario in sorted_dirs(&source_dir) {
            let Ok(body) = std::fs::read_to_string(scenario.join("provenance.json")) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
                continue;
            };
            if v.get("origin").and_then(|o| o.as_str()) != Some("recorded") {
                continue;
            }
            let version = v.get("version").and_then(|x| x.as_str()).unwrap_or("");
            scenarios_with_a_recorded_origin += 1;
            if version.chars().any(|c| c.is_ascii_digit()) {
                let captured = v
                    .get("captured")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                recorded.push((captured, version.to_string()));
            }
        }
        // A source with captures but NO version among them cannot anchor anything,
        // and skipping it is how `verified_version: "0.0.0-A-LIE"` passed for
        // copilot: three recorded scenarios, all `version: "unknown"`. The field
        // means "the version whose wire we have SEEN", so where this tree holds no
        // such evidence it must say `unknown` rather than carry a confident number
        // no capture backs.
        if recorded.is_empty() {
            let unbacked = scenarios_with_a_recorded_origin > 0;
            if unbacked {
                assert_eq!(
                    d.verified_version, "unknown",
                    "{source}: every recorded capture here says `version: unknown`, so \
                     nothing in this tree anchors {:?}. Re-record with a version, or set \
                     the field to \"unknown\" — a number no capture backs is the state \
                     the field was defined to avoid.",
                    d.verified_version
                );
            }
            continue;
        }
        // `!= "unknown"` was the first cut and it could not be falsified: a
        // `verified_version` of "0.0.0-A-LIE" passed while the captures pinned
        // 2.1.233, and three of the anchors this rule exists to hold were STALE
        // rather than unknown, so they were never in its range at all. The
        // registry field holds a bare version; a provenance holds the CLI's whole
        // `--version` line, so the anchor must be one of its TOKENS — `contains`
        // let the stale prefix "0.14" anchor against a 0.147.0 capture.
        // The NEWEST capture, not `any`: hermes holds 0.20.0 and 0.20.1, and
        // `any` let the older one anchor forever — which is precisely the stale
        // pin this test's docstring says it prevents.
        recorded.sort();
        let newest = &recorded.last().expect("non-empty").1;
        assert_eq!(
            banner_version(newest),
            Some(d.verified_version),
            "{source}: `verified_version` is {:?}, but the newest recorded capture \
             ({newest:?}) pins {:?} — the field means \"the version whose wire we \
             have SEEN\", and the most recent sighting is the one that counts",
            d.verified_version,
            banner_version(newest).unwrap_or("nothing")
        );
    }
}

/// A source whose id comes from the transcript's PARENT DIR needs the fixture to
/// reproduce that dir, or the session id silently becomes the SCENARIO NAME. The
/// README states the rule; this is what makes it fail. The probe catches a
/// deriver that returns the parent VERBATIM; one that DERIVES from it (omp's
/// `stem_chain` shape) reads as filename-keyed and is skipped.
#[test]
fn a_parent_dir_keyed_transcript_is_nested_under_its_session_id() {
    let root = fixtures_root();
    for source_dir in sorted_dirs(&root) {
        let source = source_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if is_hook_only(&source) || is_daemon(&source) {
            continue;
        }
        let derive = registry::id_deriver_for(&source);
        // The probe names a dir that no scenario could be called; a filename-keyed
        // deriver ignores it, a parent-dir-keyed one hands it straight back.
        const PROBE: &str = "0000-parent-probe";
        if derive(Path::new(&format!("{PROBE}/x.jsonl"))) != PROBE {
            continue;
        }
        for scenario_dir in sorted_dirs(&source_dir) {
            let scenario = scenario_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            for t in transcripts_in(&scenario_dir) {
                let parent = t.parent().unwrap().file_name().unwrap().to_string_lossy();
                assert_ne!(
                    parent,
                    scenario,
                    "{}: {source} keys the session on the transcript's PARENT DIR, so a flat \
                     fixture makes the scenario name the session id — nest it under the real id",
                    t.display()
                );
            }
        }
    }
}

/// The conformance tree is not the only place captures live: sibling modules
/// keep their own and the gate above cannot see them. A capture tree declares
/// its origin wherever it sits.
#[test]
fn every_single_owner_capture_tree_declares_its_provenance() {
    let sources = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sources");
    for entry in sorted_dirs(&sources) {
        let fixtures = entry.join("fixtures");
        // `decode/` holds hand-built decoder inputs, not captures; `fixtures/`
        // itself is the conformance tree the sibling test already gates.
        let name = entry.file_name().unwrap().to_string_lossy().into_owned();
        if !fixtures.is_dir() || name == "decode" || name == "fixtures" {
            continue;
        }
        let own = fixtures.join("provenance.json");
        let nested: Vec<_> = sorted_dirs(&fixtures)
            .into_iter()
            .map(|d| d.join("provenance.json"))
            .collect();
        assert!(
            own.exists() || (!nested.is_empty() && nested.iter().all(|p| p.exists())),
            "{}: a capture tree must declare its origin — add provenance.json here, \
             or one per sub-tree",
            fixtures.display()
        );
        // The `cli` cross-check reached only the conformance tree, so for these
        // eight it stayed exactly what its own comment calls the problem: the one
        // required field nothing could falsify. Only `claude/` needs a mapping —
        // every other module dir IS its source id.
        for prov in std::iter::once(own).chain(nested) {
            let Ok(body) = std::fs::read_to_string(&prov) else {
                continue;
            };
            let Ok(doc) = serde_json::from_str::<serde_json::Value>(&body) else {
                continue;
            };
            if doc.get("origin").and_then(serde_json::Value::as_str) != Some("recorded") {
                continue;
            }
            let dir = prov
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy();
            let source = if dir == "fixtures" {
                &name
            } else {
                dir.as_ref()
            };
            let source = if source == "claude" {
                "claude-code"
            } else {
                source
            };
            cli_matches_its_trees_binary(source, &doc, &prov);
        }
    }
}

/// `cli` was the one required field nothing could falsify, so a provenance naming
/// a DIFFERENT CLI than the tree it sits in passed every gate. The registry's
/// probe argv[0] is the name the user types (`agy`, `cursor-agent`), which is what
/// a capture command starts with — so the tree itself answers the field.
fn cli_matches_its_trees_binary(source: &str, doc: &serde_json::Value, at: &Path) {
    let probe = registry::descriptor_for(source).and_then(|d| d.version_probe);
    let declared = doc.get("cli").and_then(serde_json::Value::as_str);
    if let (Some(probe), Some(cli)) = (probe, declared) {
        assert_eq!(
            cli,
            probe[0],
            "{}: `cli` is {cli:?} but this tree is {source}, whose binary is {:?}",
            at.display(),
            probe[0]
        );
    }
}

/// `provenance.schema.json` — the ONE statement of what each origin requires.
/// This gate, `fixture-age.py --check-metadata` and the README table each used to
/// carry their own copy; the Python one was missing `command` while owning the
/// single-owner trees no Rust gate reaches.
struct ProvenanceSchema(serde_json::Value);

impl ProvenanceSchema {
    fn path() -> PathBuf {
        fixtures_root().join("provenance.schema.json")
    }

    fn load() -> Self {
        let p = Self::path();
        let body =
            std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
        Self(serde_json::from_str(&body).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
    }

    fn origins(&self) -> &serde_json::Map<String, serde_json::Value> {
        self.0["origins"].as_object().expect("origins is an object")
    }

    fn required(&self, origin: &str) -> Option<Vec<String>> {
        Some(
            self.origins().get(origin)?["required"]
                .as_array()
                .expect("required is an array")
                .iter()
                .map(|v| v.as_str().expect("required member is a string").to_string())
                .collect(),
        )
    }
}

/// The README's table is prose a human reads instead of the JSON, so it is the
/// copy most likely to rot — it stated the schema for its own audience while the
/// gates drifted underneath it.
/// Moving the required list into a DATA file made every provenance gate editable
/// as prose: emptying `required` and updating the README to match disarms both
/// gates and reads as documentation maintenance. Widening is free; narrowing has
/// to come through here.
#[test]
fn the_schema_cannot_be_narrowed_by_a_data_edit() {
    let schema = ProvenanceSchema::load();
    for (origin, must) in [
        ("recorded", &["cli", "version", "captured", "command"][..]),
        ("composed", &["note"][..]),
        ("unknown", &["note"][..]),
    ] {
        let got: BTreeSet<String> = schema
            .required(origin)
            .unwrap_or_else(|| panic!("{origin} missing from the schema"))
            .into_iter()
            .collect();
        let want: BTreeSet<String> = must.iter().map(|s| (*s).to_string()).collect();
        assert!(
            got.is_superset(&want),
            "provenance.schema.json narrowed {origin} to {got:?} — it must still \
             require at least {want:?}"
        );
    }
}

#[test]
fn the_readme_states_the_schema_the_gates_enforce() {
    let schema = ProvenanceSchema::load();
    let readme = std::fs::read_to_string(fixtures_root().join("README.md")).expect("README");
    for (origin, spec) in schema.origins() {
        let fields = schema.required(origin).expect("origin in its own table");
        let row = format!(
            "| `{origin}` | {} | {} |",
            fields
                .iter()
                .map(|f| format!("`{f}`"))
                .collect::<Vec<_>>()
                .join(", "),
            spec["means"].as_str().expect("means is a string")
        );
        assert!(
            readme.contains(&row),
            "fixtures/README.md must carry this row verbatim:\n  {row}"
        );
    }
}

/// Every scenario declares where its bytes came from, because nothing IN them
/// separates a capture from a composition — a redacted cwd and an invented one
/// look alike. A composed fixture pins its author's belief and the decoder then
/// agrees with it: kimi's shipped four confident per-call ids for a field kimi
/// never sends.
#[test]
fn every_scenario_declares_its_provenance() {
    let root = fixtures_root();
    let schema = ProvenanceSchema::load();
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
            let required = schema.required(origin).unwrap_or_else(|| {
                panic!(
                    "{}: origin {origin:?} is not recorded | composed | unknown",
                    path.display()
                )
            });
            for key in &required {
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
                    cli_matches_its_trees_binary(&source, &doc, &path);
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
    // BOTH directions: the one-way check let two members survive after their
    // `unknown` scenarios were recorded away, so the list claimed a reassurance
    // about sources that no longer had anything to reassure about.
    let unknown_sources: BTreeSet<&str> = UNVERIFIED_PROVENANCE
        .iter()
        .filter_map(|s| s.split('/').next())
        .collect();
    for src in UNKNOWN_BUT_BACKED_BY_A_CAPTURE {
        assert!(
            recorded.contains(*src),
            "{src} is listed as backed by a capture but has no recorded scenario"
        );
        assert!(
            unknown_sources.contains(src),
            "{src} has no `unknown` scenario left — drop its \
             UNKNOWN_BUT_BACKED_BY_A_CAPTURE entry"
        );
    }
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
