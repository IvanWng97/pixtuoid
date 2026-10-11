use std::cell::RefCell;
use std::collections::HashMap;

use super::*;

/// The network as a table: a URL it lacks is a transient failure, so a test
/// states every document it serves.
#[derive(Default)]
struct Served {
    bodies: HashMap<String, Result<Vec<u8>, u16>>,
    calls: RefCell<Vec<(String, Duration)>>,
}

impl Served {
    fn with(mut self, url: &str, body: &str) -> Self {
        self.bodies
            .insert(url.to_owned(), Ok(body.as_bytes().to_vec()));
        self
    }

    fn gone(mut self, url: &str, status: u16) -> Self {
        self.bodies.insert(url.to_owned(), Err(status));
        self
    }

    fn fetched(&self, url: &str) -> usize {
        self.calls.borrow().iter().filter(|(u, _)| u == url).count()
    }
}

impl Fetch for Served {
    fn get(&self, url: &str, timeout: Duration) -> Result<Vec<u8>, FetchError> {
        self.calls.borrow_mut().push((url.to_owned(), timeout));
        match self.bodies.get(url) {
            Some(Ok(b)) => Ok(b.clone()),
            Some(Err(status)) => Err(FetchError::Gone { status: *status }),
            None => Err(FetchError::Transient(format!("not served: {url}"))),
        }
    }
}

const REPO: Project = Project::Repo {
    repo: "o/r",
    branch: "main",
};

fn raw(path: &str) -> String {
    format!("{RAW_URL}/o/r/main/{path}")
}

const ANCHORED: &str = r"(?m)^enum Events \{";

const fn doc(path: &'static str) -> Doc {
    Doc {
        at: At::Path(path),
        anchor: ANCHORED,
        owns: "`Events`",
    }
}

const fn each(docs: &'static [Doc], pattern: &'static str) -> Watch {
    Watch {
        id: "t.each",
        ours: || vec!["a.b", "c"],
        docs,
        read: Read::Each { pattern },
        vanish: Finding::Breaking("no sprite"),
        siblings: None,
    }
}

fn check(served: &Served, w: &Watch) -> Report {
    let mut report = Report::default();
    let mut docs = Docs::new(served);
    check_watch("src", &REPO, w, &mut docs, &mut report);
    report
}

