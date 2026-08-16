//! THE enumeration of committed captures, and the rules every capture obeys.
//!
//! There used to be four walks of this tree with three different populations
//! (34 / 42 / 24), so each rule landed on whichever subset its author happened
//! to pick. That is one root cause behind a finding class that recurred across
//! four review rounds — "the fix landed on half the population": `cli` reached 34
//! of 42, the edited-must-declare rule was flat until it was made recursive, the
//! payload-stamp axis saw 24. Every one was patched where it was found.
//!
//! So: ONE walk, and `the_walk_sees_every_provenance_on_disk` fails if a capture
//! falls outside it. A rule written against `every_capture()` cannot cover a
//! subset by accident, because there is no second walk to choose.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use pixtuoid_core::source::registry;

/// Module dirs whose name is NOT the registered source id. Every other
/// single-owner tree's dir name IS its source.
const MODULE_TO_SOURCE: &[(&str, &str)] = &[("claude", "claude-code")];

/// A committed capture: the bytes, the record of where they came from, and the
/// source the LAYOUT says owns them.
pub(crate) struct Capture {
    pub(crate) dir: PathBuf,
    /// Resolved from the layout, never from the provenance — a record that lies
    /// about its `cli` must not get to pick which rules apply to it.
    pub(crate) source: String,
    pub(crate) provenance_path: PathBuf,
    pub(crate) provenance: serde_json::Value,
}

impl Capture {
    pub(crate) fn origin(&self) -> &str {
        self.provenance
            .get("origin")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
    }

    pub(crate) fn is_recorded(&self) -> bool {
        self.origin() == "recorded"
    }

    pub(crate) fn field(&self, key: &str) -> Option<&str> {
        self.provenance.get(key).and_then(serde_json::Value::as_str)
    }

    /// The scenario's transcripts — every `.jsonl` that is not the hook payload
    /// file, RECURSIVELY: a parent-dir-keyed source nests its transcript one
    /// level down, and a flat read was blind to exactly the fixtures the
    /// nesting rule forces into that shape.
    pub(crate) fn transcripts(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        push_transcripts(&self.dir, &mut out);
        out.sort();
        out
    }

    pub(crate) fn hook_payloads(&self) -> Option<PathBuf> {
        let p = self.dir.join("hook-payloads.jsonl");
        p.is_file().then_some(p)
    }

    /// Every committed byte of this capture, for the rules that read content
    /// rather than metadata.
    pub(crate) fn wire_files(&self) -> Vec<PathBuf> {
        let mut out = self.transcripts();
        out.extend(self.hook_payloads());
        out
    }
}

fn push_transcripts(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
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

pub(crate) fn sources_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sources")
}

/// The conformance subtree, whose scenarios the decode harness iterates.
pub(crate) fn fixtures_root() -> PathBuf {
    sources_root().join("fixtures")
}

/// The registered source a capture dir belongs to, from the LAYOUT alone.
/// Three shapes exist: `fixtures/<source>/<scenario>/`, `<module>/fixtures/`,
/// and `<module>/fixtures/<sub>/`.
fn source_of(dir: &Path) -> Option<String> {
    let rel = dir.strip_prefix(sources_root()).ok()?;
    let parts: Vec<&str> = rel.iter().filter_map(|s| s.to_str()).collect();
    let raw = match parts.as_slice() {
        ["fixtures", source, _scenario] => *source,
        [_module, "fixtures", sub] => *sub,
        [module, "fixtures"] => *module,
        _ => return None,
    };
    let mapped = MODULE_TO_SOURCE
        .iter()
        .find(|(from, _)| *from == raw)
        .map_or(raw, |(_, to)| *to);
    Some(mapped.to_string())
}

/// THE walk. Every `provenance.json` in the tree, with the source its layout
/// names. Sorted, so failures name captures in a stable order.
pub(crate) fn every_capture() -> Vec<Capture> {
    let root = sources_root();
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if e.path().is_dir() {
                stack.push(e.path());
            }
        }
        let prov = dir.join("provenance.json");
        if !prov.is_file() {
            continue;
        }
        let body = std::fs::read_to_string(&prov)
            .unwrap_or_else(|e| panic!("read {}: {e}", prov.display()));
        let provenance: serde_json::Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("{}: not valid JSON: {e}", prov.display()));
        let source = source_of(&dir).unwrap_or_else(|| {
            panic!(
                "{}: no layout rule names this capture's source — it is in NO \
                 population, so every provenance rule skips it",
                dir.display()
            )
        });
        out.push(Capture {
            dir,
            source,
            provenance_path: prov,
            provenance,
        });
    }
    out.sort_by(|a, b| a.dir.cmp(&b.dir));
    out
}

