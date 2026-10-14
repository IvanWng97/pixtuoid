//! A pin bump as the nightly proposes it: which moved releases become bot PRs,
//! and what a bump leg's patch may hold before publish applies it. The patch is
//! untrusted, since the leg ran a just-published CLI: it may carry data a
//! decoder reads, never code a same-repo PR's CI would run.

use std::path::{Component, Path, PathBuf};

use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{PROFILES, PROVENANCE_AIMOCK, Profile, pin, profile};
use crate::harness::{PROVENANCE, reads, sources_root};

/// The nightly's drift legs, as `runner.os` names them: a release is proposed
/// only once it moved on every one.
const OSES: &[&str] = &["macOS", "Windows"];

/// The file beside a run's reports naming each source that moved.
pub const MOVED: &str = "moved.json";

/// A source whose `latest` passed every scenario, every run reporting it, above
/// its pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Moved {
    /// The registered source id.
    pub source: String,
    /// The release it moved to.
    pub version: String,
}

/// A bot PR the nightly opens, or one a patch was admitted for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    /// The registered source id.
    pub source: String,
    /// The release its pin moves to.
    pub version: String,
    /// One per source, so a newer release updates the open PR.
    pub branch: String,
    /// Also the key a declined release is remembered by.
    pub title: String,
}

impl Proposal {
    /// The proposal moving `source` to `version`.
    pub fn new(source: &str, version: &Version) -> Self {
        Self {
            source: source.to_string(),
            version: version.to_string(),
            branch: format!("bot/bump-{source}"),
            title: format!("chore(hermetic): pin {source} at {version}"),
        }
    }
}

