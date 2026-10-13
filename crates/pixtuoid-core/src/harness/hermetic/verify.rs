//! Judging one hermetic run: the CLI's real bytes through the one pipeline
//! (`Drive`), and the scene a headless pixtuoid painted from the same run.
//! Ids, timestamps, token counts and model text change every run, so nothing
//! here compares bytes or goldens: only shapes.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{MANIFEST, Manifest, Profile, SCENE_LOG, Scenario, Shape, profile};
use crate::AgentEvent;
use crate::harness::{Drive, Driven, HOOK_PAYLOADS, Reach};

/// The tracing target every drift breadcrumb rides (`source::drift::TARGET`).
const BREADCRUMB_TARGET: &str = crate::source::drift::TARGET;

/// A run's outcome, ordered by severity: a verdict is its worst finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Label {
    /// Every check held.
    Pass,
    /// A hook in the profile's `not_yet` fired: good news, the list shrinks.
    Stale,
    /// The CLI could not run (install, network, a login wall): not judged.
    Blocked,
    /// The CLI asked the model something no script line answers: its request
    /// flow changed upstream.
    Script,
    /// pixtuoid read the run wrong.
    Fail,
}

/// One run's verdict.
#[derive(Debug)]
pub struct Verdict {
    /// The worst finding's label.
    pub label: Label,
    /// Every finding, worst first.
    pub findings: Vec<(Label, String)>,
}

/// Judge the run in `dir`.
///
/// # Errors
///
/// If the manifest or a capture file cannot be read, or the manifest names no
/// profiled scenario.
pub fn verify(dir: &Path) -> std::io::Result<Verdict> {
    let manifest: Manifest = serde_json::from_str(&std::fs::read_to_string(dir.join(MANIFEST))?)?;
    let other = |m: String| std::io::Error::new(std::io::ErrorKind::InvalidData, m);
    let p = profile(&manifest.source)
        .ok_or_else(|| other(format!("{}: not a profiled source", manifest.source)))?;
    let scenario = p
        .scenarios
        .iter()
        .find(|s| s.shape.name() == manifest.scenario)
        .ok_or_else(|| other(format!("{}: no scenario {}", p.source, manifest.scenario)))?;
    let scene = std::fs::read_to_string(dir.join(SCENE_LOG)).unwrap_or_default();
    let mut findings = Vec::new();
    judge(p, scenario, &manifest, dir, &scene, &mut findings)?;
    findings.sort_by_key(|f| std::cmp::Reverse(f.0));
    let label = findings.first().map_or(Label::Pass, |f| f.0);
    Ok(Verdict { label, findings })
}

