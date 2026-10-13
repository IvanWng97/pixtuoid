//! What a decoder READS of a capture, derived by probe and never listed: a
//! subtree is unread when neutralizing it leaves everything the registry derives
//! from the line unchanged. The recorder strips captures with it, so a field a
//! decoder stopped reading cannot linger in committed bytes.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{Capture, Drive, LineFailure, captures};
use crate::source::daemon::DecodedPresence;
use crate::source::registry;

/// What the provenance says happened to the bytes after recording. `Exempt`
/// is declared by hand, in the committed record, for a capture whose purpose
/// is to pin a wire premise the decoder drops on purpose — a test that reads
/// raw bytes is invisible to the probe, so the record has to say so.
#[derive(Debug)]
pub enum Deidentified {
    /// Subtrees blanked because no decoder reads them.
    Stripped(usize),
    /// Left as recorded, for the reason the provenance gives.
    Exempt(String),
}

impl Deidentified {
    /// The provenance's `deidentified` object.
    pub fn record(&self) -> Value {
        match self {
            Self::Stripped(n) => serde_json::json!({ "method": "decoder-allowlist", "blanked": n }),
            Self::Exempt(why) => serde_json::json!({ "method": "none", "why": why }),
        }
    }
}

/// The exemption a committed `provenance.json` declares, if any.
pub fn exemption(provenance: &Path) -> Option<String> {
    let record: Value = serde_json::from_str(&std::fs::read_to_string(provenance).ok()?).ok()?;
    (record["deidentified"]["method"] == "none").then(|| {
        record["deidentified"]["why"]
            .as_str()
            .unwrap_or("")
            .to_string()
    })
}

/// One strip pass: how many subtrees were blanked, and which files were left as
/// recorded because a line in them is not JSON.
#[derive(Debug, Default)]
pub struct Strip {
    /// Subtrees blanked across every file.
    pub blanked: usize,
    /// Files left whole: half-rewriting one would corrupt the bytes. The
    /// committed-tree gate refuses to skip such a line at all
    /// (`no_identity_key_holds_a_value_outside_a_pinned_exemption`).
    pub not_json: Vec<PathBuf>,
}

/// Blank every subtree no decoder reads, so an operator's roster, instructions
/// and paths leave without anyone naming them. DERIVED, never listed — a subtree
/// is blanked when blanking it leaves the decoded events identical — because a
/// hand-kept list drifts in the one direction nothing catches: a field the
/// decoder stopped reading stays in the bytes. Top-down, so an unread container
/// collapses whole, element count included.
///
/// # Errors
///
/// If a file cannot be read or rewritten.
pub fn strip_unread(source: &str, files: &[PathBuf]) -> std::io::Result<Strip> {
    let mut strip = Strip::default();
    for file in files {
        let Some(drive) = drive_for(source, file) else {
            continue;
        };
        let Some((raw, mut lines)) = parsed_lines(file)? else {
            strip.not_json.push(file.clone());
            continue;
        };
        let base = events_of(source, &drive, &lines);
        let stamped = is_hook_envelope(file);
        let mut out = String::new();
        for i in 0..lines.len() {
            let here = probe(source, &drive, &mut lines, &base, i, "", stamped);
            strip.blanked += here;
            if here == 0 {
                out.push_str(&raw[i]);
            } else {
                out.push_str(&serde_json::to_string(&lines[i])?);
            }
            out.push('\n');
        }
        // A sibling then a rename, so a reader never sees a half-written capture;
        // std rather than `tempfile`, which is only this crate's dev-dependency.
        let mut tmp_name = file.as_os_str().to_owned();
        tmp_name.push(".strip-tmp");
        let tmp = PathBuf::from(tmp_name);
        std::fs::File::create(&tmp)?.write_all(out.as_bytes())?;
        std::fs::rename(&tmp, file)?;
    }
    Ok(strip)
}

