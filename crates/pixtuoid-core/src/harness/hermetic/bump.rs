//! A pin bump as the nightly proposes it: which moved releases become bot PRs,
//! and what the tree a bump leg's patch leaves may hold before publish applies
//! it. The patch is untrusted, since the leg ran a just-published CLI: it may
//! carry data a decoder reads, never code a same-repo PR's CI would run. The
//! threat is a poisoned release, not one crafted against these checks (the
//! owner's call): a bypass that needs input built for this pipeline is out of
//! scope.

use std::path::{Component, Path, PathBuf};

use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{PROFILES, Profile, pin};
use crate::harness::sources_root;

/// The nightly's drift legs, as `hermetic.yml`'s matrix names them: a release
/// is proposed only once it moved on every one.
const OSES: &[&str] = &["macos", "windows"];

/// The file beside a run's reports naming each source that moved.
pub const MOVED: &str = "moved.json";

/// The `hermetic.yml` output a drift leg on `os` reports its [`MOVED`] rows as.
fn moved_output(os: &str) -> String {
    format!("moved_{os}")
}

/// The file a bump leg writes its patch for `source` to, and check reads it from.
pub fn patch_name(source: &str) -> String {
    format!("{source}.patch")
}

/// A source whose `latest` passed every scenario, every run reporting it, above
/// its pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Moved {
    /// The registered source id.
    pub source: String,
    /// The release it moved to.
    pub version: String,
}

/// Every bump PR's title opens with it, which is how plan finds the releases
/// taken.
pub(crate) const TITLE_PREFIX: &str = "chore(hermetic): pin";

/// A bot PR the nightly opens, or one a patch was admitted for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    /// The registered source id.
    pub source: String,
    /// The release its pin moves to.
    pub version: String,
    /// One per source, so a newer release updates the open PR.
    pub(crate) branch: String,
    /// Also the key a declined release is remembered by.
    pub(crate) title: String,
}

impl Proposal {
    /// The proposal moving `source` to `version`.
    pub(crate) fn new(source: &str, version: &Version) -> Self {
        Self {
            source: source.to_string(),
            version: version.to_string(),
            branch: format!("bot/bump-{source}"),
            title: format!("{TITLE_PREFIX} {source} at {version}"),
        }
    }
}

/// The releases to propose, given the drift job's outputs: each profiled source
/// that moved to one release on every OS in `OSES`, above its pin, under a
/// title no PR has taken (an open one proposes it, a closed one declined it). A
/// leg that reported nothing moved nothing.
///
/// # Errors
///
/// When a leg's rows are not rows, or the one release every leg moved a source
/// to is not a release or is below its pin.
pub fn plan(drift: &Map<String, Value>, taken: &[String]) -> Result<Vec<Proposal>, String> {
    let mut moved = Vec::new();
    for os in OSES {
        let rows: Vec<Moved> = match drift.get(&moved_output(os)).and_then(Value::as_str) {
            None | Some("") => Vec::new(),
            Some(json) => serde_json::from_str(json).map_err(|e| format!("{os}: {e}"))?,
        };
        moved.push(rows);
    }
    let mut proposals = Vec::new();
    for p in PROFILES {
        let versions: Vec<Option<&str>> = moved
            .iter()
            .map(|rows| {
                rows.iter()
                    .find(|r| r.source == p.source)
                    .map(|r| r.version.as_str())
            })
            .collect();
        let Some(&Some(first)) = versions.first() else {
            continue;
        };
        if versions.iter().any(|v| *v != Some(first)) {
            continue;
        }
        let version = Version::parse(first).map_err(|e| format!("{}: {first:?}: {e}", p.source))?;
        if !pin::moves(p.descriptor(), &version)? {
            continue;
        }
        let proposal = Proposal::new(p.source, &version);
        if !taken.contains(&proposal.title) {
            proposals.push(proposal);
        }
    }
    Ok(proposals)
}

/// The repository root, which a patch's paths are relative to.
pub fn repo_root() -> PathBuf {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    crate_dir
        .ancestors()
        .nth(2)
        .unwrap_or(crate_dir)
        .to_path_buf()
}

fn relative(abs: &Path) -> PathBuf {
    abs.strip_prefix(repo_root()).unwrap_or(abs).to_path_buf()
}

fn registry_relative() -> PathBuf {
    relative(&pin::registry_path())
}

