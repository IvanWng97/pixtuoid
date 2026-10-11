use std::cell::RefCell;
use std::collections::HashMap;

use super::*;

/// The network as a table: a URL it lacks is a transient failure, so a test
/// states every document it serves.
#[derive(Default)]
struct Served {
    bodies: HashMap<String, Result<Vec<u8>, u16>>,
    tarballs: HashMap<String, Vec<u8>>,
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

    /// `repo`'s latest release `v1.0.0`, whose vouched tarball `asset` holds
    /// `text` at `member`.
    fn release(mut self, repo: &str, asset: &str, member: &str, sums: &str, text: &str) -> Self {
        let base = format!("https://github.com/{repo}/releases/download/v1.0.0/");
        let name = asset.replace("{version}", "1.0.0");
        let tarball = format!("TARBALL {repo} {name}");
        let digest = format!("{:x}", Sha256::digest(tarball.as_bytes()));
        self.tarballs
            .insert(format!("{tarball}#{member}"), text.as_bytes().to_vec());
        self.with(
            &format!("{API_URL}/{repo}/releases/latest"),
            r#"{"tag_name": "v1.0.0"}"#,
        )
        .with(&format!("{base}{name}"), &tarball)
        .with(&format!("{base}{sums}"), &format!("{digest}  {name}\n"))
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

    fn untar(&self, tarball: &[u8], member: &str) -> Result<Option<Vec<u8>>, FetchError> {
        let key = format!("{}#{member}", String::from_utf8_lossy(tarball));
        Ok(self.tarballs.get(&key).cloned())
    }
}

const REPO: Upstream = Upstream {
    repo: "o/r",
    watches: &[],
};

fn raw(path: &str) -> String {
    format!("{RAW_URL}/o/r/main/{path}")
}

const ANCHORED: &str = r"(?m)^enum Events \{";

const fn doc(path: &'static str) -> Doc {
    Doc {
        at: At::Path {
            path,
            branch: "main",
        },
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
        vanish: "no sprite",
        siblings: None,
    }
}

fn check(served: &Served, w: &Watch) -> Report {
    let mut report = Report::default();
    let mut fetched = Fetched::new(served);
    check_watch("src", &REPO, w, &mut fetched, &mut report);
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
        !r.breaking[0].contains("there now"),
        "only a sole read names upstream's value"
    );
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
    let mut fetched = Fetched::new(&facade);
    check_watch("src", &REPO, &W1, &mut fetched, &mut report);
    check_watch("src", &REPO, &W2, &mut fetched, &mut report);
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
            at: At::Path {
                path: "s.json",
                branch: "main",
            },
            anchor: r#""Events""#,
            owns: "`Events`",
        }],
        read: Read::JsonUnion {
            def: "Events",
            prop: None,
        },
        vanish: "x",
        siblings: None,
    };
    // Anchored, and the union moved out from under the reader.
    let moved = Served::default().with(&raw("s.json"), r#"{"Events": 1}"#);
    let r = check(&moved, &W);
    assert!(r.breaking.is_empty() && r.blind.len() == 1, "{r:?}");
    assert!(r.blind[0].contains("found no declaration"), "{r:?}");
}