#[test]
fn a_vanished_name_is_breaking_and_a_present_one_is_silent() {
    const W: Watch = each(&[doc("e.ts")], r#""{name}""#);
    let both = Served::default().with(&raw("e.ts"), "enum Events {\n\"a.b\" \"c\"");
    let r = check(&both, &W);
    assert!(r.breaking.is_empty() && r.blind.is_empty(), "{r:?}");

    // `a.b` is escaped: a regex `.` would let `axb` stand in for it.
    let renamed = Served::default().with(&raw("e.ts"), "enum Events {\n\"axb\" \"c\"");
    let r = check(&renamed, &W);
    assert_eq!(r.breaking.len(), 1, "{r:?}");
    assert!(
        r.breaking[0].contains("`a.b`") && r.breaking[0].contains("no sprite"),
        "{r:?}"
    );
}

#[test]
fn a_multi_document_watch_is_all_or_nothing() {
    const W: Watch = each(&[doc("1.ts"), doc("2.ts")], r#""{name}""#);
    // `c` lives only in the second half, so reading the first alone would
    // report it gone when the second merely failed to fetch.
    let half = Served::default()
        .with(&raw("1.ts"), "enum Events {\n\"a.b\"")
        .gone(&raw("2.ts"), 404);
    let r = check(&half, &W);
    assert!(
        r.breaking.is_empty(),
        "a fetch failure is never a vanish: {r:?}"
    );
    assert_eq!(r.blind.len(), 1, "a gone pin is probe health: {r:?}");
    assert!(r.blind[0].contains("HTTP 404"), "{r:?}");

    let flaky = Served::default().with(&raw("1.ts"), "enum Events {\n\"a.b\"");
    let r = check(&flaky, &W);
    assert!(r.breaking.is_empty() && r.blind.is_empty(), "{r:?}");
    assert_eq!(
        r.errors.len(),
        1,
        "a transient failure is an error line: {r:?}"
    );
}

#[test]
fn a_missing_anchor_is_probe_health_filed_once_per_document() {
    const W1: Watch = each(&[doc("e.ts")], r#""{name}""#);
    const W2: Watch = each(&[doc("e.ts")], r#"'{name}'"#);
    // A re-export facade: it fetches, and declares nothing.
    let facade = Served::default().with(&raw("e.ts"), "export * from './events';");
    let mut report = Report::default();
    let mut docs = Docs::new(&facade);
    check_watch("src", &REPO, &W1, &mut docs, &mut report);
    check_watch("src", &REPO, &W2, &mut docs, &mut report);
    assert!(report.breaking.is_empty(), "{report:?}");
    assert_eq!(report.blind.len(), 1, "{report:?}");
    assert!(
        report.blind[0].contains("NOT evidence that upstream changed"),
        "{report:?}"
    );
    assert_eq!(
        facade.fetched(&raw("e.ts")),
        1,
        "a document is fetched once per run"
    );
}

#[test]
fn a_reader_that_finds_no_declaration_is_probe_health() {
    const W: Watch = Watch {
        id: "t.union",
        ours: || vec!["c"],
        docs: &[Doc {
            at: At::Path("s.json"),
            anchor: r#""Events""#,
            owns: "`Events`",
        }],
        read: Read::JsonUnion {
            def: "Events",
            prop: None,
        },
        vanish: Finding::Breaking("x"),
        siblings: None,
    };
    // Anchored, and the union moved out from under the reader.
    let moved = Served::default().with(&raw("s.json"), r#"{"Events": 1}"#);
    let r = check(&moved, &W);
    assert!(r.breaking.is_empty() && r.blind.len() == 1, "{r:?}");
}

#[test]
fn the_floor_skips_a_degraded_parse() {
    const SCHEMA: &str = r#"{"$defs": {"Events": {"oneOf": [{"const": "c"}]}}}"#;
    const DOC: Doc = Doc {
        at: At::Path("s.json"),
        anchor: r#""Events""#,
        owns: "`Events`",
    };
    const UNION: Watch = Watch {
        id: "t.union",
        ours: || vec!["a.b", "c"],
        docs: &[DOC],
        read: Read::JsonUnion {
            def: "Events",
            prop: None,
        },
        vanish: Finding::Breaking("x"),
        siblings: None,
    };
    // One name back where we read two: the reader degraded, nothing is renamed.
    let r = check(&Served::default().with(&raw("s.json"), SCHEMA), &UNION);
    assert!(r.breaking.is_empty() && r.blind.len() == 1, "{r:?}");
    assert!(r.blind[0].contains("floor 2"), "{r:?}");
}

#[test]
fn a_sibling_in_the_family_is_reviewed_and_a_stale_ignore_is_reported() {
    const SCHEMA: &str = r#"{"$defs": {"Events": {"oneOf": [
        {"const": "tool_call"}, {"const": "tool_call_update"},
        {"const": "tool_call_cancelled"}, {"const": "tool_call_deferred"},
        {"const": "plan"}, {"const": "agent_message"}]}}}"#;
    const W: Watch = Watch {
        id: "t.sib",
        ours: || vec!["tool_call", "tool_call_update"],
        docs: &[Doc {
            at: At::Path("s.json"),
            anchor: r#""Events""#,
            owns: "`Events`",
        }],
        read: Read::JsonUnion {
            def: "Events",
            prop: None,
        },
        vanish: Finding::Breaking("x"),
        siblings: Some(Siblings {
            family: Family::Prefix("tool_call"),
            ignored: &[
                ("tool_call_deferred", "decided"),
                ("tool_call_progress", "gone upstream"),
            ],
        }),
    };
    let r = check(&Served::default().with(&raw("s.json"), SCHEMA), &W);
    assert!(r.breaking.is_empty() && r.blind.is_empty(), "{r:?}");
    assert_eq!(r.review.len(), 2, "{r:?}");
    assert!(
        r.review.iter().any(|l| l.contains("`tool_call_cancelled`")),
        "{r:?}"
    );
    assert!(
        r.review
            .iter()
            .any(|l| l.contains("still ignores `tool_call_progress`")),
        "{r:?}"
    );
    assert!(
        !r.review.iter().any(|l| l.contains("`tool_call_deferred`")),
        "an ignored sibling goes quiet: {r:?}"
    );
    // A stranger to the family is mere growth.
    assert!(!r.review.iter().any(|l| l.contains("`plan`")), "{r:?}");
}

/// A declared pattern as text it matches, `{name}` standing for `name`. A construct it cannot render fails the test, which
/// keeps declared patterns to shapes a reader can tell names apart in.
fn render(pattern: &str, name: &str) -> String {
    let mut out = String::new();
    let mut rest = pattern;
    while !rest.is_empty() {
        let (emit, used) = {
            let fixed: &[(&str, &str)] = &[
                ("{name}", name),
                ("(?m)", ""),
                ("^", "\n"),
                (r"\s*", ""),
                (r"\s+", " "),
                (r"\b", ""),
            ];
            match fixed.iter().find(|(p, _)| rest.starts_with(p)) {
                Some((p, e)) => ((*e).to_owned(), p.len()),
                None => {
                    let c = rest.chars().next().unwrap_or_default();
                    if let Some(escaped) = rest.strip_prefix('\\') {
                        let e = escaped.chars().next().unwrap_or_default();
                        assert!(
                            e.is_ascii_punctuation(),
                            "cannot render `\\{e}` in {pattern}"
                        );
                        (e.to_string(), 1 + e.len_utf8())
                    } else {
                        assert!(
                            !"[]()*+?|.$".contains(c),
                            "cannot render `{c}` in {pattern}"
                        );
                        (c.to_string(), c.len_utf8())
                    }
                }
            }
        };
        out.push_str(&emit);
        rest = &rest[used..];
    }
    out
}

#[test]
fn rendering_a_pattern_yields_text_it_matches() {
    for (pattern, name) in [
        (r#""x-method"\s*:\s*"{name}""#, "session/update"),
        (r"Schema\.Literal\({name}\)", "a.b"),
        (r"(?m)^export const \b{name} = \{", "Event"),
    ] {
        let text = render(pattern, name);
        let re = Regex::new(&pattern.replace("{name}", &regex::escape(name))).expect("compiles");
        assert!(re.is_match(&text), "{pattern} renders {text:?}");
    }
}

/// #941's gate, generic: every watch whose pattern carries our names fires
/// when one vanishes and stays silent while all are there. The vanished name
/// leaves a decoy in its own shape, so a pattern loose enough to match the
/// decoy (a bare substring where the declaration is quoted) cannot pass.
#[test]
fn every_name_bearing_watch_fires_on_a_vanish_and_stays_silent_otherwise() {
    let mut proven = 0;
    for d in REGISTRY {
        for up in d.upstream {
            for w in up.watches {
                let pattern = match w.read {
                    Read::Each { pattern } => pattern,
                    _ => continue,
                };
                let page = |names: &[&str]| {
                    let anchors: Vec<String> =
                        w.docs.iter().map(|doc| render(doc.anchor, "")).collect();
                    let decls: Vec<String> = names.iter().map(|n| render(pattern, n)).collect();
                    format!("{}\n{}\n", anchors.join("\n"), decls.join("\n"))
                };
                let serve = |text: &str| {
                    w.docs.iter().fold(Served::default(), |s, doc| {
                        let (url, _) = locate(&up.project, &doc.at);
                        s.with(&url, text)
                    })
                };
                let run_on = |text: &str| {
                    let served = serve(text);
                    let mut report = Report::default();
                    let mut docs = Docs::new(&served);
                    check_watch(d.name, &up.project, w, &mut docs, &mut report);
                    report
                };
                let ours = (w.ours)();
                let intact = run_on(&page(&ours));
                assert!(
                    intact.breaking.is_empty() && intact.blind.is_empty(),
                    "{}: an intact document stays silent: {intact:?}",
                    w.id
                );
                let victim = ours[0];
                let decoy = format!("{victim}_pxd");
                let mut kept: Vec<&str> = ours[1..].to_vec();
                kept.push(&decoy);
                let vanished = run_on(&page(&kept));
                assert!(
                    vanished
                        .breaking
                        .iter()
                        .any(|l| l.contains(&format!("`{victim}`"))),
                    "{}: a vanished `{victim}` fires: {vanished:?}",
                    w.id
                );
                proven += 1;
            }
        }
    }
    assert!(proven > 0, "the registry declares name-bearing watches");
}

#[test]
fn the_report_title_carries_the_strongest_disposition_and_the_exit_code() {
    for file in [Report::add_breaking, Report::add_review] {
        let mut one = Report::default();
        file(&mut one, "L".to_owned());
        assert_eq!(one.exit_code(), 1);
    }
    let mut r = Report::default();
    assert_eq!(
        (r.title(), r.exit_code()),
        ("Upstream wire-format watch: no drift", 0)
    );
    assert!(r.render().contains("✅ No drift"));
    r.add_error("E".to_owned());
    assert_eq!(r.exit_code(), 2, "transient alone is warn-only");
    r.add_blind("W", "AT", "C", false);
    assert_eq!(
        (r.title(), r.exit_code()),
        ("Upstream drift watch could not verify — repin needed", 1),
        "a dark watch is actionable (#454)"
    );
    r.add_review("R".to_owned());
    assert_eq!(r.title(), "New upstream events to review");
    r.add_breaking("B".to_owned());
    assert_eq!(r.title(), "Upstream CLI wire-format drift detected");
    let out = r.render();
    assert!(
        out.starts_with("# Upstream CLI wire-format drift detected\n"),
        "{out}"
    );
    let at = |s: &str| out.find(s).unwrap_or(usize::MAX);
    assert!(
        at("- B") < at("- R") && at("- R") < at("- could not") && at("- could not") < at("- E")
    );
}

/// Every declared pattern compiles under the engine that runs it, and no
/// anchor matches a name it guards: such an anchor vanishes WITH the names on
/// the very rename it exists to tell apart from a stale pin.
#[test]
fn every_declared_pattern_compiles_and_no_anchor_is_a_guarded_name() {
    let mut watches = 0;
    for w in REGISTRY
        .iter()
        .flat_map(|d| d.upstream)
        .flat_map(|up| up.watches)
    {
        watches += 1;
        let ours = (w.ours)();
        for doc in w.docs {
            let anchor = Regex::new(doc.anchor).unwrap_or_else(|e| panic!("{}: {e}", w.id));
            for name in &ours {
                assert!(
                    !anchor.is_match(name),
                    "{}: the anchor matches `{name}`",
                    w.id
                );
            }
        }
        if let Read::Each { pattern } = w.read {
            Regex::new(&pattern.replace("{name}", "n")).unwrap_or_else(|e| panic!("{}: {e}", w.id));
        }
    }
    assert!(watches > 0, "the registry declares watches");
}

#[test]
fn the_legacy_buckets_merge_and_a_renamed_one_is_refused() {
    let mut r = Report::default();
    let json = r#"{"breaking": ["B"], "review": [], "blind": ["D"], "errors": ["E"]}"#;
    merge_legacy(&mut r, json.as_bytes()).expect("the four buckets merge");
    assert_eq!((r.breaking.len(), r.blind.len(), r.errors.len()), (1, 1, 1));
    // A key renamed on either side would otherwise drop its findings and exit 0.
    for bad in [
        r#"{"breaking": [], "review": [], "blind": []}"#,
        r#"{"broken": [], "breaking": [], "review": [], "blind": [], "errors": []}"#,
        "Traceback (most recent call last):",
    ] {
        assert!(
            merge_legacy(&mut Report::default(), bad.as_bytes()).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn a_fetched_name_cannot_leave_its_code_span() {
    assert_eq!(
        code_span("tool_call`](https://x)\nmore"),
        "tool_call](https://x)more"
    );
}