/// Strip every RECORDED capture under `root` in place — local and unbilled — and
/// record the count in its provenance. A composed capture carries no operator
/// data by construction, and the README demo reads its bytes past the decoders.
/// A pass that blanks nothing leaves a record that already carries a count
/// alone, so re-running never erases history: `blanked` counts the original
/// recording, not what a fresh capture would differ by today.
///
/// # Errors
///
/// If a provenance or capture file cannot be read, parsed or rewritten.
pub fn strip_corpus(root: &Path) -> std::io::Result<Vec<(Capture, Strip)>> {
    let mut done = Vec::new();
    for capture in captures(root) {
        let prov = capture.dir.join("provenance.json");
        let mut record: Value = serde_json::from_str(&std::fs::read_to_string(&prov)?)?;
        if record["origin"] != "recorded" || record["deidentified"]["method"] == "none" {
            continue;
        }
        let files = capture.jsonl_files();
        if files.is_empty() {
            continue;
        }
        let strip = strip_unread(&capture.source, &files)?;
        if let Some(obj) = record.as_object_mut()
            && (strip.blanked > 0 || !obj.contains_key("deidentified"))
        {
            obj.insert(
                "deidentified".into(),
                Deidentified::Stripped(strip.blanked).record(),
            );
            std::fs::write(
                &prov,
                format!("{}\n", serde_json::to_string_pretty(&record)?),
            )?;
        }
        done.push((capture, strip));
    }
    Ok(done)
}

/// A hook envelope file, by the name the recorder gives it.
pub fn is_hook_envelope(file: &Path) -> bool {
    file.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("hook-payloads"))
}

fn drive_for(source: &str, file: &Path) -> Option<Drive> {
    if is_hook_envelope(file) {
        Some(Drive::hooks())
    } else {
        Drive::transcript(source, &file.to_string_lossy())
    }
}

/// `None` when a line is not JSON: the file is left as recorded rather than
/// half-rewritten.
type Parsed = (Vec<String>, Vec<Value>);

fn parsed_lines(file: &Path) -> std::io::Result<Option<Parsed>> {
    let text = std::fs::read_to_string(file)?;
    let mut raw = Vec::new();
    let mut lines = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(v) = serde_json::from_str(line) else {
            return Ok(None);
        };
        raw.push(line.to_string());
        lines.push(v);
    }
    Ok(Some((raw, lines)))
}

type Decoded = (
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<Option<DecodedPresence>>,
    Vec<String>,
);

/// Everything the registry reads from a line, not only the line decoder: a
/// daemon's envelopes decode to no `AgentEvent` by design (presence rides
/// `presence_decoder`), and the watcher registers a transcript through
/// `cwd_extractor`, which the harness never calls — it seeds a fixed cwd.
/// Compared as rendered, the way a golden is: `PathBuf` equality drops a
/// trailing slash, and grok's `workspaceRoot` fallback differs by exactly that.
fn events_of(source: &str, drive: &Drive, lines: &[Value]) -> Decoded {
    let d = drive.lines(lines.iter().map(Value::to_string));
    let cwds = lines
        .iter()
        .map(|l| format!("{:?}", registry::cwd_extractor_for(source)(l)))
        .collect();
    let presence = lines
        .iter()
        .map(|line| {
            let src = line.get("_pixtuoid_source")?.as_str()?;
            registry::presence_decoder_for(src)?(line).ok()
        })
        .collect();
    let failures = |f: &[LineFailure]| f.iter().map(|x| format!("{x:?}")).collect::<Vec<_>>();
    (
        d.events.iter().map(|e| format!("{e:?}")).collect(),
        failures(&d.decode_errors),
        failures(&d.panics),
        presence,
        cwds,
    )
}

