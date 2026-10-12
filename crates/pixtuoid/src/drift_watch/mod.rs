//! The upstream drift watch: a name a source declares it reads
//! ([`pixtuoid_core::source::upstream`]) has not vanished upstream, and one it
//! waits on has not appeared. The
//! `drift_watch` example runs it for upstream-drift.yml, which files the report
//! as a tracking issue; a name ships renamed with no notice (`Task` → `Agent`
//! silently disabled subagent suppression), so this is how we hear first.

mod net;
mod read;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Duration;

use pixtuoid_core::source::registry::REGISTRY;
use pixtuoid_core::source::upstream::{At, Doc, Family, Finding, Read, Siblings, Watch, When};
use regex::Regex;
use sha2::{Digest, Sha256};

use crate::install::target::TARGETS;

use net::Platform;
use read::{Names, Unread};

/// One stalled host must not hold the whole sweep.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// Only a release tarball earns more.
const RELEASE_ASSET_TIMEOUT: Duration = Duration::from_secs(120);

const RAW_URL: &str = "https://raw.githubusercontent.com";
const API_URL: &str = "https://api.github.com/repos";

/// Why a fetch returned nothing. Neither is ever a vanish.
#[derive(Debug, PartialEq, Eq)]
enum FetchError {
    /// Our pin is gone, so the watch is dark for that document: `why` is the
    /// status upstream answered, or what its release lacks.
    Gone { why: String },
    /// May pass on a retry.
    Transient(String),
}

/// The network, behind a seam so the engine's tests never spawn `curl` or
/// `tar`.
trait Fetch {
    /// The body at `url`.
    ///
    /// # Errors
    ///
    /// [`FetchError::Gone`] on a status that means the pin is wrong, else
    /// [`FetchError::Transient`].
    fn get(&self, url: &str, timeout: Duration) -> Result<Vec<u8>, FetchError>;
    /// `member` out of a gzipped tarball; `None` when it has none.
    ///
    /// # Errors
    ///
    /// [`FetchError::Transient`] when the extractor cannot run or fails.
    fn untar(&self, tarball: &[u8], member: &str) -> Result<Option<Vec<u8>>, FetchError>;
}

/// One bucket per DISPOSITION, because each calls for a different action:
/// `breaking` fix the decoder or the registration, `review` adopt, ignore or re-verify,
/// `blind` repin by hand,
/// `errors` transient. Collapsing `blind` into `breaking` is what made #793
/// report five phantom renames.
#[derive(Debug, Default)]
pub struct Report {
    breaking: Vec<String>,
    review: Vec<String>,
    blind: Vec<String>,
    errors: Vec<String>,
}

impl Report {
    fn add_breaking(&mut self, line: String) {
        self.breaking.push(line);
    }

    fn add_review(&mut self, line: String) {
        self.review.push(line);
    }

    fn add_error(&mut self, line: String) {
        self.errors.push(line);
    }

    /// A lookup missed. Composed here so the disclaimer always ships with it;
    /// `ours` when the unreadable thing is ours, since the default wording
    /// blames upstream, which nothing verified.
    fn add_blind(&mut self, what: &str, at: &str, consequence: &str, ours: bool) {
        let (cause, action) = if ours {
            (
                "Our own declaration or checker is stale; nothing upstream was consulted.",
                "Fix ours — this says nothing about upstream.",
            )
        } else {
            (
                "Upstream may have moved or reshaped it, or our pin/parser may be stale; \
                 this is NOT evidence that upstream changed.",
                "Re-verify upstream by hand, then repin — do NOT change a decoder on this alone.",
            )
        };
        self.blind.push(format!(
            "could not verify {what} at {at} — {cause} {consequence} {action}"
        ));
    }

