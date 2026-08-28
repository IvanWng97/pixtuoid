//! This crate's half of the **drift surface** — the names our decoders read,
//! emitted as data for `check_upstream_drift.py`.
//!
//! The watcher is a separate program that cannot call us, and until #942 it
//! REGEX-PARSED this crate's source instead. That fails SILENTLY: rename a
//! const and the parser returns a smaller set, so the watch narrows with
//! nothing to show for it. Emitting makes it a test failure HERE.
//!
//! `crates/pixtuoid` emits the other half — what we REGISTER — because a
//! decode arm with no registration row is a real failure class, not an
//! organisational split.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::source::codex::{
    EM_RESUME, EM_SEARCH, EM_TOKENS, EM_TURN_END, EM_TURN_START, EVENT_MSG, RESPONSE_ITEM,
    RI_RESUME, RI_SEARCH, RI_TOOL_START, TURN_CONTEXT,
};

/// Where the committed fragment lives, relative to the workspace root.
const FRAGMENT: &str = "crates/pixtuoid-core/drift-surface.json";

/// Set when regenerating: `just gen-drift-surface`.
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
        "codex.escalation",
        json!(crate::source::codex::DECODED_ESCALATION),
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
        "omp.title_fields",
        json!(crate::source::omp::DECODED_TITLE_FIELDS),
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
        "opencode.part_statuses",
        json!(crate::source::opencode::DECODED_PART_STATUSES),
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
    /// `just gen-drift-surface`.
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
            // ANY `"` in the head, not just a leading or `(`-prefixed one: the
            // shape that escaped was a guard, `g if g == "wire_name" =>`, which
            // dispatches on a name the surface never carries while `armed` stays
            // equal to the export because the token is not SCREAMING_SNAKE.
            assert!(
                !head.contains('"'),
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

    /// Every wire literal a decoder equality-compares is carried by the drift
    /// surface or exempted below with its adjudicated reason (#943's gate).
    ///
    /// Three accepted loosenesses, adjudicated on #943: fragment membership is
    /// global across sets, so a cross-source spelling collision passes; a
    /// literal behind a non-exported const escapes the scan; and the scan is
    /// `==`/`!=`-shaped only — a MATCH ARM pattern, a `.starts_with` prefix
    /// gate, or a `matches!` is outside its population. Export such
    /// vocabularies instead (opencode's part statuses are the precedent).
    /// Stale exemptions fail the OTHER direction.
    #[test]
    fn every_equality_compared_wire_literal_is_accounted_for() {
        // (file stem, literal, why no upstream watch is owed)
        const EXEMPT: &[(&str, &str, &str)] = &[
            (
                "antigravity",
                "PLANNER_RESPONSE",
                "capture-verified step type; a rename lands in the \
                 non-PLANNER_RESPONSE + tool_calls breadcrumb (antigravity.rs)",
            ),
            (
                "antigravity",
                "ask_permission",
                "unverified reverse-engineered tool name (antigravity.rs comment); \
                 no watchable upstream, silent on rename — accepted",
            ),
            ("antigravity", "ask_question", "same as ask_permission"),
            (
                "admit",
                "jsonl",
                "file-extension guard; resolver/path axis (#880)",
            ),
            (
                "cc_probe",
                "json",
                "file-extension guard; resolver/path axis (#880)",
            ),
            (
                "cc_probe",
                "projects",
                "dirname guard; resolver/path axis (#880)",
            ),
            (
                "claude_code",
                "<synthetic>",
                "CC's placeholder model value; a rename degrades to a cosmetic \
                 flame label until the next real model string arrives",
            ),
            (
                "claude_code",
                "tool_use",
                "Messages-API block type; no CC doc declares it apart from \
                 `tool_use_id`, and the hook transport carries the same activity \
                 — headless-only degradation, corpus-census detectable",
            ),
            ("claude_code", "tool_result", "same as tool_use"),
            (
                "antigravity",
                "USER_INPUT",
                "no watchable upstream doc; the decoder breadcrumbs unknown \
                 vocabulary (the runtime detector)",
            ),
            ("antigravity", "CONVERSATION_HISTORY", "same as USER_INPUT"),
            (
                "claude_code",
                "attachment",
                "undocumented transcript surface; its marker KINDS are \
                 appearance-watched via CC_LIFECYCLE_SURFACE_MARKERS",
            ),
            (
                "copilot",
                "task",
                "name-only BY DESIGN (subagent_type is spoofable, see \
                 copilot_tool_detail) — and unwatchable: the schema types \
                 toolName as a free string and never declares tool-name values",
            ),
            (
                "cursor",
                "Task",
                "detection is semantic (subagent_type, capture-verified), so a \
                 rename degrades to a Generic tool, never a missed delegation",
            ),
            (
                "grok",
                "spawn_subagent",
                "semantic subagent_type fallback exists on the same arm",
            ),
            (
                "omp",
                "default",
                "resolver/path axis, deliberately unwatched (#880)",
            ),
            (
                "omp",
                "sessions",
                "resolver/path axis, deliberately unwatched (#880)",
            ),
            (
                "omp",
                "task",
                "children register via PATH NESTING regardless (module doc); the \
                 name only styles the parent's pose",
            ),
            (
                "opencode",
                "tool",
                "part-type inside the watched `message.part.updated`; a rename is \
                 #943-class value drift, accepted residual",
            ),
        ];

        let mut carried: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for rel in ["../pixtuoid-core", "../pixtuoid"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join(rel.trim_start_matches("../"))
                .join("drift-surface.json");
            let frag: Value = serde_json::from_str(
                &std::fs::read_to_string(&path).expect("committed fragment is readable"),
            )
            .expect("fragment parses");
            fn walk(v: &Value, out: &mut std::collections::BTreeSet<String>) {
                match v {
                    Value::String(s) => {
                        out.insert(s.clone());
                    }
                    Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
                    Value::Object(o) => o.values().for_each(|x| walk(x, out)),
                    _ => {}
                }
            }
            walk(&frag, &mut carried);
        }
        let sources: std::collections::BTreeSet<&str> =
            crate::source::registry::registered_source_names().collect();

        fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("the source dir is readable") {
                let path = entry.expect("a readable entry").path();
                if path.is_dir() {
                    rs_files(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let mut paths = Vec::new();
        rs_files(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/source"),
            &mut paths,
        );

        let mut found: std::collections::BTreeSet<(String, String)> =
            std::collections::BTreeSet::new();
        let mut unaccounted: Vec<String> = Vec::new();
        for path in paths {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .expect("a utf-8 stem")
                .to_owned();
            // mod/native/tests are the runtime+resolver axis, deliberately
            // unwatched (#880); drift/registry are the breadcrumb infra itself.
            if ["mod", "native", "tests", "drift", "registry"].contains(&stem.as_str()) {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("the module is readable");
            let prod = src
                .split("#[cfg(test)]\nmod ")
                .next()
                .expect("split never yields zero parts");
            for line in prod.lines() {
                // Comments first: a doc example quoting `== "x"` is not a read.
                let code = line.split("//").next().unwrap_or("");
                for pat in ["== Some(\"", "== \"", "!= Some(\"", "!= \""] {
                    for (i, _) in code.match_indices(pat) {
                        let rest = &code[i + pat.len()..];
                        let Some(end) = rest.find('\"') else { continue };
                        let lit = &rest[..end];
                        found.insert((stem.clone(), lit.to_owned()));
                        let ok = carried.contains(lit)
                            || sources.contains(lit)
                            || lit.starts_with('.')
                            || lit.ends_with(".jsonl")
                            || EXEMPT.iter().any(|(f, l, _)| *f == stem && *l == lit);
                        if !ok {
                            unaccounted.push(format!("{stem}.rs: == \"{lit}\""));
                        }
                    }
                }
            }
        }
        assert!(
            unaccounted.is_empty(),
            "a decoder equality-compares wire literals the drift surface never \
             carries — export them (a `DECODED_*` const + a fragment row), \
             breadcrumb the miss, or exempt them here with the adjudicated \
             reason:\n  {}",
            unaccounted.join("\n  "),
        );
        for (file, lit, _) in EXEMPT {
            assert!(
                found.contains(&((*file).to_owned(), (*lit).to_owned())),
                "the exemption for {file}.rs == \"{lit}\" outlived the \
                 comparison it excused — delete the row",
            );
        }
    }
}
