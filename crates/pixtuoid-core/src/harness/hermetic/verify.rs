//! Judging hermetic runs: each scenario's run alone ([`verify`]), and each
//! source's runs together ([`verify_source`]). A run's bytes ride the one
//! pipeline (`Drive`), and the scene a headless pixtuoid painted from the same
//! run is read as text. Ids, timestamps and model text change every run, so
//! nothing here compares bytes or goldens: only shapes, events and the fields
//! a decoder reads.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{MANIFEST, Manifest, Profile, SCENE_LOG, Scenario, Shape, profile};
use crate::AgentEvent;
use crate::harness::captures::jsonl_files_under;
use crate::harness::reads::{child_pointer, is_blank, is_hook_envelope, is_shim_stamp, strips};
use crate::harness::{Drive, Driven, HOOK_PAYLOADS, PROVENANCE, Reach, captures};

/// The tracing target every drift breadcrumb rides (`source::drift::TARGET`).
const BREADCRUMB_TARGET: &str = crate::source::drift::TARGET;

/// A verdict's outcome, ordered by severity: a verdict is its worst finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Label {
    /// Every check held.
    Pass,
    /// A `not_yet` hook fired or a `not_yet_fields` field was carried: good
    /// news, the list shrinks.
    Stale,
    /// The CLI could not run (install, network, a login wall): not judged.
    Blocked,
    /// The CLI asked the model something no script line answers: its request
    /// flow changed upstream.
    Script,
    /// pixtuoid read the run wrong, or upstream stopped sending what it reads.
    Fail,
}

/// One verdict: its findings, worst first.
#[derive(Debug)]
pub struct Verdict {
    findings: Vec<(Label, String)>,
}

impl Verdict {
    /// A verdict of `findings`, in any order.
    pub fn new(mut findings: Vec<(Label, String)>) -> Self {
        findings.sort_by_key(|f| std::cmp::Reverse(f.0));
        Self { findings }
    }

    /// The worst finding's label; Pass with none.
    pub fn label(&self) -> Label {
        self.findings.first().map_or(Label::Pass, |f| f.0)
    }

    /// Every finding, worst first.
    pub fn findings(&self) -> &[(Label, String)] {
        &self.findings
    }

    /// Whether the run was judged: a Blocked finding returns before any check,
    /// though a Script found first may outrank it.
    pub fn judged(&self) -> bool {
        !self.findings.iter().any(|f| f.0 == Label::Blocked)
    }
}

fn invalid(message: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}

/// Judge one scenario's run in `dir`.
///
/// # Errors
///
/// If the manifest or a capture file cannot be read, or the manifest names no
/// profiled scenario.
pub fn verify(dir: &Path) -> std::io::Result<Verdict> {
    let manifest: Manifest = serde_json::from_str(&std::fs::read_to_string(dir.join(MANIFEST))?)?;
    let p = profile(&manifest.source)
        .ok_or_else(|| invalid(format!("{}: not a profiled source", manifest.source)))?;
    let scenario = p
        .scenarios
        .iter()
        .find(|s| s.shape.name() == manifest.scenario)
        .ok_or_else(|| invalid(format!("{}: no scenario {}", p.source, manifest.scenario)))?;
    let scene = std::fs::read_to_string(dir.join(SCENE_LOG)).unwrap_or_default();
    let mut findings = Vec::new();
    judge(p, scenario, &manifest, dir, &scene, &mut findings)?;
    Ok(Verdict::new(findings))
}