/// Whether a bump of `p` may change `path`, relative to the repository: the
/// registry, a file under one of its scenarios' fixtures, or one of their
/// conformance snapshots, named as insta derives it (`conformance.rs`).
pub fn admits(p: &Profile, path: &Path) -> bool {
    if !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return false;
    }
    let sources = relative(&sources_root());
    path == registry_relative()
        || p.scenarios.iter().any(|s| {
            let fixture = sources.join(s.fixture);
            let snapshot = fixture.file_name().map(|scenario| {
                sources.join("snapshots").join(format!(
                    "sources__conformance__{}__{}.snap",
                    p.source,
                    scenario.to_string_lossy()
                ))
            });
            path.starts_with(&fixture) || snapshot.is_some_and(|s| path == s)
        })
}

/// One path `git diff HEAD --raw --no-renames -z` reports changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Relative to the repository.
    pub path: PathBuf,
    old_mode: String,
    new_mode: String,
    status: String,
}

/// The changes in git's `--raw -z` output.
///
/// # Errors
///
/// When a record is not in git's shape.
pub fn changes(raw: &[u8]) -> Result<Vec<Change>, String> {
    let text = std::str::from_utf8(raw).map_err(|e| e.to_string())?;
    let mut fields = text.split('\0').filter(|f| !f.is_empty());
    let mut out = Vec::new();
    while let Some(record) = fields.next() {
        let path = fields
            .next()
            .ok_or_else(|| format!("{record:?} names no path"))?;
        let meta: Vec<&str> = record.trim_start_matches(':').split(' ').collect();
        let [old_mode, new_mode, _, _, status] = meta[..] else {
            return Err(format!("{record:?} is not a raw record"));
        };
        out.push(Change {
            path: PathBuf::from(path),
            old_mode: old_mode.to_string(),
            new_mode: new_mode.to_string(),
            status: status.to_string(),
        });
    }
    Ok(out)
}

/// A regular, non-executable file's mode; a path absent on one side has none.
const REGULAR: &str = "100644";
const ABSENT: &str = "000000";