    /// The H1, which IS the issue title: upstream-drift.yml reads it back off
    /// the rendered report. Ordered by disposition, so the issue list carries
    /// the strongest finding.
    fn title(&self) -> &'static str {
        if !self.breaking.is_empty() {
            "Upstream CLI wire-format drift detected"
        } else if !self.review.is_empty() {
            "Upstream changes to review"
        } else if !self.blind.is_empty() {
            "Upstream drift watch could not verify — repin needed"
        } else {
            "Upstream wire-format watch: no drift"
        }
    }

    pub fn render(&self) -> String {
        let mut out = vec![format!("# {}", self.title()), String::new()];
        let mut section = |head: &str, intro: Option<&str>, lines: &[String]| {
            if lines.is_empty() {
                return;
            }
            out.push(head.to_owned());
            out.push(String::new());
            if let Some(intro) = intro {
                out.push(intro.to_owned());
                out.push(String::new());
            }
            out.extend(lines.iter().map(|l| format!("- {l}")));
            out.push(String::new());
        };
        section(
            "## ⛔ Verified upstream change — events will silently drop",
            Some(
                "Each line was read off the upstream surface that OWNS the name: the \
                 declaration is present and the name is gone. Fix what the line names: \
                 the decoder, or the hook we register.",
            ),
            &self.breaking,
        );
        section("## 🔎 Upstream changes to review", None, &self.review);
        section(
            "## 🩺 Probe could NOT verify — this is not evidence of upstream change",
            Some(
                "A lookup missed, so all the watch knows is that **its own probe failed**. \
                 A stale pin, a moved declaration, or a refactor that left a re-export \
                 facade at the old path all land here, and every check riding the \
                 affected surface was SKIPPED rather than reported as drift. **Verify \
                 upstream by hand and repin — do NOT change a decoder on these lines \
                 alone** (#793 did, against three working env vars).",
            ),
            &self.blind,
        );
        section(
            "## ⚠️ Could not verify (transient network/HTTP — not drift)",
            None,
            &self.errors,
        );
        if self.breaking.is_empty()
            && self.review.is_empty()
            && self.blind.is_empty()
            && self.errors.is_empty()
        {
            out.push(
                "✅ No drift. Nothing we read has vanished and nothing we wait on has appeared."
                    .to_owned(),
            );
        }
        out.join("\n")
    }

    /// 0 no findings · 1 actionable (the workflow opens an issue) · 2 transient
    /// only. `blind` is actionable: the watch is dark until someone repins, and
    /// a dark watch that exits 0 goes green (#454).
    pub fn exit_code(&self) -> u8 {
        if !(self.breaking.is_empty() && self.review.is_empty() && self.blind.is_empty()) {
            1
        } else if !self.errors.is_empty() {
            2
        } else {
            0
        }
    }
}

/// Where one declared document is read from.
enum Located {
    Raw {
        url: String,
    },
    Release {
        repo: &'static str,
        asset: &'static str,
        member: &'static str,
        sums: &'static str,
    },
}

/// `at`'s [`Located`], and the label the report names it by.
fn locate(at: &At) -> (Located, String) {
    match *at {
        At::Path { repo, branch, path } => (
            Located::Raw {
                url: format!("{RAW_URL}/{repo}/{branch}/{path}"),
            },
            format!("{repo}@{branch}:{path}"),
        ),
        At::ReleaseAsset {
            repo,
            asset,
            member,
            sums,
        } => (
            Located::Release {
                repo,
                asset,
                member,
                sums,
            },
            format!("{repo}'s latest release ({asset}: {member})"),
        ),
        At::Url(url) => (
            Located::Raw {
                url: url.to_owned(),
            },
            url.trim_start_matches("https://").to_owned(),
        ),
    }
}

/// One run's documents: each is upstream's answer for the whole run, fetched
/// once however many watches read it, and a missed anchor is filed once.
struct Fetched<'f> {
    fetch: &'f dyn Fetch,
    texts: HashMap<String, Option<String>>,
    unanchored: HashSet<(String, &'static str)>,
}

