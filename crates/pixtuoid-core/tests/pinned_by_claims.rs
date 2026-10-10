//! Every `Pinned by `x`` comment names a function that exists — without this the
//! citation is the prose the convention replaced. It caught `codex.rs` citing
//! `escalated_permission_is_detected_by_the_exported_pair`, which had never
//! existed.
//!
//! Two gaps, neither currently harbouring an orphan — widen before trusting it
//! further: the walk is `.rs` under `crates/`, so a claim anywhere else — a
//! sprite header, a prompt, `docs/` — goes unchecked; and `declared` is a raw
//! text scan, so `fn foo` written in prose counts as a declaration. It checks
//! the name EXISTS, never that the test pins the claim.
//!
//! Reads sibling crates at runtime, so it is workspace-only and sits in
//! `Cargo.toml`'s `exclude`.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const CRATES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // `target/` holds generated copies of our own sources, which would
        // double-count both sides and can carry a stale orphan indefinitely.
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs")
            // This file's own control fixtures ARE deliberate orphans.
            && !path.ends_with("tests/pinned_by_claims.rs")
        {
            out.push(path);
        }
    }
}

/// Collapse the comment leaders so a wrapped claim reads as one string.
///
/// Indentation is stripped per line first: anchoring the replace at column 0
/// left an indented leader intact, and the claim wrapped under it was skipped
/// silently (this file's module doc used to list that as a known gap).
fn flatten_wrapped_comments(src: &str) -> String {
    src.lines()
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n")
        .replace("\n///", " ")
        .replace("\n//!", " ")
        .replace("\n//", " ")
}

/// How far a claim's name may trail "pinned by" ("pinned by the TUI's `x`")
/// and still be the claim's.
const MAX_QUALIFIER_BYTES: usize = 24;

/// The identifier a `Pinned by` claim names, if the line makes one.
///
/// Matched on text with comment leaders collapsed to a space first: a claim
/// wrapped across two `///` lines is exactly the shape a line-at-a-time matcher
/// passes silently, so the wrap has to be gone before the scan.
fn claims_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(idx) = rest.find("inned by") {
        rest = &rest[idx + "inned by".len()..];
        let Some(tick) = rest.find('`') else { break };
        let qualifier = &rest[..tick];
        if qualifier.len() > MAX_QUALIFIER_BYTES || qualifier.contains(['.', ',', ';', ':', '\n']) {
            continue;
        }
        let body = &rest[tick + 1..];
        let Some(end) = body.find('`') else { continue };
        let name = &body[..end];
        if name.len() >= 4
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            out.push(name.to_string());
        }
    }
    out
}

#[test]
fn every_pinned_by_claim_names_a_function_that_exists() {
    let mut files = Vec::new();
    rust_sources(Path::new(CRATES_DIR), &mut files);
    assert!(
        files.len() > 100,
        "expected the whole tree, got {}",
        files.len()
    );

    let mut declared = BTreeSet::new();
    let mut claims: Vec<(PathBuf, String)> = Vec::new();
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        for (_, tail) in src.match_indices("fn ").map(|(i, _)| src.split_at(i + 3)) {
            let name: String = tail
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                declared.insert(name);
            }
        }
        claims.extend(
            claims_in(&flatten_wrapped_comments(&src))
                .into_iter()
                .map(|n| (path.clone(), n)),
        );
    }

    assert!(
        !claims.is_empty(),
        "the scan found no `Pinned by` claims at all"
    );

    let orphans: Vec<String> = claims
        .iter()
        .filter(|(_, name)| !declared.contains(name))
        .map(|(path, name)| format!("{}: claims `Pinned by {name}`", path.display()))
        .collect();
    assert!(
        orphans.is_empty(),
        "a `Pinned by` claim names no function in the tree — an unbacked claim of \
         coverage. Name the real mechanism or drop the claim:\n{}",
        orphans.join("\n")
    );
}

/// The negative control: without it the test above passes on a scan that never
/// matched anything, which is the failure mode a citation guard cannot have.
#[test]
fn the_claim_scanner_fires_on_an_orphan_and_stays_silent_on_a_real_one() {
    assert_eq!(
        claims_in("/// Pinned by `some_test_name`."),
        ["some_test_name"]
    );
    assert_eq!(
        claims_in(&flatten_wrapped_comments(
            "/// Pinned by\n/// `wrapped_across_lines`."
        )),
        ["wrapped_across_lines"],
        "a wrapped claim must survive the REAL collapse — the old hand-flattened \
         fixture never exercised it, which is how the column-0 gap stayed hidden"
    );
    assert_eq!(
        claims_in("/// pinned by [`bracketed_link`]"),
        ["bracketed_link"]
    );
    assert_eq!(
        claims_in(&flatten_wrapped_comments(
            "    // Pinned by\n    // `indented_wrapped_name`.\n    for x in y {"
        )),
        ["indented_wrapped_name"],
        "an INDENTED wrapped claim is a live shape in the tree — \
         a column-0-anchored collapse skips it silently"
    );
    assert_eq!(
        claims_in("/// in place (pinned by the TUI's `qualified_name`)."),
        ["qualified_name"],
        "a short qualifier before the name is still the same claim"
    );
    assert!(claims_in("/// Pinned by the shared harness.").is_empty());
    assert!(
        claims_in("/// Pinned by the shared harness. Its `fixture_name` differs.").is_empty(),
        "a sentence end closes the claim before a later name"
    );
    for stop in ['.', ',', ';', ':', '\n'] {
        assert!(
            claims_in(&format!("/// Pinned by e2e{stop} see `fixture_name`")).is_empty(),
            "{stop:?} closes the claim inside the qualifier bound"
        );
    }
    // The qualifier counts the spaces either side of it.
    let qualified = |len: usize| format!("/// pinned by {} `qualified_name`", "x".repeat(len - 2));
    assert_eq!(
        claims_in(&qualified(MAX_QUALIFIER_BYTES)),
        ["qualified_name"],
        "a qualifier at the bound is still the claim's"
    );
    assert!(
        claims_in(&qualified(MAX_QUALIFIER_BYTES + 1)).is_empty(),
        "a name past the qualifier bound is no longer the claim's"
    );
    assert!(
        claims_in("/// Pinned by `CONST_NAME`").is_empty(),
        "a SCREAMING const is not a function name"
    );
}