/// The paths a bump of `p` to `version` changed, once the tree a patch left
/// holds nothing else: each change [`admits`], each a regular file created,
/// edited or deleted, and the registry `after` exactly as
/// [`pin::repinned`] writes `before`. Git, never a parser of the patch, says
/// what the patch did.
///
/// # Errors
///
/// What the tree holds that a bump doesn't.
pub fn judge(
    p: &Profile,
    version: &Version,
    before: &str,
    after: &str,
    changes: &[Change],
) -> Result<Vec<PathBuf>, String> {
    let Some(expected) = pin::repinned(before, p.descriptor(), version)? else {
        return Err(format!("{}: {version} is its pin already", p.source));
    };
    if after != expected {
        return Err(format!(
            "the registry is not {}'s pin moving to {version}, and nothing else",
            p.source
        ));
    }
    let regular = |mode: &str| mode == REGULAR || mode == ABSENT;
    for c in changes {
        if !admits(p, &c.path) {
            return Err(format!("{:?} is outside {}'s bump", c.path, p.source));
        }
        // publish hands the paths to create-pull-request comma- or line-joined.
        if c.path.to_string_lossy().contains([',', '\n', '\r']) {
            return Err(format!("{:?} would split in publish's add-paths", c.path));
        }
        if !["M", "A", "D"].contains(&c.status.as_str())
            || !regular(&c.old_mode)
            || !regular(&c.new_mode)
        {
            return Err(format!(
                "{:?} is {} from mode {} to {}: a bump creates, edits or deletes regular files",
                c.path, c.status, c.old_mode, c.new_mode
            ));
        }
    }
    Ok(changes.iter().map(|c| c.path.clone()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::hermetic::profile;

    fn v(s: &str) -> Version {
        Version::parse(s).expect("a test version")
    }

    fn codex() -> &'static Profile {
        profile("codex").expect("codex's profile")
    }

    fn registry() -> String {
        std::fs::read_to_string(pin::registry_path()).expect("registry.rs")
    }

    fn above(p: &Profile) -> Version {
        let pin = pin::pinned(p.descriptor()).expect("a pin");
        Version::new(pin.major, pin.minor, pin.patch + 1)
    }

    /// The drift job's outputs, each leg's rows as `hermetic.yml` reports them.
    fn drift(legs: &[(&str, &[(&str, &str)])]) -> Map<String, Value> {
        let mut out = Map::new();
        out.insert("runs-stem".into(), Value::from("hermetic-runs-1-1-"));
        for (os, rows) in legs {
            let rows: Vec<Moved> = rows
                .iter()
                .map(|(source, version)| Moved {
                    source: (*source).to_string(),
                    version: (*version).to_string(),
                })
                .collect();
            let json = serde_json::to_string(&rows).expect("rows");
            out.insert(moved_output(os), Value::from(json));
        }
        out
    }

    #[test]
    fn a_release_moved_on_every_os_is_proposed_once() {
        let up = above(codex()).to_string();
        let both = drift(&[("macos", &[("codex", &up)]), ("windows", &[("codex", &up)])]);
        let proposals = plan(&both, &[]).expect("a plan");
        assert_eq!(proposals, [Proposal::new("codex", &v(&up))]);
        let noisy = drift(&[
            ("macos", &[("nope", &up), ("codex", &up), ("codex", &up)]),
            ("windows", &[("codex", &up)]),
        ]);
        assert_eq!(
            plan(&noisy, &[]).as_ref(),
            Ok(&proposals),
            "rows no profile reads"
        );
        let taken = [proposals[0].title.clone()];
        assert_eq!(plan(&both, &taken), Ok(vec![]), "proposed or declined");
    }

    #[test]
    fn a_release_moved_on_one_os_or_at_two_versions_is_not() {
        let up = above(codex());
        let next = Version::new(up.major, up.minor, up.patch + 1).to_string();
        let up = up.to_string();
        let mut silent = drift(&[("macos", &[("codex", &up)]), ("windows", &[])]);
        silent.insert(moved_output("windows"), Value::from(""));
        for legs in [
            drift(&[("macos", &[("codex", &up)]), ("windows", &[])]),
            drift(&[("macos", &[("codex", &up)])]),
            silent,
            drift(&[
                ("macos", &[("codex", &up)]),
                ("windows", &[("codex", &next)]),
            ]),
        ] {
            assert_eq!(plan(&legs, &[]), Ok(vec![]), "{legs:?}");
        }
        let pin = codex().descriptor().verified_version;
        let at_pin = drift(&[("macos", &[("codex", pin)]), ("windows", &[("codex", pin)])]);
        assert_eq!(plan(&at_pin, &[]), Ok(vec![]));
    }

    #[test]
    fn a_plan_refuses_rows_that_are_not_releases_above_the_pin() {
        let pin = pin::pinned(codex().descriptor()).expect("a pin");
        let below = Version {
            pre: semver::Prerelease::new("rc.1").expect("a prerelease"),
            ..pin
        }
        .to_string();
        let mut garbled = drift(&[("macos", &[]), ("windows", &[])]);
        garbled.insert(
            moved_output("macos"),
            Value::from("[{\"source\":\"codex\"}]"),
        );
        for legs in [
            garbled,
            drift(&[
                ("macos", &[("codex", "1.x")]),
                ("windows", &[("codex", "1.x")]),
            ]),
            drift(&[
                ("macos", &[("codex", &below)]),
                ("windows", &[("codex", &below)]),
            ]),
        ] {
            assert!(plan(&legs, &[]).is_err(), "{legs:?}");
        }
    }

    #[test]
    fn hermetic_yml_reports_moved_rows_from_every_os_plan_reads() {
        let yml = std::fs::read_to_string(repo_root().join(".github/workflows/hermetic.yml"))
            .expect("hermetic.yml");
        assert!(
            yml.contains(&format!("runs/{MOVED}")),
            "the file the tier writes"
        );
        assert_eq!(
            yml.matches("- { os: ").count(),
            OSES.len(),
            "one leg per OS"
        );
        for os in OSES {
            assert!(yml.contains(&format!("- {{ os: {os}, ")), "{os}'s leg");
            assert_eq!(
                yml.matches(&format!("{}:", moved_output(os))).count(),
                2,
                "{os}'s rows, as the job's output and the workflow's"
            );
        }
    }

    #[test]
    fn a_bump_admits_its_registry_fixtures_and_snapshots_alone() {
        let p = codex();
        let sources = relative(&sources_root());
        let fixture = sources.join(p.scenarios[0].fixture);
        let name = fixture.file_name().expect("a name").to_string_lossy();
        assert!(registry_relative().is_relative());
        assert_eq!(repo_root().join(registry_relative()), pin::registry_path());
        assert!(admits(p, &registry_relative()));
        assert!(admits(p, &fixture.join("provenance.json")));
        let snap = format!("sources__conformance__codex__{name}.snap");
        assert!(admits(p, &sources.join("snapshots").join(&snap)));
        let other = profile("claude-code").expect("claude-code's profile");
        assert!(!admits(
            p,
            &sources
                .join(other.scenarios[0].fixture)
                .join("provenance.json")
        ));
        let theirs = format!("sources__conformance__claude-code__{name}.snap");
        assert!(!admits(p, &sources.join("snapshots").join(theirs)));
        assert!(!admits(p, &fixture.join("..").join("..").join("build.rs")));
        assert!(!admits(p, Path::new("justfile")));
        assert!(!admits(
            p,
            &relative(&sources_root().join("conformance.rs"))
        ));
    }

    #[test]
    fn every_profile_admits_a_conformance_snapshot_insta_wrote() {
        let root = repo_root();
        let snapshots: Vec<PathBuf> = std::fs::read_dir(sources_root().join("snapshots"))
            .expect("the snapshots")
            .map(|e| relative(&e.expect("an entry").path()))
            .collect();
        for p in PROFILES {
            assert!(
                snapshots
                    .iter()
                    .any(|s| admits(p, s) && root.join(s).is_file()),
                "{}: no committed snapshot carries the name `admits` derives",
                p.source
            );
        }
    }

    #[test]
    fn the_nightly_finds_taken_releases_by_the_title_bumps_open_with() {
        let yml =
            std::fs::read_to_string(repo_root().join(".github/workflows/hermetic-nightly.yml"))
                .expect("hermetic-nightly.yml");
        assert!(yml.contains(&format!("in:title \"{TITLE_PREFIX}\"")));
        for at in ["$SOURCE", "${{ matrix.source }}"] {
            assert!(yml.contains(&patch_name(at)), "the patch {at} names");
        }
        assert!(
            Proposal::new("codex", &v("1.0.0"))
                .title
                .starts_with(TITLE_PREFIX)
        );
    }

    /// One change as git's `--raw -z` writes it.
    fn raw(old: &str, new: &str, status: &str, path: &Path) -> String {
        format!(
            ":{old} {new} 0000000 0000000 {status}\0{}\0",
            path.display()
        )
    }

    fn fixture_path(name: &str) -> PathBuf {
        relative(&sources_root().join(codex().scenarios[0].fixture).join(name))
    }

    #[test]
    fn changes_are_read_from_gits_raw_records() {
        let two = raw(REGULAR, REGULAR, "M", Path::new("a b/c.jsonl"))
            + &raw(ABSENT, REGULAR, "A", Path::new("d"));
        let read = changes(two.as_bytes()).expect("changes");
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].path, PathBuf::from("a b/c.jsonl"));
        assert_eq!(read[1].status, "A");
        assert!(changes(b":100644 M\0x\0").is_err(), "not git's shape");
        assert!(changes(b":100644 100644 0 0 M\0").is_err(), "no path");
    }

    #[test]
    fn a_bump_is_judged_by_the_tree_it_left() {
        let before = registry();
        let up = above(codex());
        let after = pin::repinned(&before, codex().descriptor(), &up)
            .expect("a repin")
            .expect("a move");
        let payloads = fixture_path("hook-payloads.jsonl");
        let edits = [
            raw(REGULAR, REGULAR, "M", &registry_relative()),
            raw(REGULAR, REGULAR, "M", &payloads),
            raw(ABSENT, REGULAR, "A", &fixture_path("new.jsonl")),
            raw(REGULAR, ABSENT, "D", &fixture_path("old.jsonl")),
        ]
        .concat();
        let ok = changes(edits.as_bytes()).expect("changes");
        assert_eq!(
            judge(codex(), &up, &before, &after, &ok).map(|p| p.len()),
            Ok(4)
        );
        let next = Version::new(up.major, up.minor, up.patch + 1);
        assert!(
            judge(codex(), &next, &before, &after, &ok).is_err(),
            "another release"
        );
        let pin = pin::pinned(codex().descriptor()).expect("a pin");
        assert!(
            judge(codex(), &pin, &before, &before, &ok).is_err(),
            "no move"
        );
        let injected = format!("{after}pub fn injected() {{}}\n");
        assert!(
            judge(codex(), &up, &before, &injected, &ok).is_err(),
            "the pin, and code"
        );
        for bad in [
            raw(REGULAR, REGULAR, "M", Path::new("justfile")),
            raw(REGULAR, "100755", "M", &payloads),
            raw("100755", REGULAR, "M", &payloads),
            raw(ABSENT, "120000", "A", &fixture_path("link.jsonl")),
            raw(REGULAR, "120000", "T", &payloads),
            raw(ABSENT, "160000", "A", &fixture_path("module")),
            raw(ABSENT, REGULAR, "A", &fixture_path("x,.")),
            raw(ABSENT, REGULAR, "A", &fixture_path("x\n.")),
        ] {
            let read = changes(format!("{edits}{bad}").as_bytes()).expect("changes");
            assert!(
                judge(codex(), &up, &before, &after, &read).is_err(),
                "{bad:?}"
            );
        }
    }

    fn git(dir: &Path, args: &[&str]) -> std::process::Output {
        std::process::Command::new("git")
            .current_dir(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "core.autocrlf=false",
            ])
            .args(args)
            .output()
            .expect("git")
    }

    #[test]
    fn a_patch_smuggling_code_into_the_registry_past_a_fixture_is_refused() {
        // A plain `---`/`+++` section after a `diff --git` hunk: `git apply`
        // applies it to its own file; a parser counting to the next
        // `diff --git` read it as the fixture's lines.
        let dir =
            std::env::temp_dir().join(format!("pixtuoid-bump-smuggle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let before = registry();
        let reg = registry_relative();
        let payloads = fixture_path("hook-payloads.jsonl");
        for (path, text) in [(&reg, before.as_str()), (&payloads, "a\n")] {
            std::fs::create_dir_all(dir.join(path).parent().expect("a parent")).expect("dirs");
            std::fs::write(dir.join(path), text).expect("write");
        }
        assert!(git(&dir, &["init", "-q"]).status.success());
        assert!(git(&dir, &["add", "."]).status.success());
        assert!(git(&dir, &["commit", "-qm", "base"]).status.success());
        let up = above(codex());
        let after = pin::repinned(&before, codex().descriptor(), &up)
            .expect("a repin")
            .expect("a move");
        let lines: Vec<&str> = before.lines().collect();
        let at = lines
            .iter()
            .zip(after.lines())
            .position(|(a, b)| *a != b)
            .expect("the pin line");
        let (prev, was, next) = (lines[at - 1], lines[at], lines[at + 1]);
        let now = after.lines().nth(at).expect("the moved pin");
        let last = lines.len();
        let tail = before.lines().last().expect("a last line");
        // A patch names paths with `/` on every OS.
        let slashed = |path: &Path| {
            path.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        };
        let (reg, payloads) = (slashed(&reg), slashed(&payloads));
        let smuggle = |code: &str| {
            let appended = if code.is_empty() {
                String::new()
            } else {
                format!("@@ -{last} +{last},2 @@\n {tail}\n+{code}\n")
            };
            format!(
                "diff --git a/{payloads} b/{payloads}\n--- a/{payloads}\n+++ b/{payloads}\n\
                 @@ -1 +1 @@\n-a\n+b\n\
                 --- a/{reg}\n+++ b/{reg}\n@@ -{at},3 +{at},3 @@\n {prev}\n-{was}\n+{now}\n {next}\n\
                 {appended}"
            )
        };
        let judged = |code: &str| {
            let file = dir.join("bump.patch");
            std::fs::write(&file, smuggle(code)).expect("write");
            let applied = git(&dir, &["apply", "bump.patch"]);
            assert!(
                applied.status.success(),
                "{}",
                String::from_utf8_lossy(&applied.stderr)
            );
            std::fs::remove_file(&file).expect("remove");
            let raw = git(&dir, &["diff", "HEAD", "--raw", "--no-renames", "-z"]).stdout;
            let after = std::fs::read_to_string(dir.join(&reg)).expect("read");
            assert!(git(&dir, &["checkout", "-q", "--", "."]).status.success());
            judge(
                codex(),
                &up,
                &before,
                &after,
                &changes(&raw).expect("changes"),
            )
        };
        assert!(
            judged("pub fn injected() {}").is_err(),
            "code beside the pin"
        );
        assert_eq!(
            judged("").map(|p| p.len()),
            Ok(2),
            "the same shape, a bump alone"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