impl<'f> Fetched<'f> {
    fn new(fetch: &'f dyn Fetch) -> Self {
        Self {
            fetch,
            texts: HashMap::new(),
            unanchored: HashSet::new(),
        }
    }

    fn text(&mut self, located: &Located, label: &str, report: &mut Report) -> Option<String> {
        if let Some(t) = self.texts.get(label) {
            return t.clone();
        }
        let got = match located {
            Located::Raw { url } => self
                .fetch
                .get(url, FETCH_TIMEOUT)
                .map(|b| String::from_utf8_lossy(&b).into_owned()),
            Located::Release {
                repo,
                asset,
                member,
                sums,
            } => self.release_member(repo, asset, member, sums),
        };
        let text = match got {
            Ok(t) => Some(t),
            Err(FetchError::Gone { why }) => {
                report.add_blind(
                    "the document",
                    &format!("{label} ({why})"),
                    "Every watch reading it was SKIPPED.",
                    false,
                );
                None
            }
            Err(FetchError::Transient(e)) => {
                report.add_error(format!("{label}: {e}"));
                None
            }
        };
        self.texts.insert(label.to_owned(), text.clone());
        text
    }

    /// The release's own sums file must vouch for the tarball before any of
    /// it is read.
    fn release_member(
        &self,
        repo: &str,
        asset: &str,
        member: &str,
        sums: &str,
    ) -> Result<String, FetchError> {
        let release = self
            .fetch
            .get(&format!("{API_URL}/{repo}/releases/latest"), FETCH_TIMEOUT)?;
        let tag = serde_json::from_slice::<serde_json::Value>(&release)
            .ok()
            .and_then(|v| v.get("tag_name")?.as_str().map(str::to_owned))
            .ok_or_else(|| {
                FetchError::Transient(format!("{repo}'s latest release names no tag"))
            })?;
        let base = format!("https://github.com/{repo}/releases/download/{tag}/");
        let name = asset.replace("{version}", tag.trim_start_matches('v'));
        let tarball = self
            .fetch
            .get(&format!("{base}{name}"), RELEASE_ASSET_TIMEOUT)?;
        let listed =
            String::from_utf8_lossy(&self.fetch.get(&format!("{base}{sums}"), FETCH_TIMEOUT)?)
                .into_owned();
        let digest = format!("{:x}", Sha256::digest(&tarball));
        let vouched = listed.lines().any(|l| {
            let mut cols = l.split_whitespace();
            cols.next() == Some(digest.as_str()) && cols.next() == Some(name.as_str())
        });
        if !vouched {
            return Err(FetchError::Transient(format!(
                "{name} does not match {tag}'s {sums}"
            )));
        }
        let body = self
            .fetch
            .untar(&tarball, member)?
            .ok_or_else(|| FetchError::Gone {
                why: format!("no {member} in {name}"),
            })?;
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    /// The text, once the document proves it is the one that declares the
    /// names: a fetch landing on the wrong content skips every watch riding
    /// it as probe health.
    fn anchored(
        &mut self,
        located: &Located,
        label: &str,
        doc: &Doc,
        report: &mut Report,
    ) -> Option<String> {
        let text = self.text(located, label, report)?;
        let anchor = match Regex::new(doc.anchor) {
            Ok(re) => re,
            Err(e) => {
                report.add_blind(
                    &format!("the anchor of {label} ({e})"),
                    label,
                    "Every watch reading it was SKIPPED.",
                    true,
                );
                return None;
            }
        };
        if anchor.is_match(&text) {
            return Some(text);
        }
        if self.unanchored.insert((label.to_owned(), doc.anchor)) {
            report.add_blind(
                &format!("{label}: the document no longer contains {}", doc.owns),
                label,
                "It still fetches, so the probe landed on the wrong content — most likely \
                 a stale pin (a moved declaration, or a re-export facade left at the old \
                 path) rather than an upstream rename. Every watch riding this document \
                 was SKIPPED, NOT reported as drift.",
                false,
            );
        }
        None
    }
}

fn check_watch(source: &str, w: &Watch, fetched: &mut Fetched, report: &mut Report) {
    let located: Vec<_> = w
        .docs
        .iter()
        .map(|doc| {
            let (l, label) = locate(&doc.at);
            (l, label, doc)
        })
        .collect();
    // All or nothing: a name may live in any one of them, so a partial read
    // would report it gone. Each miss has already filed its own line.
    let mut texts = Vec::new();
    for (l, label, doc) in &located {
        texts.push(fetched.anchored(l, label, doc, report));
    }
    let Some(texts) = texts.into_iter().collect::<Option<Vec<String>>>() else {
        return;
    };
    let at = located
        .iter()
        .map(|(_, label, _)| label.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let ours: BTreeSet<&str> = (w.ours)().into_iter().collect();
    let read_from: Vec<(&str, &str)> = located
        .iter()
        .zip(&texts)
        .map(|((_, label, _), text)| (label.as_str(), text.as_str()))
        .collect();
    let Some(read::Declared {
        names: upstream,
        partial,
    }) = read_docs(w, source, &read_from, &ours, report)
    else {
        return;
    };
    let (says, file): (&str, fn(&mut Report, String)) = match w.finding {
        Finding::Breaking(says) => (says, Report::add_breaking),
        Finding::Review(says) => (says, Report::add_review),
    };
    match w.when {
        When::Vanishes => {
            let verb = match w.read {
                Read::RustStr { .. } => Some("declares"),
                Read::TsReturn { .. } => Some("returns"),
                _ => None,
            };
            let now = verb.map_or_else(String::new, |verb| {
                format!(
                    " Upstream {verb} `{}` there now.",
                    upstream
                        .iter()
                        .map(|n| code_span(n))
                        .collect::<Vec<_>>()
                        .join("`, `")
                )
            });
            for name in ours.iter().filter(|n| !upstream.contains(**n)) {
                // Out of the read's sight, so no evidence it vanished.
                if let Some(why) = &partial {
                    report.add_blind(
                        &format!("`{name}` ({source}: `{}`)", w.id),
                        &at,
                        &format!(
                            "The reader read a partial declaration ({why}), so it may be \
                             declared out of sight and `{}` did not judge it.",
                            w.id
                        ),
                        false,
                    );
                    continue;
                }
                file(
                    report,
                    format!(
                        "`{name}` ({source}: `{}`) is no longer declared in {at} — {says}.{now}",
                        w.id
                    ),
                );
            }
        }
        When::Appears => {
            for name in ours.iter().filter(|n| upstream.contains(**n)) {
                file(
                    report,
                    format!(
                        "{at} now declares `{name}` ({source}: `{}`) — {says}.",
                        w.id
                    ),
                );
            }
        }
    }
    if let Some(sib) = &w.siblings {
        check_siblings(w.id, sib, &ours, &upstream, &at, report);
    }
}

/// Whether fewer names than ours means the reader degraded rather than that
/// most of ours were renamed at once, #929's false ⛔: only for a vanish
/// watch whose reader yields upstream's whole declaration. `Each` yields just
/// the ones of ours it finds and a family reader just the family, so their
/// shrink is the finding, and a shrunk parse cannot invent an appearance.
fn floored(w: &Watch) -> bool {
    matches!(w.when, When::Vanishes)
        && !matches!(w.read, Read::Each { .. })
        && !w.read.confined_to_family()
}

/// What the floor counts of `names`: a `TsFields` read's `field?` and
/// `field:"lit"` are facts beside a field, not fields.
fn floor_count<'a>(read: &Read, names: impl Iterator<Item = &'a str>) -> usize {
    match read {
        Read::TsFields { .. } => names.filter(|n| read::is_bare(n)).count(),
        _ => names.count(),
    }
}

/// Upstream's names off every `(label, text)` document of `w`, and why the
/// read is partial; `None` once it has filed why it could not. Every
/// document must yield, as every one must fetch: a union read off the rest
/// would hide what changed in the one that did not.
fn read_docs(
    w: &Watch,
    source: &str,
    docs: &[(&str, &str)],
    ours: &BTreeSet<&str>,
    report: &mut Report,
) -> Option<read::Declared> {
    let mut upstream = Names::new();
    let mut partial = Vec::new();
    for (label, text) in docs {
        let read = match w.read {
            Read::Each { pattern } => read::each(text, pattern, ours).map(Into::into),
            Read::RustStr { item } => read::rust_str(text, item).map(Into::into),
            Read::RustArms { name, arm } => read::rust_arms(text, name, arm).map(Into::into),
            Read::RustEnum { name } => read::rust_enum(text, name).map(Into::into),
            Read::RustStruct { name } => read::rust_struct(text, name).map(Into::into),
            Read::TsFields { name } => read::ts_fields(text, name),
            Read::TsMembers { of } => read::ts_members(text, of),
            Read::TsReturn { function } => read::ts_return(text, function).map(Into::into),
            Read::Matches { pattern } => read::matches(text, pattern).map(Into::into),
            Read::MdTable { header, under } => read::md_table(text, header, under).map(Into::into),
            Read::MdHeadings { under } => read::md_headings(text, under).map(Into::into),
            Read::PySet { decl } => read::py_set(text, decl).map(Into::into),
            Read::RustMacroRows { name } => read::rust_macro_rows(text, name).map(Into::into),
            Read::TsConstArray { name } => read::ts_const_array(text, name).map(Into::into),
            Read::JsonUnion { def, prop } => read::json_union(text, def, prop).map(Into::into),
            Read::JsonKeys => read::json_keys(text).map(Into::into),
        };
        match read {
            Ok(read::Declared {
                names,
                partial: why,
            }) => {
                upstream.extend(names);
                partial.extend(why.map(|why| format!("{label}: {why}")));
            }
            Err(Unread::Ours(e)) => {
                report.add_blind(
                    &format!("`{}`'s pattern or item spec ({e})", w.id),
                    source,
                    &format!("`{}` was SKIPPED.", w.id),
                    true,
                );
                return None;
            }
            Err(Unread::Missing(what)) => {
                report.add_blind(
                    &format!("the names `{}` reads", w.id),
                    label,
                    &format!("The reader found {what}, so `{}` was SKIPPED.", w.id),
                    false,
                );
                return None;
            }
        }
    }
    let (got, want) = (
        floor_count(&w.read, upstream.iter().map(String::as_str)),
        floor_count(&w.read, ours.iter().copied()),
    );
    if floored(w) && got < want {
        let at: Vec<&str> = docs.iter().map(|(label, _)| *label).collect();
        report.add_blind(
            &format!("the names `{}` reads", w.id),
            &at.join(", "),
            &format!(
                "The reader returned {got} names (floor {want}), so `{}` was SKIPPED.",
                w.id
            ),
            false,
        );
        return None;
    }
    Some(read::Declared {
        names: upstream,
        partial: (!partial.is_empty()).then(|| partial.join("; ")),
    })
}

/// Run only once every document of the watch was read: a missed one would
/// read as upstream deleting each ignored row.
fn check_siblings(
    id: &str,
    sib: &Siblings,
    ours: &BTreeSet<&str>,
    upstream: &Names,
    at: &str,
    report: &mut Report,
) {
    let first_word = |n: &str| n.split('_').next().unwrap_or(n).to_owned();
    let last_word = |n: &str| n.rsplit('_').next().unwrap_or(n).to_owned();
    let words = |word: &dyn Fn(&str) -> String| -> BTreeSet<String> {
        ours.iter()
            .filter(|o| o.contains('_'))
            .map(|o| word(o))
            .collect()
    };
    let in_family: Box<dyn Fn(&str) -> bool> = match sib.family {
        Family::All => Box::new(|_| true),
        Family::Prefix(p) => Box::new(move |n: &str| n.starts_with(p)),
        Family::FirstWord => {
            let kin = words(&first_word);
            Box::new(move |n: &str| kin.contains(&first_word(n)))
        }
        Family::LastWord => {
            let kin = words(&last_word);
            Box::new(move |n: &str| kin.contains(&last_word(n)))
        }
    };
    let ignored: BTreeSet<&str> = sib.ignored.iter().map(|(name, _why)| *name).collect();
    for name in upstream.iter().map(String::as_str) {
        if in_family(name) && !ours.contains(name) && !ignored.contains(name) {
            report.add_review(format!(
                "{at} declares `{}` beside the names `{id}` reads, and nothing decodes it. \
                 Decode it, or add it to that watch's ignored siblings with the reason.",
                code_span(name)
            ));
        }
    }
    for name in ignored.iter().filter(|n| !upstream.contains(**n)) {
        report.add_review(format!(
            "`{id}` still ignores `{name}`, which {at} no longer declares — upstream removed \
             it. Delete the row: it documents a decision about a name that is gone, and \
             would silently exempt it if upstream reused it."
        ));
    }
}

/// An upstream name as the inside of a Markdown code span: the report becomes
/// an issue body and a terminal's output, so a backtick would end the span and
/// a control or bidi character would drive what the reader sees.
fn code_span(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '`' && !crate::is_terminal_unsafe(*c))
        .collect()
}