/// A union read off the documents that parsed would hide whatever changed in
/// the one that did not, however many names the others still declare.
#[test]
fn one_unreadable_document_skips_the_whole_union() {
    const UNION: Doc = Doc {
        at: At::Path {
            path: "a.json",
            branch: "main",
        },
        anchor: r#""Events""#,
        owns: "`Events`",
    };
    const W: Watch = Watch {
        id: "t.two",
        ours: || vec!["c"],
        docs: &[
            UNION,
            Doc {
                at: At::Path {
                    path: "b.json",
                    branch: "main",
                },
                ..UNION
            },
        ],
        read: Read::JsonUnion {
            def: "Events",
            prop: None,
        },
        vanish: "x",
        siblings: None,
    };
    let served = Served::default()
        .with(
            &raw("a.json"),
            r#"{"$defs": {"Events": {"oneOf": [{"const": "c"}]}}}"#,
        )
        .with(&raw("b.json"), r#"{"Events": "reshaped"}"#);
    let r = check(&served, &W);
    assert!(r.breaking.is_empty() && r.blind.len() == 1, "{r:?}");
    assert!(
        r.blind[0].contains("b.json"),
        "the line names the document: {r:?}"
    );
}

/// A pattern or anchor of ours that cannot compile is ours to fix: the
/// upstream wording would send the maintainer to repin a working pin.
#[test]
fn a_broken_pattern_of_ours_blames_ours() {
    const EACH: Watch = each(&[doc("e.ts")], r"({name}");
    const SOLE: Watch = Watch {
        id: "t.sole",
        ours: || vec!["agent"],
        docs: &[doc("e.ts")],
        read: Read::Sole { pattern: "(" },
        vanish: "x",
        siblings: None,
    };
    const ARMS: Watch = Watch {
        id: "t.arms",
        ours: || vec!["start"],
        docs: &[doc("e.ts")],
        read: Read::RustArms {
            name: "parse",
            arm: "(",
        },
        vanish: "x",
        siblings: None,
    };
    for w in [&EACH, &SOLE, &ARMS] {
        let r = check(&Served::default().with(&raw("e.ts"), "enum Events {\n"), w);
        assert!(r.breaking.is_empty() && r.blind.len() == 1, "{r:?}");
        assert!(r.blind[0].contains("Our own declaration"), "{r:?}");
        assert!(
            r.blind[0].contains(&format!("`{}` was SKIPPED", w.id)),
            "{r:?}"
        );
    }
}

/// A failed fetch is upstream's answer for the run too: re-asking per watch
/// would file the same dark pin once per watch and wait out each timeout.
#[test]
fn a_failed_document_is_fetched_and_filed_once() {
    const W1: Watch = each(&[doc("e.ts")], r#""{name}""#);
    const W2: Watch = each(&[doc("e.ts")], r#"'{name}'"#);
    for (served, gone) in [
        (Served::default().gone(&raw("e.ts"), 404), true),
        (Served::default(), false),
    ] {
        let mut report = Report::default();
        let mut fetched = Fetched::new(&served);
        check_watch("src", &REPO, &W1, &mut fetched, &mut report);
        check_watch("src", &REPO, &W2, &mut fetched, &mut report);
        let filed = if gone { &report.blind } else { &report.errors };
        assert_eq!(filed.len(), 1, "{report:?}");
        assert_eq!(served.fetched(&raw("e.ts")), 1);
        assert!(
            served
                .calls
                .borrow()
                .iter()
                .all(|(_, t)| *t == FETCH_TIMEOUT)
        );
    }
}

#[test]
fn a_sole_read_names_what_upstream_declares_now() {
    const W: Watch = Watch {
        id: "t.sole",
        ours: || vec!["agent"],
        docs: &[doc("t.rs")],
        read: Read::Sole {
            pattern: r#"fn name\(\) -> &str \{ "([^"]+)""#,
        },
        vanish: "no delegation",
        siblings: None,
    };
    let renamed = Served::default().with(
        &raw("t.rs"),
        "enum Events {}\nfn name() -> &str { \"task\" }",
    );
    let r = check(&renamed, &W);
    assert_eq!(r.breaking.len(), 1, "{r:?}");
    assert!(r.breaking[0].contains("declares `task` there now"), "{r:?}");
}

#[test]
fn the_floor_skips_a_degraded_parse_but_not_a_family_reader() {
    const SCHEMA: &str = r#"{"$defs": {"Events": {"oneOf": [{"const": "c"}]}}}"#;
    const DOC: Doc = Doc {
        at: At::Path {
            path: "s.json",
            branch: "main",
        },
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
        vanish: "x",
        siblings: None,
    };
    // One name back where we read two: the reader degraded, nothing is renamed.
    let r = check(&Served::default().with(&raw("s.json"), SCHEMA), &UNION);
    assert!(r.breaking.is_empty() && r.blind.len() == 1, "{r:?}");
    assert!(r.blind[0].contains("floor 2"), "{r:?}");

    // A family reader yields exactly the family, so its shrink is the finding.
    const ARMS: Watch = Watch {
        id: "t.arms",
        ours: || vec!["start", "wait"],
        docs: &[doc("a.rs")],
        read: Read::RustArms {
            name: "parse",
            arm: r"Ok\(Go\)",
        },
        vanish: "x",
        siblings: None,
    };
    let src = "enum Events {}\nfn parse() { match s { \"start\" => Ok(Go), _ => Err(()) } }";
    let r = check(&Served::default().with(&raw("a.rs"), src), &ARMS);
    assert!(r.blind.is_empty(), "{r:?}");
    assert!(
        r.breaking.len() == 1 && r.breaking[0].contains("`wait`"),
        "{r:?}"
    );
}

#[test]
fn a_sibling_in_the_family_is_reviewed_and_a_stale_ignore_is_reported() {
    const SCHEMA: &str = r#"{"$defs": {"Events": {"oneOf": [
        {"const": "tool_call"}, {"const": "tool_call_update"},
        {"const": "tool_call_cancelled"}, {"const": "tool_call_deferred"},
        {"const": "tool_call_`](https://x)\u202e"},
        {"const": "plan"}, {"const": "agent_message"}]}}}"#;
    const W: Watch = Watch {
        id: "t.sib",
        ours: || vec!["tool_call", "tool_call_update"],
        docs: &[Doc {
            at: At::Path {
                path: "s.json",
                branch: "main",
            },
            anchor: r#""Events""#,
            owns: "`Events`",
        }],
        read: Read::JsonUnion {
            def: "Events",
            prop: None,
        },
        vanish: "x",
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
    assert_eq!(r.review.len(), 3, "{r:?}");
    // A fetched name stays inside its code span, its controls dropped.
    assert!(
        r.review
            .iter()
            .any(|l| l.contains("`tool_call_](https://x)`") && !l.contains('\u{202e}')),
        "{r:?}"
    );
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

#[test]
fn a_release_member_is_read_only_once_its_sums_vouch_for_it() {
    const MEMBER: &str = "pkg/schema.json";
    const W: Watch = Watch {
        id: "t.asset",
        ours: || vec!["a.b"],
        docs: &[Doc {
            at: At::ReleaseAsset {
                asset: "cli-{version}-linux.tgz",
                member: MEMBER,
                sums: "SUMS.txt",
            },
            anchor: r#""Events""#,
            owns: "`Events`",
        }],
        read: Read::JsonKeys,
        vanish: "x",
        siblings: None,
    };
    let schema = r#"{"Events": 1, "properties": {"a.b": {}}}"#;
    let base = "https://github.com/o/r/releases/download/v1.0.0/";
    let tarball = format!("{base}cli-1.0.0-linux.tgz");
    let ok =
        Served::default().release("o/r", "cli-{version}-linux.tgz", MEMBER, "SUMS.txt", schema);
    let r = check(&ok, &W);
    assert!(
        r.breaking.is_empty() && r.blind.is_empty() && r.errors.is_empty(),
        "{r:?}"
    );
    let long: Vec<_> = ok
        .calls
        .borrow()
        .iter()
        .filter(|(_, t)| *t == RELEASE_ASSET_TIMEOUT)
        .map(|(u, _)| u.clone())
        .collect();
    assert_eq!(long, [tarball], "only the tarball waits long");

    let unvouched = Served::default()
        .release("o/r", "cli-{version}-linux.tgz", MEMBER, "SUMS.txt", schema)
        .with(&format!("{base}SUMS.txt"), "0000  cli-1.0.0-linux.tgz\n");
    let r = check(&unvouched, &W);
    assert!(
        r.blind.is_empty() && r.errors.len() == 1,
        "a sums miss is retried: {r:?}"
    );
    // The right digest vouches only for the name it is listed under.
    let digest = format!("{:x}", Sha256::digest(b"TARBALL o/r cli-1.0.0-linux.tgz"));
    let misnamed = Served::default()
        .release("o/r", "cli-{version}-linux.tgz", MEMBER, "SUMS.txt", schema)
        .with(
            &format!("{base}SUMS.txt"),
            &format!("{digest}  other.tgz\n"),
        );
    let r = check(&misnamed, &W);
    assert!(r.errors.len() == 1, "{r:?}");

    let mut moved =
        Served::default().release("o/r", "cli-{version}-linux.tgz", MEMBER, "SUMS.txt", schema);
    moved.tarballs.clear();
    let r = check(&moved, &W);
    assert!(
        r.errors.is_empty() && r.blind.len() == 1,
        "a missing member darkens the pin: {r:?}"
    );

    let tagless = Served::default()
        .release("o/r", "cli-{version}-linux.tgz", MEMBER, "SUMS.txt", schema)
        .with(&format!("{API_URL}/o/r/releases/latest"), "{}");
    let r = check(&tagless, &W);
    assert!(
        r.blind.is_empty() && r.errors.len() == 1,
        "a tagless reply is retried: {r:?}"
    );
}

/// A declared pattern as text it matches, `{name}` and its one capture group
/// standing for `name`, an alternation for its first branch. A construct it
/// cannot render fails the test, which keeps declared patterns to shapes a
/// reader can tell names apart in.
fn render(pattern: &str, name: &str) -> String {
    let mut out = String::new();
    let mut rest = pattern;
    while !rest.is_empty() {
        let (emit, used) = if let Some(alts) = rest.strip_prefix("(?:") {
            let end = alts
                .find(')')
                .unwrap_or_else(|| panic!("unclosed group in {pattern}"));
            let first = alts[..end].split('|').next().unwrap_or_default();
            (render(first, name), 3 + end + 1)
        } else {
            let fixed: &[(&str, &str)] = &[
                ("{name}", name),
                (r#"([^"]+)"#, name),
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
        (
            r#"impl\s+X\s*\{\s*fn\s+(?:a|b)\(\)\s*\{\s*"([^"]+)""#,
            "agent",
        ),
    ] {
        let text = render(pattern, name);
        let re = Regex::new(&pattern.replace("{name}", &regex::escape(name))).expect("compiles");
        assert!(re.is_match(&text), "{pattern} renders {text:?}");
    }
}

/// A page declaring `names` the way `w` reads them, its anchors included.
fn page(w: &Watch, names: &[&str]) -> String {
    let anchors: Vec<String> = w.docs.iter().map(|doc| render(doc.anchor, "")).collect();
    match w.read {
        Read::Each { pattern } | Read::Sole { pattern } => {
            let decls: Vec<String> = names.iter().map(|n| render(pattern, n)).collect();
            format!("{}\n{}\n", anchors.join("\n"), decls.join("\n"))
        }
        Read::RustArms { name, arm } => {
            let arms: String = names
                .iter()
                .map(|n| format!("\"{n}\" => {},\n", render(arm, "")))
                .collect();
            format!(
                "{}\nfn {name}() {{ match a {{\n{arms} _ => Err(()) }} }}\n",
                anchors.join("\n")
            )
        }
        // A JSON document's anchor renders as a key, so each one opens a member.
        Read::JsonUnion { def, prop } => {
            let members: Vec<serde_json::Value> = names
                .iter()
                .map(|n| match prop {
                    Some(p) => serde_json::json!({"properties": {p: {"const": n}}}),
                    None => serde_json::json!({"const": n}),
                })
                .collect();
            let defs = serde_json::json!({ def: {"oneOf": members} });
            let keys: String = anchors.iter().map(|a| format!("{a} 0, ")).collect();
            format!("{{{keys}\"$defs\": {defs}}}")
        }
        Read::JsonKeys => {
            let props: serde_json::Map<String, serde_json::Value> = names
                .iter()
                .map(|n| ((*n).to_owned(), serde_json::json!({})))
                .collect();
            let keys: String = anchors.iter().map(|a| format!("{a} 0, ")).collect();
            format!(
                "{{{keys}\"properties\": {}}}",
                serde_json::Value::Object(props)
            )
        }
    }
}

/// #941's gate, generic: every watch fires when one of its names vanishes
/// and stays silent while all are there. The vanished name leaves a decoy in
/// its own shape, so a pattern loose enough to match the decoy (a bare
/// substring where the declaration is quoted) cannot pass.
#[test]
fn every_watch_fires_on_a_vanish_and_stays_silent_otherwise() {
    let mut proven = 0;
    for d in REGISTRY {
        for up in d.upstream {
            for w in up.watches {
                let serve = |text: &str| {
                    w.docs
                        .iter()
                        .fold(Served::default(), |s, doc| match locate(up, &doc.at).0 {
                            Located::Raw { url } => s.with(&url, text),
                            Located::Release {
                                repo,
                                asset,
                                member,
                                sums,
                            } => s.release(repo, asset, member, sums, text),
                        })
                };
                let run_on = |text: &str| {
                    let served = serve(text);
                    let mut report = Report::default();
                    let mut fetched = Fetched::new(&served);
                    check_watch(d.name, up, w, &mut fetched, &mut report);
                    report
                };
                let ours = (w.ours)();
                let intact = run_on(&page(w, &ours));
                assert!(
                    intact.breaking.is_empty() && intact.blind.is_empty(),
                    "{}: an intact document stays silent: {intact:?}",
                    w.id
                );
                let victim = ours[0];
                let decoy = format!("{victim}_pxd");
                let mut kept: Vec<&str> = ours[1..].to_vec();
                kept.push(&decoy);
                let vanished = run_on(&page(w, &kept));
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
    assert!(proven > 0, "the registry declares watches");
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
        let compiled = match w.read {
            Read::Each { pattern } => Regex::new(&pattern.replace("{name}", "n")).map(drop),
            Read::Sole { pattern } => Regex::new(pattern).map(drop),
            Read::RustArms { name, arm } => read::rust_arms("", name, arm).map(drop),
            Read::JsonUnion { .. } | Read::JsonKeys => Ok(()),
        };
        compiled.unwrap_or_else(|e| panic!("{}: {e}", w.id));
    }
    assert!(watches > 0, "the registry declares watches");
}

#[test]
fn the_legacy_buckets_merge_and_an_unreadable_run_goes_dark_on_our_side() {
    let mut r = Report::default();
    let json = r#"{"breaking": ["B"], "review": [], "blind": ["D"], "errors": ["E"]}"#;
    fold_legacy(&mut r, "x.py", Ok(json.as_bytes().to_vec()));
    assert_eq!((r.breaking.len(), r.blind.len(), r.errors.len()), (1, 1, 1));
    // A key renamed on either side would otherwise drop its findings and exit 0.
    for bad in [
        Ok(br#"{"breaking": [], "review": [], "blind": []}"#.to_vec()),
        Ok(br#"{"broken": [], "breaking": [], "review": [], "blind": [], "errors": []}"#.to_vec()),
        Ok(b"Traceback (most recent call last):".to_vec()),
        Err("No such file or directory".to_owned()),
    ] {
        let mut r = Report::default();
        fold_legacy(&mut r, "x.py", bad);
        assert_eq!(r.blind.len(), 1, "{r:?}");
        assert!(r.blind[0].contains("the legacy watches") && r.blind[0].contains("Our own"));
        assert_eq!(r.exit_code(), 1);
    }
}

#[test]
fn a_fetched_name_cannot_leave_its_code_span() {
    assert_eq!(
        code_span("tool_call`](https://x)\nmore\u{202e}"),
        "tool_call](https://x)more"
    );
}

/// The heading is the instruction: a line filed under the wrong one sends a
/// reader to edit a decoder that works (#793).
#[test]
fn each_line_renders_under_its_own_heading() {
    let mut r = Report::default();
    r.add_breaking("VERIFIED-LINE".to_owned());
    r.add_blind("PROBE-LINE", "AT", "C", false);
    let out = r.render();
    let at = |s: &str| out.find(s).unwrap_or(usize::MAX);
    let order = [
        at("## ⛔ Verified upstream change"),
        at("VERIFIED-LINE"),
        at("## 🩺 Probe could NOT verify"),
        at("PROBE-LINE"),
    ];
    assert!(order.is_sorted() && order[3] < usize::MAX, "{out}");
    assert!(out.contains("do NOT change a decoder"), "{out}");

    let mut blind = Report::default();
    blind.add_blind("PROBE-ONLY", "AT", "C", false);
    let out = blind.render();
    assert!(out.contains("Probe could NOT verify") && !out.contains("Verified upstream change"));
}

/// upstream-drift.yml titles the issue with the report's H1 rather than its
/// own copy of these strings: a reworded copy would title a confirmed rename
/// "could not verify".
#[test]
fn the_workflow_titles_the_issue_from_the_report_h1() {
    let wf = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.github/workflows/upstream-drift.yml"
    ))
    .expect("the workflow is readable");
    assert!(wf.contains("head -1 drift-report.md") && wf.contains("s/^# //"));
    let mut r = Report::default();
    let mut titles = vec![r.title()];
    r.add_blind("W", "AT", "C", false);
    titles.push(r.title());
    r.add_review("R".to_owned());
    titles.push(r.title());
    r.add_breaking("B".to_owned());
    titles.push(r.title());
    for t in titles {
        assert!(!wf.contains(t), "the workflow keeps a copy of {t:?}");
    }
}

/// A family reader yields only the family, so everything it yields beyond ours
/// is a sibling.
#[test]
fn every_name_a_family_reader_yields_beyond_ours_is_a_sibling() {
    const W: Watch = Watch {
        id: "t.all",
        ours: || vec!["start"],
        docs: &[doc("a.rs")],
        read: Read::RustArms {
            name: "parse",
            arm: r"Ok\(Go\)",
        },
        vanish: "x",
        siblings: Some(Siblings {
            family: Family::All,
            ignored: &[],
        }),
    };
    let src =
        "enum Events {}\nfn parse() { match s { \"start\" | \"begin\" => Ok(Go), _ => Err(()) } }";
    let r = check(&Served::default().with(&raw("a.rs"), src), &W);
    assert!(r.breaking.is_empty() && r.blind.is_empty(), "{r:?}");
    assert!(
        r.review.len() == 1 && r.review[0].contains("`begin`"),
        "{r:?}"
    );
}
