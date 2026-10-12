//! This crate's half of the **drift surface** — the names its installers ship,
//! emitted as data for `check_upstream_drift.py`, the watches not yet
//! declared beside their source (`source/upstream.rs`).
//!
//! That watcher is a separate program that cannot call us, and until #942 it
//! REGEX-PARSED this crate's source instead. That fails SILENTLY: rename a
//! const and the parser returns a smaller set, so the watch narrows with
//! nothing to show for it. Emitting makes it a test failure HERE.
//!
//! `crates/pixtuoid` emits the other half — the values its install templates
//! ship — because it owns those tables.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// The committed fragment, by absolute path so a worktree writes its own.
const FRAGMENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/drift-surface.json");

fn surface() -> Value {
    let mut shipped: BTreeMap<&str, Value> = BTreeMap::new();
    // The value rides the file the installer writes; why it is watched is on
    // `omp::EXTENSIONS_SUBDIR`.
    shipped.insert(
        "omp.extension_subdir",
        json!([crate::source::omp::EXTENSIONS_SUBDIR]),
    );

    // Through a BTreeMap, never a `json!` object literal: the workspace's
    // serde_json `preserve_order` keeps a literal's insertion order under a
    // whole-workspace build and sorts it under `-p pixtuoid-core`, so the
    // committed file would pass one way and fail the other.
    let mut root: BTreeMap<&str, Value> = BTreeMap::new();
    root.insert("shipped", json!(shipped));
    json!(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed fragment IS what the installers ship. Regenerate with
    /// `just gen-drift-surface`.
    ///
    /// This is the gate, and it is a TEST rather than a CI-only job so it runs
    /// on every platform in `just test` — a stale fragment is a narrowed watch,
    /// which is precisely the failure that cannot announce itself.
    #[test]
    fn the_committed_fragment_matches_what_the_installers_ship() {
        let want = serde_json::to_string_pretty(&surface()).expect("surface serializes") + "\n";
        let committed = snapbox::Data::read_from(
            std::path::Path::new(FRAGMENT),
            Some(snapbox::data::DataFormat::Text),
        );
        snapbox::assert_data_eq!(want, committed.raw());
    }

    /// Every value in the fragment is a non-empty name, and no set is empty: an
    /// emptied set would narrow the watch to nothing while the gate above stayed
    /// green, because it compares the file to the same empty set.
    #[test]
    fn no_emitted_set_is_empty() {
        let s = surface();
        let groups = s.as_object().expect("the fragment root is an object");
        for (group, entries) in groups {
            let obj = entries.as_object().expect("group is an object");
            for (k, v) in obj {
                match v {
                    Value::Array(a) => {
                        assert!(!a.is_empty(), "{group}.{k} is an empty set");
                        assert!(
                            a.iter().all(|x| x.as_str().is_some_and(|s| !s.is_empty())),
                            "{group}.{k} carries a blank name",
                        );
                    }
                    Value::String(_) => panic!(
                        "{group}.{k} is a bare string — the Python reader does \
                         set(value), so it reads as a set of CHARACTERS; wrap it \
                         in a 1-element array"
                    ),
                    other => panic!("{group}.{k} is neither a set nor a value: {other}"),
                }
            }
        }
    }

    /// Every dispatch arm in `file` names a CONST, and those consts are exactly
    /// the ones `export` declares.
    ///
    /// A drift watch carries the const in its source's `UPSTREAM`, so it
    /// follows a RENAME. It cannot see an ADDITION: an arm
    /// written as a bare literal decodes a name no watch carries, and every
    /// other gate stays green — so a bare literal is rejected outright.
    ///
    /// Reading our own source here is not what this crate stopped doing: that
    /// was a FOREIGN program regex-parsing us, where a miss narrowed the watch
    /// in silence. A miss here fails this crate's own test.
    fn assert_arms_match_export(file: &str, dispatch: &str, export: &str) {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/source")
                .join(file),
        )
        .expect("the module is readable");

        let after = src
            .split_once(dispatch)
            .unwrap_or_else(|| panic!("{file}: no `{dispatch}` dispatch"))
            .1;
        let body = &after[..after.find("\n}\n").unwrap_or(after.len())];
        // The bare-literal ban runs at EVERY depth, not just the level the export
        // is collected from: a nested `match` (acp's on `status`) would otherwise
        // hide wire values behind an indent the collector skips. Naming them is
        // what puts them in front of a reviewer and lets them be exported.
        for line in body.lines() {
            let Some((head, _)) = line.split_once("=>") else {
                continue;
            };
            // ANY `"` in the head, not just a leading or `(`-prefixed one: the
            // shape that escaped was a guard, `g if g == "wire_name" =>`, which
            // dispatches on a name no watch carries while `armed` stays
            // equal to the export because the token is not SCREAMING_SNAKE.
            assert!(
                !head.contains('"'),
                "{file}: arm `{}` in {export}'s dispatch reads a bare wire literal — \
                 name it as a const so a drift watch can carry it",
                head.trim(),
            );
        }
        let mut armed: Vec<&str> = Vec::new();
        for line in body.lines() {
            if !line.contains("=>")
                || !line.starts_with("        ")
                || line.starts_with("         ")
            {
                continue;
            }
            for tok in line
                .split("=>")
                .next()
                .unwrap_or("")
                .split('|')
                .map(str::trim)
            {
                if tok != "_"
                    && !tok.is_empty()
                    && tok
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
                {
                    armed.push(tok);
                }
            }
        }

        let decl = src
            .split_once(&format!("{export}: &[&str] = &["))
            .unwrap_or_else(|| panic!("{file}: no {export} declaration"))
            .1;
        let mut declared: Vec<&str> = decl[..decl.find("];").unwrap_or(0)]
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect();

        armed.sort_unstable();
        armed.dedup();
        declared.sort_unstable();
        assert_eq!(armed, declared, "{file}: dispatch arms vs {export}");
    }

    /// One row per decoder whose export is a match dispatch. Every OTHER
    /// `DECODED_*` export names the test that pins it instead — a new one gets
    /// neither by default, and the census below is what refuses it.
    #[test]
    fn every_exported_set_is_exactly_its_dispatch_arms() {
        let rows = [
            (
                "acp.rs",
                "match str_field(\"sessionUpdate\").unwrap_or(\"\") {",
                "DECODED_TAGS",
            ),
            ("copilot.rs", "let out = match kind {", "DECODED_KINDS"),
            ("opencode.rs", "match event {", "DECODED_EVENTS"),
            ("opencode.rs", "match status {", "DECODED_PART_STATUSES"),
            ("omp.rs", "match ty {", "DECODED_HOOK_EVENTS"),
            (
                "claude_code.rs",
                ".and_then(|t| t.as_str())?\n    {",
                "DECODED_EFFORT_ATTACHMENTS",
            ),
        ];
        for (file, dispatch, export) in rows {
            assert_arms_match_export(file, dispatch, export);
        }

        // Each entry is the const and the test that pins it, so a renamed test
        // cannot leave a row still claiming coverage.
        const PINNED_ELSEWHERE: &[(&str, &str)] = &[
            (
                "DECODED_TERMINAL_STATUSES",
                "tool_call_starts_and_terminal_update_ends_keyed_by_tool_call_id",
            ),
            (
                "DECODED_TYPES",
                "the_decoded_set_is_exactly_what_the_arms_match",
            ),
            (
                "DECODED_FIELDS",
                "the_field_set_is_exactly_what_the_decoder_reads",
            ),
            (
                "DECODED_DISPATCH_NAMES",
                "the_dispatch_name_set_is_exactly_what_the_fallback_matches",
            ),
            (
                "DECODED_ENTRY_TYPES",
                "the_decoded_entry_type_set_is_exactly_what_the_arms_match",
            ),
            ("DECODED_EXIT_MARKER", "session_exit_ends_root_not_as_child"),
            ("DECODED_ESCALATION", "escalated_function_call_is_waiting"),
            (
                "DECODED_MESSAGE_VOCAB",
                "the_exported_message_vocabulary_is_exactly_what_the_arms_match",
            ),
            (
                "DECODED_TITLE_FIELDS",
                "the_exported_title_field_set_is_exactly_what_both_readers_use",
            ),
            // grok's arms nest inside the method arm, so the shared dispatch
            // scanner above cannot reach them.
            (
                "DECODED_XAI_TAGS",
                "the_exported_xai_tag_set_is_exactly_what_the_arms_match",
            ),
        ];
        // Recursive: `src/source/` has nine module directories, and a const in
        // one would otherwise be invisible to the census that exists to see it.
        fn collect(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
            for entry in std::fs::read_dir(dir).expect("the source dir is readable") {
                let path = entry.expect("a readable entry").path();
                if path.is_dir() {
                    collect(&path, out);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let src = std::fs::read_to_string(&path).expect("the module is readable");
                for line in src.lines() {
                    let Some((_, rest)) = line.split_once("const DECODED_") else {
                        continue;
                    };
                    if let Some((name, _)) = rest.split_once(':') {
                        out.push((format!("DECODED_{name}"), src.clone()));
                    }
                }
            }
        }
        let mut hits: Vec<(String, String)> = Vec::new();
        collect(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/source"),
            &mut hits,
        );
        for (name, src) in &hits {
            if let Some((_, test)) = PINNED_ELSEWHERE.iter().find(|(n, _)| n == name) {
                let body = src
                    .split_once(&format!("fn {test}("))
                    .map(|(_, rest)| rest)
                    .unwrap_or("");
                assert!(
                    !body.is_empty(),
                    "{name} claims to be pinned by {test}, which its module no longer \
                     declares — the exemption is now an unbacked claim of coverage",
                );
                assert!(
                    body.split("\n    fn ").next().unwrap_or("").contains(name),
                    "{test} exists but never names {name}, so the exemption claims a \
                     coverage the test does not provide — have it read the const",
                );
            }
        }
        let mut found: Vec<String> = hits.into_iter().map(|(n, _)| n).collect();
        let mut covered: Vec<String> = rows
            .iter()
            .map(|(_, _, export)| (*export).to_owned())
            .chain(PINNED_ELSEWHERE.iter().map(|(e, _)| (*e).to_owned()))
            .collect();
        found.sort_unstable();
        found.dedup();
        covered.sort_unstable();
        covered.dedup();
        assert_eq!(
            found, covered,
            "a DECODED_* export needs a row above or a named pin in PINNED_ELSEWHERE — \
             or, the other direction, a row here outlived the const it named"
        );
    }
}
