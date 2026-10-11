//! The upstream drift watch: every name a source declares it reads
//! ([`pixtuoid_core::source::upstream`]) is still declared upstream. The
//! `drift_watch` example runs it for upstream-drift.yml, which files the report
//! as a tracking issue; a name ships renamed with no notice (`Task` → `Agent`
//! silently disabled subagent suppression), so this is how we hear first.

mod net;
mod read;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Duration;

use pixtuoid_core::source::registry::REGISTRY;
use pixtuoid_core::source::upstream::{At, Doc, Family, Read, Siblings, Upstream, Watch};
use regex::Regex;
use sha2::{Digest, Sha256};

use net::Platform;
use read::Names;

/// One stalled host must not hold the whole sweep.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// Only a release tarball earns more.
const RELEASE_ASSET_TIMEOUT: Duration = Duration::from_secs(120);

const RAW_URL: &str = "https://raw.githubusercontent.com";
const API_URL: &str = "https://api.github.com/repos";

/// Why a fetch returned nothing. Neither is ever a vanish.
#[derive(Debug, PartialEq, Eq)]
enum FetchError {
    /// Our pin is gone, so the watch is dark for that document.
    Gone { status: u16 },
    /// May pass on a retry.
    Transient(String),
}

/// The network, behind a seam so the tests never spawn `curl` or `tar`.
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
/// `breaking` fix the decoder, `review` adopt or ignore, `blind` repin by hand,
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
            "New upstream events to review"
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
            "## ⛔ Verified upstream change — decoder will silently drop events",
            Some(
                "Each line was read off the upstream surface that OWNS the name: the \
                 declaration is present and the name is gone. Fix the decoder.",
            ),
            &self.breaking,
        );
        section("## 🔎 New upstream events to review", None, &self.review);
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
            out.push("✅ No drift. Every name we depend on is present upstream.".to_owned());
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

