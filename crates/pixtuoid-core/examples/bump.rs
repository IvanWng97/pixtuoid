//! The nightly's pin bumps, one subcommand per job; the rules are
//! `pixtuoid_core::harness::hermetic::bump`'s.
//!
//! - `plan <taken.json> <os>=<moved.json>...`: the proposals, as JSON, given
//!   the titles `gh pr list --json title` names and each drift leg's moved rows.
//! - `patch <source> <out>`: the tree's bump of `source` as a patch at `out`;
//!   nothing is written when the tree is unchanged.
//! - `check <source> <version> <patch>`: applies an admitted patch to the tree
//!   and prints its proposal with the paths it changed.
//!
//! Exits 0 on success, 1 on a refusal, and 2 on a usage error.

use std::path::{Path, PathBuf};
use std::process::Command;

use pixtuoid_core::harness::hermetic::bump::{self, Moved, Proposal};
use pixtuoid_core::harness::hermetic::{self, Profile, pin};
use semver::Version;
use serde::{Deserialize, Serialize};

const USAGE: &str = "usage: bump plan <taken.json> <os>=<moved.json>...\n       \
                     bump patch <source> <out>\n       \
                     bump check <source> <version> <patch>";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let done = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["plan", taken, ref moved @ ..] if !moved.is_empty() => plan(Path::new(taken), moved),
        ["patch", source, out] => patch(source, Path::new(out)),
        ["check", source, version, patch] => check(source, version, Path::new(patch)),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(e) = done {
        eprintln!("bump: {e}");
        std::process::exit(1);
    }
}

#[derive(Deserialize)]
struct Titled {
    title: String,
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path:?}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{path:?}: {e}"))
}

fn plan(taken: &Path, moved: &[&str]) -> Result<(), String> {
    let taken: Vec<String> = read_json::<Vec<Titled>>(taken)?
        .into_iter()
        .map(|t| t.title)
        .collect();
    let moved = moved
        .iter()
        .map(|arg| {
            let (os, file) = arg
                .split_once('=')
                .ok_or_else(|| format!("{arg:?} is not <os>=<moved.json>"))?;
            Ok((os.to_string(), read_json::<Vec<Moved>>(Path::new(file))?))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let proposals = bump::plan(&moved, &taken)?;
    println!(
        "{}",
        serde_json::to_string(&proposals).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn profile(source: &str) -> Result<&'static Profile, String> {
    hermetic::profile(source).ok_or_else(|| format!("{source:?} has no profile"))
}

/// `git` at the repository root; its stdout.
fn git(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("git")
        .current_dir(bump::repo_root())
        .args(args)
        .output()
        .map_err(|e| format!("git {args:?}: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(format!(
            "git {args:?} exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn patch(source: &str, out: &Path) -> Result<(), String> {
    let p = profile(source)?;
    // A recording can add a file; intent-to-add puts it in the diff.
    git(&["add", "--intent-to-add", "--all", "--", "."])?;
    let names = git(&["diff", "--name-only", "--no-renames", "-z"])?;
    let changed: Vec<PathBuf> = names
        .split(|&b| b == 0)
        .filter(|n| !n.is_empty())
        .map(|n| PathBuf::from(String::from_utf8_lossy(n).into_owned()))
        .collect();
    if let Some(stray) = changed.iter().find(|path| !bump::admits(p, path)) {
        return Err(format!("{stray:?} changed, outside {source}'s bump"));
    }
    if changed.is_empty() {
        println!("{source}: the tree is unchanged, so there is no patch");
        return Ok(());
    }
    let diff = git(&[
        "diff",
        "--no-renames",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ])?;
    std::fs::write(out, diff).map_err(|e| format!("{out:?}: {e}"))
}

/// What publish opens: the proposal, and the paths `add-paths` stages.
#[derive(Serialize)]
struct Admitted {
    #[serde(flatten)]
    proposal: Proposal,
    paths: Vec<String>,
}

fn check(source: &str, version: &str, patch: &Path) -> Result<(), String> {
    let p = profile(source)?;
    let version = Version::parse(version).map_err(|e| format!("{version:?}: {e}"))?;
    let text = std::fs::read_to_string(patch).map_err(|e| format!("{patch:?}: {e}"))?;
    let registry = std::fs::read_to_string(pin::registry_path()).map_err(|e| e.to_string())?;
    let paths = bump::admit(&registry, p, &version, &text)?;
    let patch = patch
        .to_str()
        .ok_or_else(|| format!("{patch:?} is not UTF-8"))?;
    git(&["apply", "--check", patch])?;
    git(&["apply", patch])?;
    bump::recorded(p, &bump::fixture_files(p, &paths))?;
    let admitted = Admitted {
        proposal: Proposal::new(source, &version),
        paths: paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
    };
    println!(
        "{}",
        serde_json::to_string(&admitted).map_err(|e| e.to_string())?
    );
    Ok(())
}
