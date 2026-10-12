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
}