/// Judge `source`'s runs together, for what no one scenario owns: a `not_yet`
/// hook any run fired is Stale, and the field census. A selection of one
/// scenario skips this, so it never fails on a field another scenario carries.
///
/// The census takes every field a decoder reads in a committed capture of
/// `source` (what the probe left unblanked, never a list), for each kind of
/// line a run produced, and requires some run to carry it: a field upstream
/// renamed or dropped fails here though the decoder still parses the line.
/// A kind no run produced is the event checks' to judge.
///
/// # Errors
///
/// If a run's or a committed capture's file cannot be read, or `source` has no
/// profile.
pub fn verify_source(source: &str, dirs: &[PathBuf]) -> std::io::Result<Verdict> {
    let p = profile(source).ok_or_else(|| invalid(format!("{source}: not a profiled source")))?;
    let mut fired = BTreeSet::new();
    let mut carried = Fields::default();
    for dir in dirs {
        for file in jsonl_files_under(dir) {
            for (i, line) in json_lines(&file)?.iter().enumerate() {
                if is_hook_envelope(&file)
                    && let Some(event) = (p.event_name)(line)
                {
                    fired.insert(event.to_owned());
                }
                carried.add(p, &file, i, line, true);
            }
        }
    }
    let mut findings = census(&committed_reads(p)?, &carried, p.not_yet_fields, source);
    for hook in p.not_yet {
        if fired.contains(*hook) {
            findings.push((
                Label::Stale,
                format!("{hook} fired: it leaves the profile's not_yet"),
            ));
        }
    }
    Ok(Verdict::new(findings))
}

/// The field census proper: each read field of a kind the runs produced must
/// be carried, unless `owed`, and an owed one carried is Stale. A committed
/// transcript line the strip left untyped has no kind to match: a head is
/// matched as the head ([`HEAD`]), and any other line needs some run's
/// transcript line to carry all of its read fields together.
fn census(
    reads: &Fields,
    carried: &Fields,
    owed: &[(&str, &str)],
    source: &str,
) -> Vec<(Label, String)> {
    let mut findings = Vec::new();
    for (kind, paths) in &reads.by_kind {
        let Some(seen) = carried.by_kind.get(kind) else {
            continue;
        };
        for path in paths {
            let is_owed = owed.contains(&(kind.as_str(), path.as_str()));
            match (seen.contains(path), is_owed) {
                (true, true) => findings.push((
                    Label::Stale,
                    format!("{kind} {path} was carried: it leaves the profile's not_yet_fields"),
                )),
                (false, false) => findings.push((
                    Label::Fail,
                    format!(
                        "{kind} {path}: a decoder reads it in a committed capture, \
                         and no run of {source} carried it"
                    ),
                )),
                _ => {}
            }
        }
    }
    if carried.lines.is_empty() {
        return findings;
    }
    for line in &reads.lines {
        if !carried.lines.iter().any(|l| line.is_subset(l)) {
            let paths: Vec<&str> = line.iter().map(String::as_str).collect();
            findings.push((
                Label::Fail,
                format!(
                    "a committed transcript line with no type reads {}, \
                     and no run of {source} carried them in one line",
                    paths.join(" ")
                ),
            ));
        }
    }
    findings
}

/// The non-blank leaves a set of captures holds, for the census.
#[derive(Debug, Default)]
pub(crate) struct Fields {
    /// Every leaf of each kind of line.
    by_kind: BTreeMap<String, BTreeSet<String>>,
    /// The leaves of each transcript line taken whole: every one of a run's,
    /// or a committed capture's untyped ones.
    lines: BTreeSet<BTreeSet<String>>,
}

impl Fields {
    /// Add line `index` of `file`. A committed (`run == false`) transcript
    /// line the strip left untyped has no kind a run's line could have: a head
    /// goes to [`HEAD`], any other line to `lines`. A run's head goes to both
    /// its kind and [`HEAD`].
    fn add(&mut self, p: &Profile, file: &Path, index: usize, line: &Value, run: bool) {
        let stamped = is_hook_envelope(file);
        let leaves: BTreeSet<String> = leaves(line, stamped).into_iter().collect();
        let kind = kind_of(p, file, line);
        let untyped = !stamped && kind == TX;
        if !stamped && index == 0 && (run || untyped) {
            self.by_kind
                .entry(HEAD.to_owned())
                .or_default()
                .extend(leaves.iter().cloned());
        }
        if run && !stamped || untyped && index > 0 {
            self.lines.insert(leaves.clone());
        }
        if run || !untyped {
            self.by_kind.entry(kind).or_default().extend(leaves);
        }
    }

    /// Whether a line of `kind` carried `path`, an untyped transcript line's
    /// included.
    #[cfg(test)]
    pub(crate) fn has(&self, kind: &str, path: &str) -> bool {
        self.by_kind.get(kind).is_some_and(|p| p.contains(path))
            || (kind == TX && self.lines.iter().any(|l| l.contains(path)))
    }
}