/// Where one declared document is read from, and how the report names it.
fn locate(up: &Upstream, at: &At) -> (Located, String) {
    let repo = up.repo;
    match *at {
        At::Path { path, branch } => (
            Located::Raw {
                url: format!("{RAW_URL}/{repo}/{branch}/{path}"),
            },
            format!("{repo}@{branch}:{path}"),
        ),
        At::ReleaseAsset {
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
            Err(FetchError::Gone { status }) => {
                report.add_blind(
                    "the document",
                    &format!("{label} (HTTP {status})"),
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
            .ok_or(FetchError::Gone { status: 404 })?;
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

fn check_watch(source: &str, up: &Upstream, w: &Watch, fetched: &mut Fetched, report: &mut Report) {
    let located: Vec<_> = w
        .docs
        .iter()
        .map(|doc| {
            let (l, label) = locate(up, &doc.at);
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
    let read_from: Vec<(&str, &str, &str)> = located
        .iter()
        .zip(&texts)
        .map(|((_, label, doc), text)| {
            let path = match doc.at {
                At::Path { path, .. } => path,
                At::ReleaseAsset { member, .. } => member,
            };
            (label.as_str(), path, text.as_str())
        })
        .collect();
    let upstream: Option<Names> = match w.read {
        Read::Each { pattern } => {
            let joined = texts.join("\n");
            let mut present = Names::new();
            for name in &ours {
                match Regex::new(&pattern.replace("{name}", &regex::escape(name))) {
                    Ok(re) if re.is_match(&joined) => {
                        present.insert((*name).to_owned());
                    }
                    Ok(_) => {}
                    Err(e) => {
                        our_pattern_is_broken(w, source, &e, report);
                        return;
                    }
                }
            }
            Some(present)
        }
        Read::Sole { pattern } => {
            read_docs(w, source, &read_from, ours.len(), report, |t, path| {
                read::sole(t, path, pattern)
            })
        }
        Read::RustArms { name, arm } => {
            read_docs(w, source, &read_from, ours.len(), report, |t, _| {
                read::rust_arms(t, name, arm)
            })
        }
        Read::JsonUnion { def, prop } => {
            read_docs(w, source, &read_from, ours.len(), report, |t, _| {
                Ok(read::json_union(t, def, prop))
            })
        }
        Read::JsonKeys => read_docs(w, source, &read_from, ours.len(), report, |t, _| {
            Ok(read::json_keys(t))
        }),
    };
    let Some(upstream) = upstream else {
        return;
    };
    let says = w.vanish;
    let now = match w.read {
        Read::Sole { .. } => format!(
            " Upstream declares `{}` there now.",
            upstream
                .iter()
                .map(|n| code_span(n))
                .collect::<Vec<_>>()
                .join("`, `")
        ),
        _ => String::new(),
    };
    for name in ours.iter().filter(|n| !upstream.contains(**n)) {
        report.add_breaking(format!(
            "`{name}` ({source}: `{}`) is no longer declared in {at} — {says}.{now}",
            w.id
        ));
    }
    if let Some(sib) = &w.siblings {
        check_siblings(w.id, sib, &ours, &upstream, &at, report);
    }
}

/// Readers that yield only the family we read: a shrink there is the finding
/// itself, so the floor that catches a degraded parse does not apply.
fn yields_only_the_family(read: &Read) -> bool {
    matches!(read, Read::RustArms { .. })
}

fn our_pattern_is_broken(w: &Watch, source: &str, e: &regex::Error, report: &mut Report) {
    report.add_blind(
        &format!("`{}`'s pattern ({e})", w.id),
        source,
        &format!("`{}` was SKIPPED.", w.id),
        true,
    );
}

/// Upstream's names off every `(label, path, text)` document of `w`, each
/// through `reader`; `None` once it has filed why it could not. Every
/// document must yield, as every one must fetch: a union read off the rest
/// would hide what changed in the one that did not.
fn read_docs(
    w: &Watch,
    source: &str,
    docs: &[(&str, &str, &str)],
    ours: usize,
    report: &mut Report,
    reader: impl Fn(&str, &str) -> Result<Option<Names>, regex::Error>,
) -> Option<Names> {
    let mut upstream = Names::new();
    for (label, path, text) in docs {
        match reader(text, path) {
            Ok(Some(names)) => upstream.extend(names),
            Ok(None) => {
                report.add_blind(
                    &format!("the names `{}` reads", w.id),
                    label,
                    &format!(
                        "The reader found no declaration, so `{}` was SKIPPED.",
                        w.id
                    ),
                    false,
                );
                return None;
            }
            Err(e) => {
                our_pattern_is_broken(w, source, &e, report);
                return None;
            }
        }
    }
    // Fewer names than we read means the reader degraded, not that most of
    // ours were renamed at once: #929's false ⛔ was that.
    if !yields_only_the_family(&w.read) && upstream.len() < ours {
        let at: Vec<&str> = docs.iter().map(|(label, _, _)| *label).collect();
        report.add_blind(
            &format!("the names `{}` reads", w.id),
            &at.join(", "),
            &format!(
                "The reader returned {} names (floor {ours}), so `{}` was SKIPPED.",
                upstream.len(),
                w.id
            ),
            false,
        );
        return None;
    }
    Some(upstream)
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
    let ignored: BTreeSet<&str> = sib.ignored.iter().map(|(name, _why)| *name).collect();
    for name in upstream.iter().map(String::as_str) {
        let in_family = match sib.family {
            Family::All => true,
            Family::Prefix(p) => name.starts_with(p),
        };
        if in_family && !ours.contains(name) && !ignored.contains(name) {
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

/// Every watch every registered source declares, over the network.
pub fn run(report: &mut Report) {
    check_all(&Platform, report);
}

fn check_all(fetch: &dyn Fetch, report: &mut Report) {
    let mut fetched = Fetched::new(fetch);
    for d in REGISTRY {
        for up in d.upstream {
            for w in up.watches {
                check_watch(d.name, up, w, &mut fetched, report);
            }
        }
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