/// A hook envelope's top-level `_` keys are the shim's stamps, kept by namespace:
/// `_shim_ts_ms` is read by no decoder, only by `captures.rs`, which dates a capture by it.
fn probe(
    source: &str,
    drive: &Drive,
    lines: &mut [Value],
    base: &Decoded,
    i: usize,
    at: &str,
    stamped: bool,
) -> usize {
    let Some(node) = lines[i].pointer(at) else {
        return 0;
    };
    let neutral = match node {
        Value::Null => return 0,
        Value::Bool(_) => Value::Bool(false),
        Value::Number(_) => Value::from(0),
        Value::String(_) => Value::String(String::new()),
        Value::Array(_) => Value::Array(Vec::new()),
        Value::Object(_) => Value::Object(serde_json::Map::new()),
    };
    if *node == neutral {
        return 0;
    }
    let Some(slot) = lines[i].pointer_mut(at) else {
        return 0;
    };
    let kept = std::mem::replace(slot, neutral);
    if events_of(source, drive, lines) == *base {
        return 1;
    }
    let children: Vec<String> = match &kept {
        Value::Array(a) => (0..a.len()).map(|k| format!("{at}/{k}")).collect(),
        Value::Object(m) => m
            .keys()
            .filter(|k| !(stamped && at.is_empty() && k.starts_with('_')))
            .map(|k| format!("{at}/{}", k.replace('~', "~0").replace('/', "~1")))
            .collect(),
        _ => Vec::new(),
    };
    if let Some(slot) = lines[i].pointer_mut(at) {
        *slot = kept;
    }
    children
        .iter()
        .map(|child| probe(source, drive, lines, base, i, child, stamped))
        .sum()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn sources_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sources")
    }

    fn scenario_events(source: &str, files: &[PathBuf]) -> Vec<Decoded> {
        files
            .iter()
            .filter_map(|f| {
                let (_, lines) = parsed_lines(f).ok()??;
                Some(events_of(source, &drive_for(source, f)?, &lines))
            })
            .collect()
    }

    fn first_key(line: &str) -> &str {
        line.trim_start_matches('{')
            .split('"')
            .nth(1)
            .expect("an object with a key")
    }

    /// Plant a key no decoder reads on the first non-empty line, so the test
    /// holds whether or not the committed corpus has already been stripped.
    fn plant_unread(file: &Path, key: &str, value: &str) {
        let text = std::fs::read_to_string(file).expect("read");
        let mut planted = false;
        let out: Vec<String> = text
            .lines()
            .map(|l| {
                let mut v: serde_json::Value = serde_json::from_str(l).expect("json");
                if !planted && v.as_object().is_some_and(|o| !o.is_empty()) {
                    v[key] = serde_json::Value::String(value.into());
                    planted = true;
                }
                serde_json::to_string(&v).expect("json")
            })
            .collect();
        assert!(planted, "no non-empty line to plant on");
        std::fs::write(file, out.join("\n") + "\n").expect("write");
    }

    /// The scenario's one transcript, found rather than named: a re-record
    /// gives it a new filename. The registry's admit rule and the recorder's
    /// own hook-envelope test decide what is a transcript, and a stale second
    /// one beside a promoted `.new` fails here instead of being picked by
    /// directory order.
    fn transcript_in(source: &str, scenario: &Path) -> PathBuf {
        let mut found: Vec<PathBuf> = crate::harness::transcripts_under(source, scenario)
            .into_iter()
            .filter(|p| !is_hook_envelope(p))
            .collect();
        assert_eq!(
            found.len(),
            1,
            "{}: one transcript per scenario, found {:?}",
            scenario.display(),
            found
        );
        found.remove(0)
    }

    fn stripped_copy_of(src: &Path) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().expect("tempdir");
        let to = d.path().join(src.file_name().expect("file name"));
        std::fs::copy(src, &to).expect("copy");
        (d, to)
    }

    fn stripped_copy(source: &str, scenario: &str, name: &str) -> (tempfile::TempDir, PathBuf) {
        stripped_copy_of(
            &sources_root()
                .join("fixtures")
                .join(source)
                .join(scenario)
                .join(name),
        )
    }

    #[test]
    fn a_daemon_capture_keeps_what_its_presence_decoder_reads() {
        let (_d, hooks) = stripped_copy(
            "openclaw",
            "gateway-lifecycle-recorded",
            "hook-payloads.jsonl",
        );
        let files = vec![hooks.clone()];
        let before = scenario_events("openclaw", &files);
        let wire = std::fs::read_to_string(&hooks).expect("read");
        strip_unread("openclaw", &files).expect("strip");
        assert_eq!(scenario_events("openclaw", &files), before);
        let stripped = std::fs::read_to_string(&hooks).expect("read");
        assert_eq!(
            stripped.lines().next(),
            wire.lines().next(),
            "every field of `gateway_start` is read or a shim stamp, so the line is untouched"
        );
    }

    #[test]
    fn a_cwd_whose_fallback_differs_by_a_trailing_slash_survives_the_strip() {
        let (_d, hooks) = stripped_copy("grok", "gate-both-ways", "hook-payloads.jsonl");
        let rendered = |p: &Path| -> Vec<String> {
            Drive::hooks()
                .lines(std::fs::read_to_string(p).expect("read").lines())
                .events
                .iter()
                .map(|e| format!("{e:?}"))
                .collect()
        };
        let before = rendered(&hooks);
        assert!(before.iter().any(|e| e.contains("pixtuoid-capture/proj\"")));
        strip_unread("grok", std::slice::from_ref(&hooks)).expect("strip");
        assert_eq!(rendered(&hooks), before);
    }

    #[test]
    fn the_first_sight_cwd_survives_the_strip() {
        let (_d, rollout) = stripped_copy_of(&transcript_in(
            "codex",
            &sources_root().join("fixtures/codex/tool-run-recorded"),
        ));
        let cwds = |p: &Path| -> Vec<Option<PathBuf>> {
            let extract = registry::cwd_extractor_for("codex");
            parsed_lines(p)
                .expect("read")
                .expect("json")
                .1
                .iter()
                .map(extract)
                .collect()
        };
        let before = cwds(&rollout);
        assert!(
            before
                .iter()
                .any(|c| c.as_ref().is_some_and(|p| !p.as_os_str().is_empty())),
            "the rollout carries the cwd first-sight registers"
        );
        strip_unread("codex", std::slice::from_ref(&rollout)).expect("strip");
        assert_eq!(cwds(&rollout), before);
    }

    #[test]
    fn the_shim_stamps_survive_the_strip() {
        let (_d, hooks) = stripped_copy("claude-code", "tool-run-recorded", "hook-payloads.jsonl");
        let stamps = |p: &Path| -> Vec<serde_json::Value> {
            parsed_lines(p)
                .expect("read")
                .expect("json")
                .1
                .iter()
                .map(|l| l["_shim_ts_ms"].clone())
                .collect()
        };
        let before = stamps(&hooks);
        assert!(before.iter().all(|v| v.as_i64().is_some_and(|ms| ms > 0)));
        strip_unread("claude-code", std::slice::from_ref(&hooks)).expect("strip");
        assert_eq!(stamps(&hooks), before);
    }

    #[test]
    fn stripping_blanks_what_no_decoder_reads_and_changes_no_event() {
        let d = tempfile::tempdir().expect("tempdir");
        let src = sources_root().join("fixtures/codex/tool-run-recorded");
        let files: Vec<PathBuf> = [
            transcript_in("codex", &src),
            src.join("hook-payloads.jsonl"),
        ]
        .iter()
        .map(|from| {
            let to = d.path().join(from.file_name().expect("file name"));
            std::fs::copy(from, &to).expect("copy");
            to
        })
        .collect();

        plant_unread(&files[0], "installed_skills", "example-skill-1");
        let before = scenario_events("codex", &files);
        let wire = std::fs::read_to_string(&files[0]).expect("read");
        let session_id = first_hook_field(&files[1], "session_id");
        let blanked = strip_unread("codex", &files).expect("strip").blanked;
        assert!(blanked > 0, "the planted key is unread");
        assert_eq!(scenario_events("codex", &files), before);

        let transcript = std::fs::read_to_string(&files[0]).expect("read");
        let rewritten = wire
            .lines()
            .zip(transcript.lines())
            .find(|(w, t)| w != t && *t != "{}")
            .expect("a line rewritten but not collapsed");
        assert_eq!(
            first_key(rewritten.1),
            first_key(rewritten.0),
            "a rewritten line keeps the wire's key order"
        );
        assert!(
            !transcript.contains("example-skill"),
            "a roster rides a field no decoder reads, so it leaves"
        );
        assert!(
            transcript.contains("gpt-5.6-sol"),
            "the model is read, so it stays"
        );
        let hooks = std::fs::read_to_string(&files[1]).expect("read");
        assert!(
            hooks.contains(&session_id),
            "the hook's session_id keys coalescing, so it stays"
        );
    }

    /// A string field off the first hook payload, so an assertion follows the
    /// capture rather than pinning one recording's id. Refuses a blank: read
    /// after the strip, `contains("")` would pass on exactly the wrongly
    /// stripped shape.
    fn first_hook_field(hooks: &Path, key: &str) -> String {
        let line = std::fs::read_to_string(hooks).expect("read");
        let line = line.lines().next().expect("a hook line");
        let value = serde_json::from_str::<serde_json::Value>(line).expect("json")[key]
            .as_str()
            .unwrap_or_else(|| panic!("{key} on the first hook payload"))
            .to_string();
        assert!(
            !value.is_empty(),
            "{key} on the first hook payload is blank"
        );
        value
    }

    #[test]
    fn every_committed_scenario_decodes_the_same_once_stripped() {
        let root = sources_root();
        let mut walked = BTreeSet::new();
        for capture in captures(&root) {
            let (scenario, source) = (&capture.dir, &capture.source);
            let label = scenario
                .strip_prefix(&root)
                .expect("under root")
                .to_string_lossy()
                .replace('\\', "/");
            let jsonl = capture.jsonl_files();
            assert!(
                !jsonl.is_empty() || !scenario.join("provenance.json").is_file(),
                "{label}: a provenance with no bytes under it"
            );
            if jsonl.is_empty() {
                continue;
            }
            let d = tempfile::tempdir().expect("tempdir");
            let files: Vec<PathBuf> = jsonl
                .iter()
                .map(|from| {
                    let to = d.path().join(from.strip_prefix(scenario).expect("under"));
                    std::fs::create_dir_all(to.parent().expect("parent")).expect("mkdir");
                    std::fs::copy(from, &to).expect("copy");
                    to
                })
                .collect();
            assert!(
                registry::descriptor_for(source).is_some(),
                "{label}: {source:?} is not a registered source, so every rule keyed on it misses"
            );
            for f in &files {
                assert!(
                    drive_for(source, f).is_some(),
                    "{label}: {} cannot be routed as {source}, so the walk would skip it",
                    f.display()
                );
            }
            let before = scenario_events(source, &files);
            strip_unread(source, &files).expect("strip");
            assert_eq!(scenario_events(source, &files), before, "{label}");
            walked.insert(label);
        }
        for sentinel in [
            "fixtures/codex/tool-run-recorded",
            "fixtures/claude-code/tool-run-recorded",
            "omp/fixtures/bridge-run-recorded",
            "cursor/fixtures",
            "delegation/fixtures/copilot",
            "fixtures/copilot/tool-run-recorded",
            "fixtures/grok/tool-run-recorded",
        ] {
            assert!(
                walked.contains(sentinel),
                "walk missed {sentinel}: {walked:?}"
            );
        }
    }

    #[test]
    fn strip_corpus_leaves_a_composed_capture_alone() {
        let d = tempfile::tempdir().expect("tempdir");
        let scenario = d.path().join("fixtures/claude-code/proof-session");
        std::fs::create_dir_all(&scenario).expect("mkdir");
        let src = sources_root().join("fixtures/claude-code/proof-session");
        for from in std::fs::read_dir(&src).expect("read").flatten() {
            std::fs::copy(from.path(), scenario.join(from.file_name())).expect("copy");
        }
        let bytes = |dir: &Path| -> Vec<(PathBuf, Vec<u8>)> {
            let mut out: Vec<_> = std::fs::read_dir(dir)
                .expect("read")
                .flatten()
                .map(|e| (e.path(), std::fs::read(e.path()).expect("bytes")))
                .collect();
            out.sort();
            out
        };
        let before = bytes(&scenario);
        strip_corpus(d.path()).expect("strip corpus");
        assert_eq!(
            bytes(&scenario),
            before,
            "composed bytes carry no operator data"
        );
    }

    #[test]
    fn strip_corpus_honors_an_exemption_the_provenance_declares() {
        let d = tempfile::tempdir().expect("tempdir");
        let scenario = d.path().join("fixtures/codex/tool-run-recorded");
        std::fs::create_dir_all(&scenario).expect("mkdir");
        let src = sources_root().join("fixtures/codex/tool-run-recorded");
        for from in std::fs::read_dir(&src).expect("read").flatten() {
            std::fs::copy(from.path(), scenario.join(from.file_name())).expect("copy");
        }
        let prov = scenario.join("provenance.json");
        let mut record: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&prov).expect("read")).expect("json");
        record["deidentified"] = serde_json::json!({
            "method": "none",
            "why": "pins a wire premise the decoder drops on purpose"
        });
        std::fs::write(&prov, serde_json::to_string_pretty(&record).expect("json")).expect("write");
        let bytes = |dir: &Path| -> Vec<(PathBuf, Vec<u8>)> {
            let mut out: Vec<_> = std::fs::read_dir(dir)
                .expect("read")
                .flatten()
                .map(|e| (e.path(), std::fs::read(e.path()).expect("bytes")))
                .collect();
            out.sort();
            out
        };
        let before = bytes(&scenario);
        strip_corpus(d.path()).expect("strip corpus");
        assert_eq!(
            bytes(&scenario),
            before,
            "an exempt capture keeps its bytes and its record"
        );
    }

    #[test]
    fn strip_corpus_records_its_count_in_the_provenance_and_is_idempotent() {
        let d = tempfile::tempdir().expect("tempdir");
        let scenario = d.path().join("fixtures/codex/tool-run-recorded");
        std::fs::create_dir_all(&scenario).expect("mkdir");
        let src = sources_root().join("fixtures/codex/tool-run-recorded");
        for from in std::fs::read_dir(&src).expect("read").flatten() {
            std::fs::copy(from.path(), scenario.join(from.file_name())).expect("copy");
        }
        let snapshot = |dir: &Path| -> Vec<(PathBuf, Vec<u8>)> {
            let mut out: Vec<_> = std::fs::read_dir(dir)
                .expect("read")
                .flatten()
                .map(|e| (e.path(), std::fs::read(e.path()).expect("bytes")))
                .collect();
            out.sort();
            out
        };
        plant_unread(
            &transcript_in("codex", &scenario),
            "installed_skills",
            "example-skill-1",
        );
        let wire = snapshot(&scenario);

        strip_corpus(d.path()).expect("strip corpus");
        let once = snapshot(&scenario);
        assert_ne!(once, wire, "the planted key is unread");
        let prov: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(scenario.join("provenance.json")).expect("read"),
        )
        .expect("json");
        assert_eq!(prov["deidentified"]["method"], "decoder-allowlist");
        assert!(
            prov["deidentified"]["blanked"]
                .as_u64()
                .is_some_and(|n| n > 0)
        );
        assert_eq!(prov["origin"], "recorded", "the rest of the record is kept");

        strip_corpus(d.path()).expect("strip corpus again");
        assert_eq!(
            snapshot(&scenario),
            once,
            "a second pass finds nothing and rewrites nothing"
        );
    }
}