/// A transcript line's kind prefix, and the whole kind of one whose `type`
/// is blank.
const TX: &str = "tx:";
/// The kind of a transcript's first line, read by position where its `type`
/// is not (codex's head carries the cwd a watcher registers by:
/// `registry::cwd_extractor_for`).
const HEAD: &str = "tx:#head";

/// The fields a decoder reads in `p`'s committed captures: the recorder
/// blanks every subtree no decoder reads (`reads::strip_unread`), so a
/// stripped recording's non-blank leaves are exactly the read set, the
/// shim's stamps aside. A capture the strip skips proves nothing about reads.
pub(crate) fn committed_reads(p: &Profile) -> std::io::Result<Fields> {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sources");
    let mut reads = Fields::default();
    for capture in captures(&corpus) {
        if capture.source != p.source {
            continue;
        }
        let record: Value =
            serde_json::from_str(&std::fs::read_to_string(capture.dir.join(PROVENANCE))?)?;
        if !strips(&record) {
            continue;
        }
        for file in capture.jsonl_files() {
            for (i, line) in json_lines(&file)?.iter().enumerate() {
                reads.add(p, &file, i, line, false);
            }
        }
    }
    Ok(reads)
}

/// A line's kind: the hook event of an envelope, the `type` of a transcript
/// line.
fn kind_of(p: &Profile, file: &Path, line: &Value) -> String {
    if is_hook_envelope(file) {
        format!("hook:{}", (p.event_name)(line).unwrap_or("?"))
    } else {
        format!("{TX}{}", line["type"].as_str().unwrap_or("?"))
    }
}

/// Every leaf of `v` the strip would not blank (`reads::is_blank`), by its
/// pointer with `*` for an array index. A `stamped` hook envelope's shim
/// stamps are skipped, as the probe skips them (`reads::is_shim_stamp`).
fn leaves(v: &Value, stamped: bool) -> Vec<String> {
    fn walk(v: &Value, at: &str, stamped: bool, out: &mut Vec<String>) {
        if is_blank(v) {
            return;
        }
        match v {
            Value::Object(m) => {
                for (k, x) in m {
                    if !(stamped && is_shim_stamp(at, k)) {
                        walk(x, &child_pointer(at, k), stamped, out);
                    }
                }
            }
            Value::Array(a) => {
                for x in a {
                    walk(x, &format!("{at}/*"), stamped, out);
                }
            }
            _ => out.push(at.to_owned()),
        }
    }
    let mut out = Vec::new();
    walk(v, "", stamped, &mut out);
    out
}