/// Every watch the registered sources and the install targets declare, over the
/// network.
pub fn run(report: &mut Report) {
    check_all(&Platform, report);
}

/// Every declared watch with the source it reads for: the decoders' off the
/// registry, then the installers' off their targets.
fn declared() -> impl Iterator<Item = (&'static str, &'static Watch)> {
    REGISTRY
        .iter()
        .flat_map(|d| d.upstream.watches().map(move |w| (d.name, w)))
        .chain(
            TARGETS
                .iter()
                .flat_map(|t| t.upstream.iter().map(move |w| (t.core_source, w))),
        )
}

fn check_all(fetch: &dyn Fetch, report: &mut Report) {
    let mut fetched = Fetched::new(fetch);
    for (source, w) in declared() {
        check_watch(source, w, &mut fetched, report);
    }
}

/// The watches not yet declared in Rust, still in `check_upstream_drift.py`:
/// its buckets join this report, so each watch has one checker and the run one
/// issue. Its traceback, if it has one, goes straight to our stderr.
pub fn run_legacy(report: &mut Report) {
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/check_upstream_drift.py"
    );
    let stdout = std::process::Command::new("python3")
        .arg(script)
        .stderr(std::process::Stdio::inherit())
        .output()
        .map(|out| out.stdout)
        .map_err(|e| e.to_string());
    fold_legacy(report, script, stdout);
}

/// Every bucket is required and no other key is allowed: a renamed one on
/// either side would otherwise drop its findings and exit 0.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Buckets {
    breaking: Vec<String>,
    review: Vec<String>,
    blind: Vec<String>,
    errors: Vec<String>,
}

/// A legacy run whose buckets cannot be read is every legacy watch gone dark,
/// so it files probe health against ours rather than reading as no drift.
fn fold_legacy(report: &mut Report, script: &str, stdout: Result<Vec<u8>, String>) {
    let read =
        stdout.and_then(|o| serde_json::from_slice::<Buckets>(&o).map_err(|e| e.to_string()));
    match read {
        Ok(b) => {
            report.breaking.extend(b.breaking);
            report.review.extend(b.review);
            report.blind.extend(b.blind);
            report.errors.extend(b.errors);
        }
        Err(e) => report.add_blind(
            &format!("the legacy watches ({e})"),
            script,
            "They were SKIPPED.",
            true,
        ),
    }
}

#[cfg(test)]
mod tests;