/// The releases to propose: each profiled source that moved to one release on
/// every OS in [`OSES`], above its pin, under a title no PR has taken (an open
/// one proposes it, a closed one declined it).
///
/// # Errors
///
/// When `moved` is not one row list per OS in [`OSES`], names an unprofiled
/// source or one source twice, or holds a release that is not one or is below
/// its pin.
pub fn plan(moved: &[(String, Vec<Moved>)], taken: &[String]) -> Result<Vec<Proposal>, String> {
    let mut oses: Vec<&str> = moved.iter().map(|(os, _)| os.as_str()).collect();
    oses.sort_unstable();
    let mut expected = OSES.to_vec();
    expected.sort_unstable();
    if oses != expected {
        return Err(format!("moved rows for {oses:?}, not {expected:?}"));
    }
    for (os, rows) in moved {
        for (i, row) in rows.iter().enumerate() {
            if profile(&row.source).is_none() {
                return Err(format!("{os}: {:?} has no profile", row.source));
            }
            if rows[..i].iter().any(|r| r.source == row.source) {
                return Err(format!("{os}: {:?} moved twice", row.source));
            }
        }
    }
    let mut proposals = Vec::new();
    for p in PROFILES {
        let versions: Vec<Option<&str>> = moved
            .iter()
            .map(|(_, rows)| {
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

/// One file's part of a patch: its path, and the lines its hunks remove and add.
#[derive(Debug)]
struct FileDiff<'a> {
    path: PathBuf,
    removed: Vec<&'a str>,
    added: Vec<&'a str>,
    created: bool,
    deleted: bool,
}

const DEV_NULL: &str = "/dev/null";

/// The files of a `git diff --no-renames` patch, admitting only an edit, a
/// creation or a deletion of a regular, non-executable text file: no rename,
/// copy, mode, link or binary, which `git apply` would otherwise honor.
fn files(patch: &str) -> Result<Vec<FileDiff<'_>>, String> {
    let mut out = Vec::new();
    let mut lines = patch.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(names) = line.strip_prefix("diff --git ") else {
            return Err(format!("{line:?} opens no file"));
        };
        let (mut created, mut deleted, mut old, mut new) = (false, false, None, None);
        while let Some(&header) = lines.peek() {
            if header.starts_with("@@") || header.starts_with("diff --git ") {
                break;
            }
            lines.next();
            match header {
                "new file mode 100644" => created = true,
                "deleted file mode 100644" => deleted = true,
                _ if header.starts_with("index ") => {
                    let mode = header.split(' ').nth(2);
                    if mode.is_some_and(|m| m != "100644") {
                        return Err(format!("{header:?}: a bump writes mode 100644 only"));
                    }
                }
                _ if header.starts_with("--- ") => old = Some(&header[4..]),
                _ if header.starts_with("+++ ") => new = Some(&header[4..]),
                _ => {
                    return Err(format!(
                        "{header:?}: a bump changes no mode, link, name or binary"
                    ));
                }
            }
        }
        let (Some(old), Some(new)) = (old, new) else {
            return Err(format!("{names:?} carries no text change"));
        };
        let path = match (old, new) {
            (DEV_NULL, DEV_NULL) => return Err(format!("{names:?} names no file")),
            (DEV_NULL, b) if created => b.strip_prefix("b/"),
            (a, DEV_NULL) if deleted => a.strip_prefix("a/"),
            (a, b) if !created && !deleted && a.strip_prefix("a/") == b.strip_prefix("b/") => {
                a.strip_prefix("a/")
            }
            _ => None,
        }
        .filter(|p| {
            // publish hands the paths to create-pull-request comma-joined.
            *names == format!("a/{p} b/{p}") && !p.starts_with('"') && !p.contains(['\t', ','])
        })
        .ok_or_else(|| format!("{names:?}: its `---`/`+++` lines name another file"))?;
        let (mut removed, mut added) = (Vec::new(), Vec::new());
        while let Some(&body) = lines.peek() {
            if body.starts_with("diff --git ") {
                break;
            }
            lines.next();
            match body.as_bytes().first() {
                Some(b'-') => removed.push(&body[1..]),
                Some(b'+') => added.push(&body[1..]),
                Some(b' ' | b'\\' | b'@') => {}
                _ => return Err(format!("{body:?} is no hunk line")),
            }
        }
        out.push(FileDiff {
            path: PathBuf::from(path),
            removed,
            added,
            created,
            deleted,
        });
    }
    Ok(out)
}

/// The files `patch` changes, once it is a bump of `p` to `version` and nothing
/// else: every path [`admits`], and the registry edited as
/// [`pin::repinned`] would, `version` above the pin.
///
/// # Errors
///
/// What the patch holds that a bump doesn't.
pub fn admit(
    registry: &str,
    p: &Profile,
    version: &Version,
    patch: &str,
) -> Result<Vec<PathBuf>, String> {
    let Some(expected) = pin::repinned(registry, p.descriptor(), version)? else {
        return Err(format!("{}: {version} is its pin already", p.source));
    };
    let mut changed = registry
        .lines()
        .zip(expected.lines())
        .filter(|(was, now)| was != now);
    let (Some((was, now)), None) = (changed.next(), changed.next()) else {
        return Err(format!("{}: the repin is not one line", p.source));
    };
    let files = files(patch)?;
    let registry_path = registry_relative();
    let mut repinned = false;
    for f in &files {
        if !admits(p, &f.path) {
            return Err(format!("{:?} is outside {}'s bump", f.path, p.source));
        }
        if f.path == registry_path {
            if f.created || f.deleted || f.removed != [was] || f.added != [now] {
                return Err(format!(
                    "the registry's change is not {}'s pin moving to {version}",
                    p.source
                ));
            }
            repinned = true;
        }
    }
    if !repinned {
        return Err(format!("{}'s pin does not move", p.source));
    }
    Ok(files.into_iter().map(|f| f.path).collect())
}

/// The files `paths` (repository-relative) holds under `p`'s fixtures, as they
/// are on disk now: a deleted one is gone.
pub fn fixture_files(p: &Profile, paths: &[PathBuf]) -> Vec<PathBuf> {
    let root = repo_root();
    paths
        .iter()
        .map(|path| root.join(path))
        .filter(|f| {
            f.is_file()
                && p.scenarios
                    .iter()
                    .any(|s| f.starts_with(sources_root().join(s.fixture)))
        })
        .collect()
}

/// Whether the fixture files a bump of `p` left are what the recorder writes:
/// a provenance it strips, from aimock, and transcripts or hook payloads a
/// decoder reads, already stripped, so nothing rides in a subtree no decoder
/// reads.
///
/// # Errors
///
/// The first file that isn't.
pub fn recorded(p: &Profile, files: &[PathBuf]) -> Result<(), String> {
    let mut lines = Vec::new();
    for file in files {
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == PROVENANCE {
            let text = std::fs::read_to_string(file).map_err(|e| format!("{file:?}: {e}"))?;
            let record: Value =
                serde_json::from_str(&text).map_err(|e| format!("{file:?}: {e}"))?;
            let from_aimock = record[PROVENANCE_AIMOCK]
                .as_str()
                .is_some_and(|v| !v.is_empty());
            if !reads::strips(&record) || !from_aimock {
                return Err(format!("{file:?} is not a stripped aimock recording's"));
            }
        } else if name.ends_with(".jsonl") && reads::drive_for(p.source, file).is_some() {
            lines.push(file.clone());
        } else {
            return Err(format!(
                "{file:?} is no file a recording of {} writes",
                p.source
            ));
        }
    }
    let strip = reads::strip_unread(p.source, &lines).map_err(|e| e.to_string())?;
    if strip.blanked > 0 || !strip.not_json.is_empty() {
        return Err(format!(
            "{}'s recording holds what no decoder reads: {} subtrees, unparsed {:?}",
            p.source, strip.blanked, strip.not_json
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn moved(rows: &[(&str, &[(&str, &str)])]) -> Vec<(String, Vec<Moved>)> {
        rows.iter()
            .map(|(os, rows)| {
                let rows = rows
                    .iter()
                    .map(|(source, version)| Moved {
                        source: (*source).to_string(),
                        version: (*version).to_string(),
                    })
                    .collect();
                ((*os).to_string(), rows)
            })
            .collect()
    }

    #[test]
    fn a_release_moved_on_every_os_is_proposed_once() {
        let up = above(codex()).to_string();
        let both = moved(&[("macOS", &[("codex", &up)]), ("Windows", &[("codex", &up)])]);
        let proposals = plan(&both, &[]).expect("a plan");
        assert_eq!(proposals, [Proposal::new("codex", &v(&up))]);
        let taken = [proposals[0].title.clone()];
        assert_eq!(plan(&both, &taken), Ok(vec![]), "proposed or declined");
    }

    #[test]
    fn a_release_moved_on_one_os_or_at_two_versions_is_not() {
        let up = above(codex());
        let next = Version::new(up.major, up.minor, up.patch + 1).to_string();
        let up = up.to_string();
        for rows in [
            moved(&[("macOS", &[("codex", &up)]), ("Windows", &[])]),
            moved(&[
                ("macOS", &[("codex", &up)]),
                ("Windows", &[("codex", &next)]),
            ]),
        ] {
            assert_eq!(plan(&rows, &[]), Ok(vec![]), "{rows:?}");
        }
        let pin = codex().descriptor().verified_version;
        let at_pin = moved(&[("macOS", &[("codex", pin)]), ("Windows", &[("codex", pin)])]);
        assert_eq!(plan(&at_pin, &[]), Ok(vec![]));
    }

    #[test]
    fn a_plan_refuses_rows_the_tier_cannot_have_written() {
        let up = above(codex()).to_string();
        let pin = pin::pinned(codex().descriptor()).expect("a pin");
        let below = Version {
            pre: semver::Prerelease::new("rc.1").expect("a prerelease"),
            ..pin
        }
        .to_string();
        for rows in [
            moved(&[("macOS", &[("codex", &up)])]),
            moved(&[
                ("macOS", &[("codex", &up)]),
                ("Windows", &[("codex", &up)]),
                ("Linux", &[("codex", &up)]),
            ]),
            moved(&[("macOS", &[("codex", &up)]), ("macOS", &[("codex", &up)])]),
            moved(&[("macOS", &[("nope", &up)]), ("Windows", &[("nope", &up)])]),
            moved(&[
                ("macOS", &[("codex", &up), ("codex", &up)]),
                ("Windows", &[("codex", &up)]),
            ]),
            moved(&[
                ("macOS", &[("codex", "1.x")]),
                ("Windows", &[("codex", "1.x")]),
            ]),
            moved(&[
                ("macOS", &[("codex", &below)]),
                ("Windows", &[("codex", &below)]),
            ]),
        ] {
            assert!(plan(&rows, &[]).is_err(), "{rows:?}");
        }
    }

    #[test]
    fn a_bump_admits_its_registry_fixtures_and_snapshots_alone() {
        let p = codex();
        let sources = relative(&sources_root());
        let fixture = sources.join(p.scenarios[0].fixture);
        let name = fixture.file_name().expect("a name").to_string_lossy();
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

    /// The patch `git diff --no-renames` writes for codex's repin plus `extra`.
    fn patch(version: &Version, extra: &str) -> String {
        let src = registry();
        let moved = pin::repinned(&src, codex().descriptor(), version)
            .expect("a repin")
            .expect("a move");
        let (was, now) = src
            .lines()
            .zip(moved.lines())
            .find(|(a, b)| a != b)
            .expect("one changed line");
        let reg = registry_relative();
        let reg = reg.to_string_lossy();
        format!(
            "diff --git a/{reg} b/{reg}\nindex 1111111..2222222 100644\n--- a/{reg}\n+++ b/{reg}\n\
             @@ -1,3 +1,3 @@\n context\n-{was}\n+{now}\n context\n{extra}"
        )
    }

    fn fixture_file(name: &str) -> String {
        relative(&sources_root().join(codex().scenarios[0].fixture).join(name))
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn a_bump_patch_is_admitted_with_its_paths() {
        let up = above(codex());
        let f = fixture_file("hook-payloads.jsonl");
        let created = fixture_file("new.jsonl");
        let extra = format!(
            "diff --git a/{f} b/{f}\nindex 3333333..4444444 100644\n--- a/{f}\n+++ b/{f}\n\
             @@ -1 +1 @@\n-{{}}\n+{{\"a\":1}}\n\
             diff --git a/{created} b/{created}\nnew file mode 100644\nindex 0000000..5555555\n\
             --- /dev/null\n+++ b/{created}\n@@ -0,0 +1 @@\n+{{}}\n"
        );
        let paths = admit(&registry(), codex(), &up, &patch(&up, &extra)).expect("admitted");
        assert_eq!(
            paths,
            [
                registry_relative(),
                PathBuf::from(&f),
                PathBuf::from(&created)
            ]
        );
    }

    #[test]
    fn a_bump_patch_refuses_what_a_recording_never_writes() {
        let up = above(codex());
        let f = fixture_file("hook-payloads.jsonl");
        let g = fixture_file("other.jsonl");
        let edit = |header: &str| {
            format!("diff --git a/{f} b/{f}\n{header}--- a/{f}\n+++ b/{f}\n@@ -1 +1 @@\n-a\n+b\n")
        };
        for extra in [
            edit("old mode 100644\nnew mode 100755\n"),
            edit("index 3333333..4444444 120000\n"),
            edit("similarity index 90%\n"),
            format!("diff --git a/{f} b/{g}\nrename from {f}\nrename to {g}\n"),
            format!("diff --git a/{f} b/{f}\nnew file mode 100755\n--- /dev/null\n+++ b/{f}\n@@ -0,0 +1 @@\n+a\n"),
            format!("diff --git a/{f} b/{f}\nindex 3333333..4444444 100644\nBinary files a/{f} and b/{f} differ\n"),
            format!("diff --git a/{f} b/{f}\n--- a/{g}\n+++ b/{g}\n@@ -1 +1 @@\n-a\n+b\n"),
            format!("diff --git a/{f} b/{f}\n--- a/{f}\n+++ b/{f}\n@@ -1 +1 @@\n-a\n*b\n"),
            "diff --git a/justfile b/justfile\n--- a/justfile\n+++ b/justfile\n@@ -1 +1 @@\n-a\n+b\n"
                .to_string(),
            format!("diff --git \"a/{f}\" \"b/{f}\"\n--- \"a/{f}\"\n+++ \"b/{f}\"\n@@ -1 +1 @@\n-a\n+b\n"),
            {
                let comma = fixture_file("a,b.jsonl");
                format!("diff --git a/{comma} b/{comma}\n--- a/{comma}\n+++ b/{comma}\n@@ -1 +1 @@\n-a\n+b\n")
            },
        ] {
            assert!(
                admit(&registry(), codex(), &up, &patch(&up, &extra)).is_err(),
                "{extra}"
            );
        }
        assert!(
            admit(
                &registry(),
                codex(),
                &up,
                &format!("junk\n{}", patch(&up, ""))
            )
            .is_err(),
            "text before the first file"
        );
    }

    #[test]
    fn a_bump_patch_moves_the_pin_exactly_once_and_up() {
        let src = registry();
        let up = above(codex());
        let next = Version::new(up.major, up.minor, up.patch + 1);
        assert!(admit(&src, codex(), &up, &patch(&up, "")).is_ok());
        assert!(
            admit(&src, codex(), &next, &patch(&up, "")).is_err(),
            "another release"
        );
        let pin = pin::pinned(codex().descriptor()).expect("a pin");
        assert!(
            admit(&src, codex(), &pin, &patch(&up, "")).is_err(),
            "no move"
        );
        let f = fixture_file("hook-payloads.jsonl");
        let fixtures_only =
            format!("diff --git a/{f} b/{f}\n--- a/{f}\n+++ b/{f}\n@@ -1 +1 @@\n-a\n+b\n");
        assert!(
            admit(&src, codex(), &up, &fixtures_only).is_err(),
            "the pin stays"
        );
        let reg = registry_relative();
        let reg = reg.to_string_lossy();
        let second_line =
            format!("diff --git a/{reg} b/{reg}\n--- a/{reg}\n+++ b/{reg}\n@@ -1 +1 @@\n-a\n+b\n");
        assert!(
            admit(&src, codex(), &up, &patch(&up, &second_line)).is_err(),
            "another registry line"
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pixtuoid-bump-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch dir");
        dir
    }

    #[test]
    fn a_recording_is_admitted_only_as_the_recorder_writes_it() {
        let p = codex();
        let committed = sources_root().join(p.scenarios[0].fixture);
        let dir = scratch("recorded");
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&committed).expect("codex's fixture") {
            let from = entry.expect("an entry").path();
            let to = dir.join(from.file_name().expect("a name"));
            std::fs::copy(&from, &to).expect("a copy");
            files.push(to);
        }
        assert_eq!(recorded(p, &files), Ok(()), "the committed recording");

        let provenance = dir.join(PROVENANCE);
        let text = std::fs::read_to_string(&provenance).expect("provenance");
        let mut record: Value = serde_json::from_str(&text).expect("json");
        record
            .as_object_mut()
            .expect("an object")
            .remove(PROVENANCE_AIMOCK);
        std::fs::write(&provenance, record.to_string()).expect("write");
        assert!(recorded(p, &files).is_err(), "no aimock");
        record[PROVENANCE_AIMOCK] = Value::from("1.0.0");
        record["deidentified"]["method"] = Value::from("none");
        std::fs::write(&provenance, record.to_string()).expect("write");
        assert!(recorded(p, &files).is_err(), "exempt from the strip");
        std::fs::write(&provenance, &text).expect("restore");

        let payloads = dir.join(crate::harness::HOOK_PAYLOADS);
        let mut lines: Vec<Value> = std::fs::read_to_string(&payloads)
            .expect("payloads")
            .lines()
            .map(|l| serde_json::from_str(l).expect("a json line"))
            .collect();
        lines[0]["pixtuoid_bump_payload"] = Value::from("unread");
        let rewritten: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(&payloads, rewritten).expect("write");
        assert!(recorded(p, &files).is_err(), "a subtree no decoder reads");

        let stray = dir.join("build.rs");
        std::fs::write(&stray, "fn main() {}").expect("write");
        assert!(recorded(p, &[stray]).is_err(), "no recording writes code");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
