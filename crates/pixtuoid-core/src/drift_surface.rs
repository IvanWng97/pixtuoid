//! This crate's half of the **drift surface** — the names our decoders read,
//! emitted as data for `check_upstream_drift.py`.
//!
//! The watcher is a separate program that cannot call us, and for years it
//! REGEX-PARSED this crate's source instead. That fails SILENTLY: rename a
//! const and the parser returns a smaller set, so the watch narrows with
//! nothing to show for it. Emitting makes it a test failure HERE.
//!
//! `crates/pixtuoid` emits the other half — the `*_EVENTS` sharp edge in its
//! `SHARP-EDGES.md`, not an organisational split.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::source::codex::{
    EM_RESUME, EM_SEARCH, EM_TOKENS, EM_TURN_END, EM_TURN_START, EVENT_MSG, RESPONSE_ITEM,
    RI_RESUME, RI_SEARCH, RI_TOOL_START, TURN_CONTEXT,
};

/// Where the committed fragment lives, relative to the workspace root.
const FRAGMENT: &str = "crates/pixtuoid-core/drift-surface.json";

/// Set when regenerating: `UPDATE_DRIFT_SURFACE=1 cargo test -p pixtuoid-core`.
const UPDATE_ENV: &str = "UPDATE_DRIFT_SURFACE";

fn surface() -> Value {
    // A BTreeMap so the emitted order is the key order, not a hash order — the
    // committed file has to be byte-stable or the gate flaps.
    let mut decoded: BTreeMap<&str, Value> = BTreeMap::new();
    decoded.insert(
        "acp.session_update_tags",
        json!(crate::source::acp::DECODED_TAGS),
    );
    decoded.insert(
        "acp.terminal_statuses",
        json!(crate::source::acp::DECODED_TERMINAL_STATUSES),
    );
    decoded.insert(
        "codex.event_msg",
        json!([EM_TURN_START, EM_RESUME, EM_SEARCH, EM_TURN_END, EM_TOKENS].concat()),
    );
    decoded.insert(
        "codex.response_item",
        json!([RI_TOOL_START, RI_RESUME, RI_SEARCH].concat()),
    );
    decoded.insert(
        "codex.rollout_outers",
        json!([EVENT_MSG, RESPONSE_ITEM, TURN_CONTEXT]),
    );
    decoded.insert(
        "grok.xai_method",
        json!([crate::source::grok::DECODED_XAI_METHOD]),
    );
    decoded.insert(
        "grok.xai_tags",
        json!(crate::source::grok::DECODED_XAI_TAGS),
    );
    decoded.insert(
        "omp.message_vocab",
        json!(crate::source::omp::DECODED_MESSAGE_VOCAB),
    );
    decoded.insert(
        "omp.exit_marker",
        json!([crate::source::omp::DECODED_EXIT_MARKER]),
    );
    decoded.insert(
        "omp.entry_types",
        json!(crate::source::omp::DECODED_ENTRY_TYPES),
    );
    decoded.insert(
        "copilot.kinds",
        json!(crate::source::copilot::DECODED_KINDS),
    );
    decoded.insert(
        "copilot.payload_fields",
        json!(crate::source::copilot::DECODED_FIELDS),
    );
    decoded.insert(
        "decoder.dispatch_names",
        json!(super::source::decoder::DECODED_DISPATCH_NAMES),
    );
    decoded.insert(
        "opencode.hook_events",
        json!(crate::source::opencode::DECODED_EVENTS),
    );

    // Through a BTreeMap, never a `json!` object literal — see
    // `every_emitted_object_has_sorted_keys` for why that distinction is a gate.
    let mut root: BTreeMap<&str, Value> = BTreeMap::new();
    root.insert("decoded", json!(decoded));
    json!(root)
}

fn fragment_path() -> PathBuf {
    // CARGO_MANIFEST_DIR is this crate; the fragment path is workspace-relative.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(FRAGMENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed fragment IS what the decoders read. Regenerate with
    /// `UPDATE_DRIFT_SURFACE=1 cargo test -p pixtuoid-core`.
    ///
    /// This is the gate, and it is a TEST rather than a CI-only job so it runs
    /// on every platform in `just test` — a stale fragment is a narrowed watch,
    /// which is precisely the failure that cannot announce itself.
    #[test]
    fn the_committed_fragment_matches_what_the_decoders_read() {
        let want = serde_json::to_string_pretty(&surface()).expect("surface serializes");
        let path = fragment_path();
        if std::env::var_os(UPDATE_ENV).is_some() {
            std::fs::write(&path, format!("{want}\n")).expect("fragment is writable");
            return;
        }
        let got = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!("{FRAGMENT} is missing ({e}); regenerate with {UPDATE_ENV}=1")
        });
        assert_eq!(
            got.trim_end(),
            want,
            "{FRAGMENT} is stale — regenerate with `just gen-drift-surface`",
        );
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
                    Value::String(x) => assert!(!x.is_empty(), "{group}.{k} is blank"),
                    other => panic!("{group}.{k} is neither a set nor a value: {other}"),
                }
            }
        }
    }

    /// Byte-stability across feature unification. The workspace enables
    /// serde_json's `preserve_order` (`pixtuoid` asks for it), and feature
    /// unification hands it to every crate — so an object built by `json!({…})`
    /// is sorted under `cargo test -p` and insertion-ordered under
    /// `cargo nextest run --workspace`. That difference reached the committed
    /// file once and made the gate above pass one way and fail the other.
    #[test]
    fn every_emitted_object_has_sorted_keys() {
        fn walk(v: &Value, path: &str) {
            if let Some(o) = v.as_object() {
                let keys: Vec<&str> = o.keys().map(String::as_str).collect();
                let mut sorted = keys.clone();
                sorted.sort_unstable();
                assert_eq!(keys, sorted, "{path} emits keys in insertion order");
                for (k, x) in o {
                    walk(x, &format!("{path}.{k}"));
                }
            }
        }
        walk(&surface(), "<root>");
    }

    /// Every dispatch arm in `file` names a CONST, and those consts are exactly
    /// the ones `export` declares.
    ///
    /// The fragment binds a RENAME — change a const's value and the committed
    /// file goes stale. It cannot bind an ADDITION: an arm written as a bare
    /// literal decodes a name the surface never mentions, and every other gate
    /// stays green — so a bare literal is rejected outright.
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
            assert!(
                !head.trim_start().starts_with('"') && !head.contains("(\""),
                "{file}: arm `{}` in {export}'s dispatch reads a bare wire literal — \
                 name it as a const so the drift surface can carry it",
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
                assert!(
                    !tok.starts_with('"'),
                    "{file}: arm {tok} dispatches on a bare literal — name it and put it in \
                     {export}, or the decoder reads a name the drift surface omits",
                );
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
            (
                "DECODED_MESSAGE_VOCAB",
                "the_exported_message_vocabulary_is_exactly_what_the_arms_match",
            ),
            // grok's arms nest inside the method arm, so the shared dispatch
            // scanner above cannot reach them.
            (
                "DECODED_XAI_TAGS",
                "the_exported_xai_tag_set_is_exactly_what_the_arms_match",
            ),
            (
                "DECODED_XAI_METHOD",
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
                assert!(
                    src.contains(&format!("fn {test}")),
                    "{name} claims to be pinned by {test}, which its module no longer \
                     declares — the exemption is now an unbacked claim of coverage",
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