/// A file's lines that parse; the Drive reports the ones that don't.
fn json_lines(file: &Path) -> std::io::Result<Vec<Value>> {
    Ok(std::fs::read_to_string(file)?
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

fn judge(
    p: &Profile,
    scenario: &Scenario,
    manifest: &Manifest,
    dir: &Path,
    scene: &str,
    findings: &mut Vec<(Label, String)>,
) -> std::io::Result<()> {
    // Before the Blocked return: a CLI the script lost may then exit non-zero.
    if manifest.unmatched > 0 {
        findings.push((
            Label::Script,
            format!(
                "{} model request(s) matched no script line: the request flow changed",
                manifest.unmatched
            ),
        ));
    }
    // A CLI that never ran says nothing about pixtuoid, so nothing below is judged.
    if manifest.capture.exit_code != Some(0) || manifest.requests == 0 {
        findings.push((
            Label::Blocked,
            format!(
                "the CLI did not complete a turn (exit {:?}, {} model requests)",
                manifest.capture.exit_code, manifest.requests
            ),
        ));
        return Ok(());
    }

    let payloads = hook_lines(dir)?;
    let observed: BTreeSet<String> = payloads
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| (p.event_name)(&v).map(str::to_owned))
        .collect();
    for want in scenario.expects {
        if !observed.contains(*want) {
            findings.push((
                Label::Fail,
                format!("the scenario must fire {want}; it did not"),
            ));
        }
    }

    let mut events: Vec<AgentEvent> = Vec::new();
    let mut reached: Vec<Reach> = Vec::new();
    let mut absorb = |what: &str, d: Driven, findings: &mut Vec<(Label, String)>| {
        for f in d.decode_errors.iter().chain(&d.panics) {
            findings.push((Label::Fail, format!("{what}: {f}")));
        }
        if d.unparseable > 0 {
            findings.push((
                Label::Fail,
                format!("{what}: {} line(s) not JSON", d.unparseable),
            ));
        }
        reached.extend(d.reached.iter().copied());
        events.extend(d.events.into_iter().skip(d.seed_events));
    };
    if !payloads.is_empty() {
        absorb(HOOK_PAYLOADS, Drive::hooks().lines(&payloads), findings);
    }
    for t in jsonl_files_under(dir)
        .into_iter()
        .filter(|f| !is_hook_envelope(f))
    {
        let Some(drive) = Drive::transcript_at(p.source, &t) else {
            continue;
        };
        let text = std::fs::read_to_string(&t)?;
        absorb(
            &t.display().to_string(),
            drive.seeded().lines(text.lines()),
            findings,
        );
    }

    match scenario.shape {
        Shape::ToolRun => tool_run(&events, &reached, &manifest.capture.workspace, findings),
    }
    judge_scene(p, scene, findings);
    Ok(())
}

fn tool_run(
    events: &[AgentEvent],
    reached: &[Reach],
    workspace: &Path,
    findings: &mut Vec<(Label, String)>,
) {
    let ids: BTreeSet<_> = events.iter().map(AgentEvent::agent_id).collect();
    if ids.len() != 1 {
        findings.push((
            Label::Fail,
            format!("one session must decode to ONE agent; got {}", ids.len()),
        ));
    }
    let want = canonical(workspace);
    let cwds: Vec<PathBuf> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::SessionStart { cwd, .. } => Some(canonical(cwd)),
            _ => None,
        })
        .collect();
    if cwds.is_empty() {
        findings.push((Label::Fail, "no SessionStart decoded".into()));
    } else if let Some(bad) = cwds.iter().find(|c| **c != want) {
        findings.push((
            Label::Fail,
            format!(
                "SessionStart cwd {} is not the workspace {}",
                bad.display(),
                want.display()
            ),
        ));
    }
    let named = events.iter().any(|e| {
        matches!(e, AgentEvent::ActivityStart { detail: Some(d), .. }
            if !d.display().trim().is_empty() && d.display() != "?")
    });
    if !named {
        findings.push((
            Label::Fail,
            "no tool start decoded with a detail naming the tool".into(),
        ));
    }
    if !reached.contains(&Reach::Active) {
        findings.push((Label::Fail, "the slot never went Active".into()));
    }
    if !events
        .iter()
        .any(|e| matches!(e, AgentEvent::ActivityEnd { .. }))
    {
        findings.push((Label::Fail, "the tool never ended".into()));
    }
}

/// The scene a headless pixtuoid painted, and every drift breadcrumb it logged.
/// The agent must be seen active, and gone once the CLI has exited: every
/// profiled CLI's clean exit ends its sprite, whether by an end event or by its
/// process's death, so one left standing is a session pixtuoid lost track of.
fn judge_scene(p: &Profile, scene: &str, findings: &mut Vec<(Label, String)>) {
    let prefix = format!("{}\u{b7}", p.descriptor().label_prefix);
    let frames: Vec<&str> = scene
        .lines()
        .filter_map(|l| l.split_once("agents=[").map(|(_, agents)| agents))
        .collect();
    let shows = |frame: &str, active: bool| {
        frame
            .split(", ")
            .any(|a| a.starts_with(&prefix) && (!active || a.contains(":active")))
    };
    if !frames.iter().any(|f| shows(f, true)) {
        findings.push((
            Label::Fail,
            format!("the scene never showed a {prefix} agent active"),
        ));
    }
    if frames.last().is_some_and(|last| shows(last, false)) {
        findings.push((
            Label::Fail,
            format!("the {prefix} agent was still in the scene after the CLI exited"),
        ));
    }
    for line in scene.lines().filter(|l| l.contains(BREADCRUMB_TARGET)) {
        findings.push((Label::Fail, format!("drift breadcrumb: {}", line.trim())));
    }
}