fn judge(
    p: &Profile,
    scenario: &Scenario,
    manifest: &Manifest,
    dir: &Path,
    scene: &str,
    findings: &mut Vec<(Label, String)>,
) -> std::io::Result<()> {
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
    if manifest.unmatched > 0 {
        findings.push((
            Label::Script,
            format!(
                "{} model request(s) matched no script line: the request flow changed",
                manifest.unmatched
            ),
        ));
    }

    let payloads = hook_lines(dir)?;
    let observed: BTreeSet<String> = payloads
        .iter()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
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
    for n in p.not_yet {
        if observed.contains(*n) {
            findings.push((
                Label::Stale,
                format!("{n} fired: it leaves the profile's not_yet"),
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
    for t in transcripts(dir)? {
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
fn judge_scene(p: &Profile, scene: &str, findings: &mut Vec<(Label, String)>) {
    let prefix = p.descriptor().label_prefix;
    let active = scene.lines().any(|l| {
        l.split("agents=[").nth(1).is_some_and(|agents| {
            agents
                .split(", ")
                .any(|a| a.starts_with(&format!("{prefix}\u{b7}")) && a.contains(":active"))
        })
    });
    if !active {
        findings.push((
            Label::Fail,
            format!("the scene never showed a {prefix}\u{b7} agent active"),
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

/// Every `.jsonl` under the run that is not the hook file: the transcripts the
/// recorder harvested.
fn transcripts(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let capture = crate::harness::Capture {
        dir: dir.to_path_buf(),
        source: String::new(),
    };
    Ok(capture
        .jsonl_files()
        .into_iter()
        .filter(|f| f.file_name().and_then(|n| n.to_str()) != Some(HOOK_PAYLOADS))
        .collect())
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
        for f in (crate::harness::Capture {
            dir: src.clone(),
            source: source.into(),
        })
        .jsonl_files()
        {
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

    fn edit_hooks(dir: &Path, mut f: impl FnMut(&mut serde_json::Value)) {
        let path = dir.join(HOOK_PAYLOADS);
        let text = std::fs::read_to_string(&path).expect("read");
        let out: Vec<String> = text
            .lines()
            .map(|l| {
                let mut v: serde_json::Value = serde_json::from_str(l).expect("json");
                f(&mut v);
                v.to_string()
            })
            .collect();
        std::fs::write(&path, out.join("\n") + "\n").expect("write");
    }

    const CX_SCENE: &str =
        "agents=[cx\u{b7}proj@0:active(Bash: cat NOTE.txt)]\nagents=[cx\u{b7}proj@0:idle]\n";
    const CC_SCENE: &str = "agents=[cc\u{b7}proj@0:active(Read: NOTE.txt)]\n";

    #[test]
    fn a_recorded_codex_tool_run_passes() {
        let d = run_from("codex", "tool-run-recorded", CX_SCENE);
        let v = verify(d.path()).expect("verify");
        assert_eq!(v.label, Label::Pass, "{:?}", v.findings);
    }

    #[test]
    fn a_recorded_claude_code_tool_run_passes() {
        let d = run_from("claude-code", "tool-run-recorded", CC_SCENE);
        let v = verify(d.path()).expect("verify");
        assert_eq!(v.label, Label::Pass, "{:?}", v.findings);
    }

    #[test]
    fn a_session_id_that_splits_the_run_into_two_agents_fails() {
        let d = run_from("codex", "tool-run-recorded", CX_SCENE);
        let mut first = true;
        edit_hooks(d.path(), |v| {
            if std::mem::take(&mut first) {
                v["session_id"] = "another-session".into();
            }
        });
        let v = verify(d.path()).expect("verify");
        assert_eq!(v.label, Label::Fail);
        assert!(
            v.findings.iter().any(|f| f.1.contains("ONE agent")),
            "{:?}",
            v.findings
        );
    }

    #[test]
    fn a_cwd_outside_the_workspace_fails() {
        let d = run_from("codex", "tool-run-recorded", CX_SCENE);
        write_manifest(d.path(), "codex", |m| {
            m.capture.workspace = PathBuf::from("/elsewhere")
        });
        let v = verify(d.path()).expect("verify");
        assert!(
            v.findings
                .iter()
                .any(|f| f.1.contains("is not the workspace")),
            "{:?}",
            v.findings
        );
    }

    #[test]
    fn a_tool_start_with_no_name_fails() {
        let d = run_from("claude-code", "tool-run-recorded", CC_SCENE);
        // Both transports name the tool, so both lose it.
        edit_hooks(d.path(), |v| {
            if v.get("tool_name").is_some() {
                v["tool_name"] = "".into();
            }
        });
        for t in transcripts(d.path()).expect("walk") {
            std::fs::write(&t, "").expect("blank");
        }
        let v = verify(d.path()).expect("verify");
        assert!(
            v.findings
                .iter()
                .any(|f| f.1.contains("detail naming the tool")),
            "{:?}",
            v.findings
        );
    }

    #[test]
    fn a_hook_the_scenario_expects_but_never_fired_fails() {
        let d = run_from("codex", "tool-run-recorded", CX_SCENE);
        let path = d.path().join(HOOK_PAYLOADS);
        let kept: Vec<String> = std::fs::read_to_string(&path)
            .expect("read")
            .lines()
            .filter(|l| !l.contains("\"Stop\""))
            .map(str::to_owned)
            .collect();
        std::fs::write(&path, kept.join("\n") + "\n").expect("write");
        let v = verify(d.path()).expect("verify");
        assert!(
            v.findings.iter().any(|f| f.1.contains("must fire Stop")),
            "{:?}",
            v.findings
        );
    }

    #[test]
    fn an_unmatched_model_request_is_script_and_a_cli_that_never_ran_is_blocked() {
        let d = run_from("codex", "tool-run-recorded", CX_SCENE);
        write_manifest(d.path(), "codex", |m| m.unmatched = 1);
        assert_eq!(verify(d.path()).expect("verify").label, Label::Script);
        write_manifest(d.path(), "codex", |m| m.capture.exit_code = Some(1));
        assert_eq!(verify(d.path()).expect("verify").label, Label::Blocked);
        write_manifest(d.path(), "codex", |m| m.requests = 0);
        assert_eq!(verify(d.path()).expect("verify").label, Label::Blocked);
    }

    #[test]
    fn a_scene_that_never_shows_the_agent_active_fails() {
        let d = run_from(
            "codex",
            "tool-run-recorded",
            "agents=[cx\u{b7}proj@0:idle]\n",
        );
        let v = verify(d.path()).expect("verify");
        assert!(
            v.findings.iter().any(|f| f.1.contains("never showed")),
            "{:?}",
            v.findings
        );
    }

    #[test]
    fn a_drift_breadcrumb_in_the_scene_log_fails() {
        let scene = format!(
            "{CX_SCENE}WARN {BREADCRUMB_TARGET}: a field went missing source=codex kind=missing_field\n"
        );
        let d = run_from("codex", "tool-run-recorded", &scene);
        let v = verify(d.path()).expect("verify");
        assert!(
            v.findings
                .iter()
                .any(|f| f.1.starts_with("drift breadcrumb")),
            "{:?}",
            v.findings
        );
    }

    #[test]
    fn a_not_yet_hook_that_fires_is_stale_not_fail() {
        let d = run_from("codex", "tool-run-recorded", CX_SCENE);
        let path = d.path().join(HOOK_PAYLOADS);
        let mut text = std::fs::read_to_string(&path).expect("read");
        let first = text.lines().next().expect("a line").to_string();
        let mut extra: serde_json::Value = serde_json::from_str(&first).expect("json");
        extra["hook_event_name"] = "SubagentStart".into();
        text.push_str(&format!("{extra}\n"));
        std::fs::write(&path, text).expect("write");
        let v = verify(d.path()).expect("verify");
        assert!(
            v.findings
                .iter()
                .any(|f| f.0 == Label::Stale && f.1.contains("SubagentStart")),
            "{:?}",
            v.findings
        );
    }
}
