//! This crate's half of the **drift surface** — the hook events we REGISTER,
//! emitted as data for `check_upstream_drift.py`.
//!
//! `pixtuoid-core` emits the other half (what the decoders READ). Two fragments
//! because each crate's names are private to it, and because registration and
//! decoding answer different questions against different upstream documents —
//! see the `*_EVENTS` sharp edge in `SHARP-EDGES.md`.
//! `every_registered_*_event_decodes` is what binds them.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{json, Value};

/// Where the committed fragment lives, relative to the workspace root.
const FRAGMENT: &str = "crates/pixtuoid/drift-surface.json";

/// Set when regenerating: `UPDATE_DRIFT_SURFACE=1 cargo test -p pixtuoid`.
const UPDATE_ENV: &str = "UPDATE_DRIFT_SURFACE";

fn surface() -> Value {
    // BTreeMap so the emitted order is key order, not hash order — the committed
    // file has to be byte-stable or the gate flaps.
    let mut registered: BTreeMap<&str, Value> = BTreeMap::new();
    registered.insert("claude-code", json!(crate::install::claude::EVENTS));
    registered.insert("codewhale", {
        // The only registration set carrying a second column (whether the event
        // is shell-serviceable); the drift watch compares NAMES.
        let names: Vec<&str> = crate::install::codewhale::CODEWHALE_EVENTS
            .iter()
            .map(|(name, _)| *name)
            .collect();
        json!(names)
    });
    registered.insert("codex", json!(crate::install::codex::CODEX_EVENTS));
    registered.insert("cursor", json!(crate::install::cursor::CURSOR_EVENTS));
    registered.insert("hermes", json!(crate::install::hermes::HERMES_EVENTS));
    registered.insert("kimi", json!(crate::install::kimi::KIMI_EVENTS));
    registered.insert("openclaw", json!(crate::install::openclaw::OPENCLAW_EVENTS));
    registered.insert("reasonix", json!(crate::install::reasonix::REASONIX_EVENTS));

    let mut root: BTreeMap<&str, Value> = BTreeMap::new();
    root.insert("registered", json!(registered));
    json!(root)
}

fn fragment_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(FRAGMENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed fragment IS what we register. Regenerate with
    /// `UPDATE_DRIFT_SURFACE=1 cargo test -p pixtuoid`.
    ///
    /// A TEST rather than a CI-only job so it runs on every platform in
    /// `just test` — a stale fragment narrows the watch, which is exactly the
    /// failure that cannot announce itself.
    #[test]
    fn the_committed_fragment_matches_what_we_register() {
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
            "{FRAGMENT} is stale — regenerate with `{UPDATE_ENV}=1 cargo test -p pixtuoid`",
        );
    }

    /// Every registered source in the fragment is a REGISTRY source name, and no
    /// set is empty. Without the roster check a typo'd key emits a row the
    /// watcher would silently never match to a source.
    #[test]
    fn every_emitted_source_is_registered_and_non_empty() {
        let roster: Vec<&str> =
            pixtuoid_core::source::registry::registered_source_names().collect();
        let s = surface();
        let obj = s["registered"]
            .as_object()
            .expect("registered is an object");
        assert!(!obj.is_empty(), "no source emits a registration set");
        for (name, events) in obj {
            assert!(
                roster.contains(&name.as_str()),
                "{name} is not a registry source name: {roster:?}",
            );
            let a = events.as_array().expect("a set");
            assert!(!a.is_empty(), "{name} registers nothing");
            assert!(
                a.iter().all(|x| x.as_str().is_some_and(|s| !s.is_empty())),
                "{name} carries a blank event name",
            );
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