fn hook_lines(dir: &Path) -> std::io::Result<Vec<String>> {
    match std::fs::read_to_string(dir.join(HOOK_PAYLOADS)) {
        Ok(text) => Ok(text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_owned)
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// macOS's `/tmp` is `/private/tmp`: a CLI may report either.
fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::CaptureRecord;

    const RECORDED_WORKSPACE: &str = "/private/tmp/pixtuoid-capture/proj";

    /// A run directory built from a committed recording of `source`'s tool run,
    /// judged as if the tier had just produced it.
    fn run_from(source: &str, scenario_dir: &str, scene: &str) -> tempfile::TempDir {
        let d = tempfile::tempdir().expect("tempdir");
        let src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/sources/fixtures")
            .join(source)
            .join(scenario_dir);
        for f in jsonl_files_under(&src) {
            let to = d.path().join(f.strip_prefix(&src).expect("under"));
            std::fs::create_dir_all(to.parent().expect("parent")).expect("mkdir");
            std::fs::copy(&f, &to).expect("copy");
        }
        write_manifest(d.path(), source, |_| {});
        std::fs::write(d.path().join(SCENE_LOG), scene).expect("scene");
        d
    }

    fn write_manifest(dir: &Path, source: &str, edit: impl FnOnce(&mut Manifest)) {
        let mut m = Manifest {
            source: source.into(),
            scenario: Shape::ToolRun.name().into(),
            capture: CaptureRecord {
                workspace: PathBuf::from(RECORDED_WORKSPACE),
                cli: "test".into(),
                version: "test".into(),
                exit_code: Some(0),
            },
            aimock: "test".into(),
            requests: 2,
            unmatched: 0,
        };
        edit(&mut m);
        std::fs::write(dir.join(MANIFEST), serde_json::to_string(&m).expect("json"))
            .expect("write");
    }

    /// Rewrite every hook line; `None` drops it.
    fn edit_hooks(dir: &Path, mut f: impl FnMut(Value) -> Option<Value>) {
        let path = dir.join(HOOK_PAYLOADS);
        let text = std::fs::read_to_string(&path).expect("read");
        let out: Vec<String> = text
            .lines()
            .filter_map(|l| f(serde_json::from_str(l).expect("json")))
            .map(|v| v.to_string())
            .collect();
        std::fs::write(&path, out.join("\n") + "\n").expect("write");
    }

    fn blank_transcripts(dir: &Path) {
        for t in jsonl_files_under(dir)
            .into_iter()
            .filter(|f| !is_hook_envelope(f))
        {
            std::fs::write(&t, "").expect("blank");
        }
    }

    fn has(v: &Verdict, label: Label, text: &str) -> bool {
        v.findings()
            .iter()
            .any(|f| f.0 == label && f.1.contains(text))
    }

    const CX_SCENE: &str = "agents=[cx\u{b7}proj@0:active(Bash: cat NOTE.txt)]\n\
                            agents=[cx\u{b7}proj@0:idle]\nagents=[] daemons=[]\n";
    const CC_SCENE: &str = "agents=[cc\u{b7}proj@0:active(Read: NOTE.txt)]\nagents=[] daemons=[]\n";

    #[test]
    fn a_recorded_codex_tool_run_passes() {
        let d = run_from("codex", "tool-run-recorded", CX_SCENE);
        let v = verify(d.path()).expect("verify");
        assert_eq!(v.label(), Label::Pass, "{:?}", v.findings());
    }

    #[test]
    fn a_recorded_claude_code_tool_run_passes() {
        let d = run_from("claude-code", "tool-run-recorded", CC_SCENE);
        let v = verify(d.path()).expect("verify");
        assert_eq!(v.label(), Label::Pass, "{:?}", v.findings());
    }

    #[test]
    fn a_session_id_that_splits_the_run_into_two_agents_fails() {
        let d = run_from("codex", "tool-run-recorded", CX_SCENE);
        let mut first = true;
        edit_hooks(d.path(), |mut v| {
            if std::mem::take(&mut first) {
                v["session_id"] = "another-session".into();
            }
            Some(v)
        });
        assert!(has(
            &verify(d.path()).expect("verify"),
            Label::Fail,
            "ONE agent"
        ));
    }

    #[test]
    fn a_cwd_outside_the_workspace_fails() {
        let d = run_from("codex", "tool-run-recorded", CX_SCENE);
        write_manifest(d.path(), "codex", |m| {
            m.capture.workspace = PathBuf::from("/elsewhere")
        });
        let v = verify(d.path()).expect("verify");
        assert!(
            has(&v, Label::Fail, "is not the workspace"),
            "{:?}",
            v.findings()
        );
    }

    #[test]
    fn a_run_with_no_session_start_fails() {
        // Not codex: its UserPromptSubmit decodes to a SessionStart too.
        let d = run_from("claude-code", "tool-run-recorded", CC_SCENE);
        edit_hooks(d.path(), |v| {
            (v["hook_event_name"] != "SessionStart").then_some(v)
        });
        blank_transcripts(d.path());
        let v = verify(d.path()).expect("verify");
        assert!(
            has(&v, Label::Fail, "no SessionStart decoded"),
            "{:?}",
            v.findings()
        );
    }

    #[test]
    fn a_tool_that_never_ends_fails() {
        // Not codex: its Stop ends the activity too.
        let d = run_from("claude-code", "tool-run-recorded", CC_SCENE);
        edit_hooks(d.path(), |v| {
            (v["hook_event_name"] != "PostToolUse").then_some(v)
        });
        blank_transcripts(d.path());
        let v = verify(d.path()).expect("verify");
        assert!(
            has(&v, Label::Fail, "the tool never ended"),
            "{:?}",
            v.findings()
        );
    }

    #[test]
    fn a_tool_start_with_no_name_fails_and_a_slot_with_no_tool_never_goes_active() {
        let d = run_from("claude-code", "tool-run-recorded", CC_SCENE);
        // Both transports name the tool, so both lose it.
        edit_hooks(d.path(), |mut v| {
            if v.get("tool_name").is_some() {
                v["tool_name"] = "".into();
            }
            Some(v)
        });
        blank_transcripts(d.path());
        let v = verify(d.path()).expect("verify");
        assert!(
            has(&v, Label::Fail, "detail naming the tool"),
            "{:?}",
            v.findings()
        );

        edit_hooks(d.path(), |v| {
            (v["hook_event_name"] != "PreToolUse").then_some(v)
        });
        let v = verify(d.path()).expect("verify");
        assert!(
            has(&v, Label::Fail, "never went Active"),
            "{:?}",
            v.findings()
        );
    }

    #[test]
    fn a_line_the_decoder_rejects_fails() {
        let d = run_from("codex", "tool-run-recorded", CX_SCENE);
        edit_hooks(d.path(), |mut v| {
            if v["hook_event_name"] == "PreToolUse" {
                v["session_id"] = Value::Null;
            }
            Some(v)
        });
        let v = verify(d.path()).expect("verify");
        assert!(
            v.findings
                .iter()
                .any(|f| f.0 == Label::Fail && f.1.starts_with(HOOK_PAYLOADS)),
            "{:?}",
            v.findings()
        );
    }

    #[test]
    fn a_hook_the_scenario_expects_but_never_fired_fails() {
        let d = run_from("codex", "tool-run-recorded", CX_SCENE);
        edit_hooks(d.path(), |v| (v["hook_event_name"] != "Stop").then_some(v));
        let v = verify(d.path()).expect("verify");
        assert!(has(&v, Label::Fail, "must fire Stop"), "{:?}", v.findings());
    }

    #[test]
    fn a_run_the_script_lost_before_it_blocked_is_not_judged() {
        let v = Verdict::new(vec![
            (Label::Blocked, "exit 1".into()),
            (Label::Script, "1 unmatched".into()),
        ]);
        assert_eq!((v.label(), v.judged()), (Label::Script, false));
        assert!(Verdict::new(vec![(Label::Script, "1 unmatched".into())]).judged());
    }

    #[test]
    fn an_unmatched_model_request_is_script_and_a_cli_that_never_ran_is_blocked() {
        let d = run_from("codex", "tool-run-recorded", CX_SCENE);
        write_manifest(d.path(), "codex", |m| m.unmatched = 1);
        assert_eq!(verify(d.path()).expect("verify").label(), Label::Script);
        write_manifest(d.path(), "codex", |m| m.capture.exit_code = Some(1));
        assert_eq!(verify(d.path()).expect("verify").label(), Label::Blocked);
        write_manifest(d.path(), "codex", |m| m.requests = 0);
        assert_eq!(verify(d.path()).expect("verify").label(), Label::Blocked);
        write_manifest(d.path(), "codex", |m| {
            m.capture.exit_code = Some(1);
            m.unmatched = 1;
        });
        assert_eq!(
            verify(d.path()).expect("verify").label(),
            Label::Script,
            "a CLI the script lost and that then exited is the script's finding"
        );
    }

    #[test]
    fn a_scene_that_never_shows_the_agent_active_fails() {
        let d = run_from(
            "codex",
            "tool-run-recorded",
            "agents=[cx\u{b7}proj@0:idle]\nagents=[]\n",
        );
        let v = verify(d.path()).expect("verify");
        assert!(has(&v, Label::Fail, "never showed"), "{:?}", v.findings());
    }

    #[test]
    fn an_agent_still_in_the_scene_after_the_cli_exited_fails() {
        let scene =
            "agents=[cx\u{b7}proj@0:active(Bash: cat NOTE.txt)]\nagents=[cx\u{b7}proj@0:idle]\n";
        let d = run_from("codex", "tool-run-recorded", scene);
        let v = verify(d.path()).expect("verify");
        assert!(
            has(&v, Label::Fail, "after the CLI exited"),
            "{:?}",
            v.findings()
        );
    }

    #[test]
    fn a_real_drift_breadcrumb_in_the_scene_log_fails() {
        let breadcrumb = crate::test_capture::capture_logs(|| {
            crate::source::drift::missing_field("codex", "PreToolUse", "tool_name");
        });
        let d = run_from(
            "codex",
            "tool-run-recorded",
            &format!("{breadcrumb}{CX_SCENE}"),
        );
        let v = verify(d.path()).expect("verify");
        assert!(
            has(&v, Label::Fail, "drift breadcrumb"),
            "{:?}",
            v.findings()
        );
    }

    #[test]
    fn a_not_yet_hook_any_run_fires_is_stale() {
        let d = run_from("codex", "tool-run-recorded", CX_SCENE);
        let mut first = true;
        edit_hooks(d.path(), |mut v| {
            if std::mem::take(&mut first) {
                v["hook_event_name"] = "SubagentStart".into();
            }
            Some(v)
        });
        let v = verify_source("codex", &[d.path().to_path_buf()]).expect("verify");
        assert!(has(&v, Label::Stale, "SubagentStart"), "{:?}", v.findings());
    }

    #[test]
    fn a_read_field_no_run_carries_fails_the_census() {
        let d = run_from("claude-code", "tool-run-recorded", CC_SCENE);
        edit_hooks(d.path(), |mut v| {
            if let Some(name) = v.as_object_mut().and_then(|o| o.remove("tool_name")) {
                v["toolName"] = name;
            }
            Some(v)
        });
        let v = verify_source("claude-code", &[d.path().to_path_buf()]).expect("verify");
        assert!(
            has(&v, Label::Fail, "hook:PreToolUse /tool_name"),
            "{:?}",
            v.findings()
        );
    }

    #[test]
    fn a_not_yet_field_a_run_carries_is_stale() {
        let d = run_from("claude-code", "tool-run-recorded", CC_SCENE);
        edit_hooks(d.path(), |mut v| {
            if v["hook_event_name"] == "SessionStart" {
                v["model"] = "claude-test".into();
            }
            Some(v)
        });
        let v = verify_source("claude-code", &[d.path().to_path_buf()]).expect("verify");
        assert!(
            has(&v, Label::Stale, "hook:SessionStart /model"),
            "{:?}",
            v.findings()
        );
    }

    #[test]
    fn a_stamped_envelope_drops_its_top_level_shim_stamps_only() {
        let v = serde_json::json!({"_shim_ts_ms": 5, "_pid": 7, "a": {"_k": 1}});
        assert_eq!(leaves(&v, true), ["/a/_k"]);
        assert_eq!(leaves(&v, false).len(), 3);
    }

    fn fields(kinds: &[(&str, &[&str])], lines: &[&[&str]]) -> Fields {
        let set = |paths: &[&str]| {
            paths
                .iter()
                .map(|p| (*p).to_owned())
                .collect::<BTreeSet<_>>()
        };
        Fields {
            by_kind: kinds
                .iter()
                .map(|(k, p)| ((*k).to_owned(), set(p)))
                .collect(),
            lines: lines.iter().map(|l| set(l)).collect(),
        }
    }

    #[test]
    fn the_census_fails_a_read_field_not_carried_unless_owed_and_stales_one_carried() {
        let reads = fields(
            &[("hook:A", &["/x", "/owed", "/owed2"]), ("hook:B", &["/y"])],
            &[],
        );
        let carried = fields(&[("hook:A", &["/owed2"])], &[]);
        let owed = [("hook:A", "/owed"), ("hook:A", "/owed2")];
        let got = census(&reads, &carried, &owed, "s");
        assert_eq!(
            got,
            [
                (Label::Stale, "hook:A /owed2 was carried: it leaves the profile's not_yet_fields".to_owned()),
                (Label::Fail, "hook:A /x: a decoder reads it in a committed capture, and no run of s carried it".to_owned()),
            ],
            "an owed field not carried is silent, and kind B, which no run produced, is not judged"
        );
    }

    #[test]
    fn an_untyped_committed_line_needs_one_run_line_carrying_all_its_reads() {
        let reads = fields(&[], &[&["/payload/cwd", "/payload/id"]]);
        let split = fields(&[], &[&["/payload/cwd"], &["/payload/id"]]);
        assert_eq!(
            census(&reads, &split, &[], "s").len(),
            1,
            "two lines don't make one"
        );
        let whole = fields(&[], &[&["/payload/cwd", "/payload/id", "/payload/more"]]);
        assert!(census(&reads, &whole, &[], "s").is_empty());
        assert!(
            census(&reads, &Fields::default(), &[], "s").is_empty(),
            "no transcript ran"
        );
    }

    #[test]
    fn the_committed_corpus_is_a_legitimate_run_the_census_passes() {
        let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sources");
        for p in super::super::PROFILES {
            let dirs: Vec<PathBuf> = captures(&corpus)
                .into_iter()
                .filter(|c| c.source == p.source)
                .map(|c| c.dir)
                .collect();
            let v = verify_source(p.source, &dirs).expect("verify");
            assert!(
                v.findings().iter().all(|f| f.0 != Label::Fail),
                "{}: {:?}",
                p.source,
                v.findings()
            );
        }
    }

    #[test]
    fn a_rollout_head_that_loses_its_cwd_fails_the_census() {
        let d = run_from("codex", "tool-run-recorded", CX_SCENE);
        for t in jsonl_files_under(d.path())
            .into_iter()
            .filter(|f| !is_hook_envelope(f))
        {
            let text = std::fs::read_to_string(&t).expect("read");
            let mut lines: Vec<Value> = text
                .lines()
                .map(|l| serde_json::from_str(l).expect("json"))
                .collect();
            if let Some(head) = lines.first_mut().and_then(|h| h["payload"].as_object_mut()) {
                head.remove("cwd");
            }
            let out: Vec<String> = lines.iter().map(Value::to_string).collect();
            std::fs::write(&t, out.join("\n") + "\n").expect("write");
        }
        let v = verify_source("codex", &[d.path().to_path_buf()]).expect("verify");
        assert!(
            has(&v, Label::Fail, "tx:#head /payload/cwd"),
            "{:?}",
            v.findings()
        );
    }

    #[test]
    fn a_leaf_is_blank_exactly_where_the_probe_neutralizes() {
        let v = serde_json::json!({
            "s": "x", "n": 1, "f": 0.0, "b": true, "a": [{"k": "y"}], "l/m": "z", "z": null
        });
        let got: BTreeSet<String> = leaves(&v, false).into_iter().collect();
        let want: BTreeSet<String> = ["/s", "/n", "/f", "/b", "/a/*/k", "/l~1m"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(got, want, "0.0 is a value: the probe neutralizes it to 0");
        // What the probe writes over each leaf is blank to the census.
        for (_, x) in v.as_object().expect("an object") {
            if let Some(neutral) = crate::harness::reads::neutral_of(x) {
                assert!(leaves(&neutral, false).is_empty(), "{neutral}");
            }
        }
    }
}