/// The completeness pin that makes the single walk load-bearing: a capture the
/// walk cannot see is one every rule silently skips, which is the failure the
/// walk exists to make impossible.
#[test]
fn the_walk_sees_every_provenance_on_disk() {
    fn count(dir: &Path, n: &mut usize) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                count(&p, n);
            } else if p.file_name().and_then(|s| s.to_str()) == Some("provenance.json") {
                *n += 1;
            }
        }
    }
    let mut on_disk = 0;
    count(&sources_root(), &mut on_disk);
    let walked = every_capture().len();
    assert_eq!(
        walked, on_disk,
        "the walk sees {walked} captures but {on_disk} provenance.json files exist — \
         the difference is captures no rule applies to"
    );
    assert!(
        on_disk >= 40,
        "only {on_disk} captures found; the walk is looking at the wrong tree, so \
         every rule below would pass vacuously"
    );
}

/// Every capture's layout resolves to a REGISTERED source. A dir name that
/// matches no source is a typo that would otherwise route the capture's rules
/// to a descriptor lookup that silently returns `None`.
#[test]
fn every_captures_layout_names_a_registered_source() {
    for c in every_capture() {
        assert!(
            registry::descriptor_for(&c.source).is_some(),
            "{}: layout names source {:?}, which is not registered",
            c.dir.display(),
            c.source
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The rules. Each is an assertion over `every_capture()`, so none can cover a
// subset by accident — the shape that produced "the fix landed on half the
// population" in four separate review rounds.
// ─────────────────────────────────────────────────────────────────────────────

/// `unknown` scenarios that stay unknown, with the reason. It only SHRINKS.
const UNVERIFIED_PROVENANCE: &[&str] = &[
    // Not re-recordable: its cwd is a real Windows path with a space and parens,
    // which is what makes it the Windows arm of the cwd-extractor test.
    "copilot/tool-run",
];

/// Sources whose `unknown` scenarios now sit BESIDE a recorded one.
const UNKNOWN_BUT_BACKED_BY_A_CAPTURE: &[&str] = &["copilot"];

/// Hook-only sources with no recorded scenario. The list only SHRINKS — a hook
/// event is transient, so for these the fixture is the only wire evidence there
/// will ever be, and a new hook-only CLI must not join it by default.
const NO_WIRE_EVIDENCE_YET: &[&str] = &[];

/// `provenance.schema.json` — the ONE statement of what each origin requires,
/// read by this gate, `fixture-age.py --check-metadata`, and the README table.
struct ProvenanceSchema(serde_json::Value);

impl ProvenanceSchema {
    fn load() -> Self {
        let p = fixtures_root().join("provenance.schema.json");
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
                .map(|v| {
                    v.as_str()
                        .expect("a required member is a string")
                        .to_string()
                })
                .collect(),
        )
    }
}

/// Every scenario declares where its bytes came from, because nothing IN them
/// separates a capture from a composition — a redacted cwd and an invented one
/// look alike. A composed fixture pins its author's belief and the decoder then
/// agrees with it: kimi's shipped four confident per-call ids for a field kimi
/// never sends.
#[test]
fn every_capture_declares_a_valid_origin_with_its_required_fields() {
    let schema = ProvenanceSchema::load();
    for c in every_capture() {
        let required = schema.required(c.origin()).unwrap_or_else(|| {
            panic!(
                "{}: origin {:?} is not recorded | composed | unknown",
                c.provenance_path.display(),
                c.origin()
            )
        });
        for key in &required {
            assert!(
                c.field(key).is_some_and(|s| !s.trim().is_empty()),
                "{}: origin {:?} requires a non-empty {key:?}",
                c.provenance_path.display(),
                c.origin()
            );
        }
    }
}

/// `cli` was the one required field nothing could falsify, so a provenance
/// naming a DIFFERENT CLI than the tree it sits in passed every gate. The
/// registry's probe argv[0] is the name the user types (`agy`, `cursor-agent`),
/// which is what a capture command starts with.
#[test]
fn a_recorded_captures_cli_is_its_trees_binary() {
    for c in every_capture().iter().filter(|c| c.is_recorded()) {
        let Some(probe) = registry::descriptor_for(&c.source).and_then(|d| d.version_probe) else {
            continue;
        };
        let Some(cli) = c.field("cli") else { continue };
        assert_eq!(
            cli,
            probe[0],
            "{}: `cli` is {cli:?} but this tree is {}, whose binary is {:?}",
            c.provenance_path.display(),
            c.source,
            probe[0]
        );
    }
}

/// A `recorded` fixture whose bytes were EDITED must say so. Nothing in the
/// bytes separates a capture from a composition — which is the whole reason
/// provenance exists — so a redaction sentinel with a silent `note` is the one
/// state the mechanism cannot tolerate. Sentinels, not a `/Users/dev` grep: the
/// sweep that keyed on that alone missed kimi's, whose redaction is the owner
/// column inside a captured `ls -la`.
#[test]
fn a_recorded_capture_that_was_edited_says_so() {
    const SENTINELS: &[&str] = &[
        "/Users/dev",
        " dev  wheel",
        " dev  staff",
        "[redacted",
        "dev@",
    ];
    // A word ending the clause right before "redact" that inverts it.
    const NEGATIONS: &[&str] = &["no", "not", "nothing", "never", "without"];
    let mut silent = Vec::new();
    for c in every_capture().iter().filter(|c| c.is_recorded()) {
        let note = c.field("note").unwrap_or_default().to_ascii_lowercase();
        // "verbatim" USED to satisfy this, which let a note assert the OPPOSITE
        // of what the bytes show. A bare `contains` re-opens that hole one word
        // over — "unredacted", "nothing redacted".
        let declares = note.match_indices("redact").any(|(at, _)| {
            let before = note[..at].trim_end();
            !before.ends_with("un") && !NEGATIONS.iter().any(|n| before.ends_with(n))
        });
        let edited = c.wire_files().iter().any(|p| {
            std::fs::read_to_string(p)
                .map(|b| SENTINELS.iter().any(|s| b.contains(s)))
                .unwrap_or(false)
        });
        if edited && !declares {
            silent.push(c.dir.strip_prefix(sources_root()).unwrap().to_path_buf());
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
/// first. One token, not a set — a token SET let a registry pin the build date
/// (`2026.8.13`), a git sha (`e8db854`), or the word `Agent`.
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
        runs.push((
            start > 0 && matches!(bytes[start - 1], b'v' | b'V'),
            major,
            run,
        ));
    }
    runs.iter()
        .find(|(vp, ..)| *vp)
        .or_else(|| runs.iter().find(|(_, maj, _)| *maj < IMPLAUSIBLE_MAJOR))
        .or_else(|| runs.first())
        .map(|(.., run)| *run)
}

/// The cases `doctor::parse_version`'s own doc names, so the mirror cannot drift
/// from the parser it copies.
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
/// scenario IS that sighting, so the two must not disagree — `doctor`'s "newer
/// than verified" warning is structurally silent at `unknown` and would
/// otherwise warn a user running the exact version we hold a capture from.
#[test]
fn a_recorded_capture_anchors_its_sources_verified_version() {
    let mut by_source: std::collections::BTreeMap<String, Vec<(String, String)>> =
        std::collections::BTreeMap::new();
    let mut recorded_at_all: BTreeSet<String> = BTreeSet::new();
    for c in every_capture().iter().filter(|c| c.is_recorded()) {
        recorded_at_all.insert(c.source.clone());
        let version = c.field("version").unwrap_or_default();
        if version.chars().any(|ch| ch.is_ascii_digit()) {
            by_source.entry(c.source.clone()).or_default().push((
                c.field("captured").unwrap_or_default().to_string(),
                version.to_string(),
            ));
        }
    }
    for source in recorded_at_all {
        let Some(d) = registry::descriptor_for(&source) else {
            continue;
        };
        let Some(versions) = by_source.get(&source) else {
            // A source with captures but NO version among them cannot anchor
            // anything, and skipping it is how `verified_version: "0.0.0-A-LIE"`
            // passed for copilot: three recorded scenarios, all `unknown`.
            assert_eq!(
                d.verified_version, "unknown",
                "{source}: every recorded capture here says `version: unknown`, so nothing \
                 in this tree anchors {:?}. Re-record with a version, or set the field to \
                 \"unknown\" — a number no capture backs is the state the field was defined \
                 to avoid.",
                d.verified_version
            );
            continue;
        };
        // The NEWEST capture, not `any`: hermes holds 0.20.0 and 0.20.1, and
        // `any` let the older one anchor forever — precisely the stale pin this
        // test exists to prevent. ISO dates sort lexically, but an UNDATED
        // capture cannot say which sighting is most recent (and `"unknown"`
        // sorts above every date), so it is excluded from the ordering and only
        // used when a source has nothing dated.
        let mut dated: Vec<&(String, String)> = versions
            .iter()
            .filter(|(when, _)| when.len() == 10 && when.starts_with("20"))
            .collect();
        dated.sort();
        let candidates: Vec<&String> = if dated.is_empty() {
            versions.iter().map(|(_, v)| v).collect()
        } else {
            vec![&dated.last().expect("non-empty").1]
        };
        let newest = candidates[0];
        assert!(
            candidates
                .iter()
                .any(|v| banner_version(v) == Some(d.verified_version)),
            "{source}: `verified_version` is {:?}, but the newest recorded capture \
             ({newest:?}) pins {:?} — the most recent sighting is the one that counts",
            d.verified_version,
            banner_version(newest).unwrap_or("nothing")
        );
    }
}

/// The three pinned rosters, over the SAME walk the rules use — they used to be
/// computed inside one rule's own narrower walk.
#[test]
fn the_pinned_provenance_rosters_hold_both_ways() {
    let captures = every_capture();
    let recorded: BTreeSet<&str> = captures
        .iter()
        .filter(|c| c.is_recorded())
        .map(|c| c.source.as_str())
        .collect();
    let unknown: BTreeSet<String> = captures
        .iter()
        .filter(|c| c.origin() == "unknown")
        .filter_map(|c| {
            let rel = c.dir.strip_prefix(fixtures_root()).ok()?;
            Some(rel.to_string_lossy().replace('\\', "/"))
        })
        .collect();
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
            recorded.contains(src),
            "{src} is listed as backed by a capture but has no recorded scenario"
        );
        assert!(
            unknown_sources.contains(src),
            "{src} has no `unknown` scenario left — drop its \
             UNKNOWN_BUT_BACKED_BY_A_CAPTURE entry"
        );
    }
    for src in registry::registered_source_names()
        .filter(|s| registry::descriptor_for(s).is_some_and(|d| d.line_decoder().is_none()))
    {
        assert_eq!(
            NO_WIRE_EVIDENCE_YET.contains(&src),
            !recorded.contains(src),
            "{src} is hook-only with recorded={}; either record a scenario and drop its \
             NO_WIRE_EVIDENCE_YET entry, or add one",
            recorded.contains(src)
        );
    }
}

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
            "provenance.schema.json narrowed {origin} to {got:?} — it must still require \
             at least {want:?}"
        );
    }
}

