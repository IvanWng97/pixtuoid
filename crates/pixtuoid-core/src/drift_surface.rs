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
        "codex.rollout_outers",
        json!(crate::source::codex::DECODED_OUTERS),
    );
    decoded.insert(
        "codex.event_msg",
        json!(crate::source::codex::decoded_event_msg()),
    );
    decoded.insert(
        "codex.response_item",
        json!(crate::source::codex::decoded_response_item()),
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
        "omp.entry_types",
        json!(crate::source::omp::DECODED_ENTRY_TYPES),
    );
    decoded.insert(
        "opencode.hook_events",
        json!(crate::source::opencode::DECODED_EVENTS),
    );

    let mut values: BTreeMap<&str, Value> = BTreeMap::new();
    values.insert(
        "grok.xai_session_update_method",
        json!(crate::source::grok::XAI_SESSION_UPDATE_METHOD),
    );

    // The per-source staleness row: what version we last VERIFIED this decoder
    // and its fixtures against, and where that CLI publishes releases. This is
    // what lets a watcher say how old our evidence is WITHOUT scraping the
    // vendor's private file layout.
    let sources: BTreeMap<&str, Value> = crate::source::registry::REGISTRY
        .iter()
        .map(|d| {
            let mut feed: BTreeMap<&str, Value> = BTreeMap::new();
            match d.release_feed {
                crate::source::registry::ReleaseFeed::GitHub { repo, version_in } => {
                    feed.insert("kind", json!("github"));
                    feed.insert("repo", json!(repo));
                    feed.insert(
                        "version_in",
                        json!(match version_in {
                            crate::source::registry::ReleaseField::Tag => "tag",
                            crate::source::registry::ReleaseField::Name => "name",
                        }),
                    );
                }
                crate::source::registry::ReleaseFeed::Npm(pkg) => {
                    feed.insert("kind", json!("npm"));
                    feed.insert("package", json!(pkg));
                }
                crate::source::registry::ReleaseFeed::None => {
                    feed.insert("kind", json!("none"));
                }
            }
            let mut row: BTreeMap<&str, Value> = BTreeMap::new();
            row.insert("verified_version", json!(d.verified_version));
            row.insert("release_feed", json!(feed));
            (d.name, json!(row))
        })
        .collect();

    // EVERY object goes through a BTreeMap, never a `json!` object literal: the
    // workspace enables serde_json's `preserve_order` (pixtuoid does), and
    // feature unification hands it to this crate too — so a `json!` map is
    // sorted under `cargo test -p` and insertion-ordered under `just test`. The
    // committed file must be byte-stable across both or the gate flaps.
    let mut root: BTreeMap<&str, Value> = BTreeMap::new();
    root.insert("decoded", json!(decoded));
    root.insert("values", json!(values));
    root.insert("sources", json!(sources));
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
        for (group, entries) in [("decoded", &s["decoded"]), ("values", &s["values"])] {
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
    /// serde_json's `preserve_order`, so an object built by `json!({…})` is
    /// sorted under `cargo test -p` and insertion-ordered under `cargo nextest
    /// run --workspace`. That difference reached the committed file and made the
    /// gate above pass one way and fail the other; sorted keys everywhere is
    /// what makes it a gate rather than a coin flip.
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
