//! This crate's half of the **drift surface** — the names our decoders actually
//! read, emitted as data for `check_upstream_drift.py`.
//!
//! It exists because the watcher is a separate program that cannot call us, and
//! the alternative it used for years was to REGEX-PARSE this crate's source. A
//! scraped `match` arm goes stale silently: rename a const and the parser
//! quietly returns a smaller set, so the watch narrows without anything failing.
//! Emitting instead makes that a test failure here, in the crate that owns the
//! names.
//!
//! The names stay `pub(crate)` and `#[cfg(test)]`: nothing about this is a
//! published API, and the shipped crate must not be able to read a second copy
//! of a vocabulary it dispatches on.
//!
//! `crates/pixtuoid` emits the other half — its `install/` registration sets are
//! private to that crate for the same reason.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{json, Value};

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
        "copilot.kinds",
        json!(crate::source::copilot::DECODED_KINDS),
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
            "{FRAGMENT} is stale — regenerate with `{UPDATE_ENV}=1 cargo test -p pixtuoid-core`",
        );
    }

    /// Every value in the fragment is a non-empty name, and no set is empty: an
    /// emptied set would narrow the watch to nothing while the gate above stayed
    /// green, because it compares the file to the same empty set.
    #[test]
    fn no_emitted_set_is_empty() {
        let s = surface();
        for (group, entries) in [("decoded", &s["decoded"])] {
            let obj = entries.as_object().expect("group is an object");
            assert!(!obj.is_empty(), "{group} is empty");
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
}