/// The README's table is prose a human reads instead of the JSON, so it is the
/// copy most likely to rot.
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

/// The two walks must see the SAME population. They exist on both sides of a
/// language boundary and neither can call the other, so this is the pin the
/// repo's magic-number rule asks for when a value genuinely crosses one.
#[test]
fn the_two_capture_walks_agree() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/lib/captures.py");
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(format!(
            "import sys; sys.dont_write_bytecode=True; sys.path.insert(0, {:?});\n\
             import captures as C\n\
             for c in C.every_capture(): print(f'{{c.dir.relative_to(C.SOURCES)}}\\t{{c.source}}')",
            script.parent().expect("has a parent").to_string_lossy()
        ))
        .output()
        .expect("python3 — the Python gates run in lint and CI, so it is a build dep here");
    assert!(
        out.status.success(),
        "the Python walk failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let theirs: BTreeSet<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.replace('\\', "/"))
        .collect();
    let ours: BTreeSet<String> = every_capture()
        .iter()
        .map(|c| {
            format!(
                "{}\t{}",
                c.dir
                    .strip_prefix(sources_root())
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                c.source
            )
        })
        .collect();
    assert_eq!(
        ours,
        theirs,
        "the Rust and Python capture walks disagree.\n  only Rust: {:?}\n  only Python: {:?}",
        ours.difference(&theirs).collect::<Vec<_>>(),
        theirs.difference(&ours).collect::<Vec<_>>()
    );
}
