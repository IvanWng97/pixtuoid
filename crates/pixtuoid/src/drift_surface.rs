//! This crate's half of the **drift surface** — the values our shipped
//! artifacts bake in, emitted as data for `check_upstream_drift.py`.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// The committed fragment, by absolute path so a worktree writes its own.
const FRAGMENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/drift-surface.json");

fn surface() -> Value {
    // BTreeMap so the emitted order is key order, not hash order — the committed
    // file has to be byte-stable or the gate flaps. The values ride the artifacts
    // we install, read out of those templates so the watch compares what installs.
    let mut shipped: BTreeMap<&str, Value> = BTreeMap::new();
    shipped.insert(
        "openclaw.default_gateway_port",
        json!([crate::install::openclaw::default_gateway_port()
            .expect("openclaw_plugin.js declares DEFAULT_GATEWAY_PORT")]),
    );

    let mut root: BTreeMap<&str, Value> = BTreeMap::new();
    root.insert("shipped", json!(shipped));
    json!(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed fragment IS what we ship. Regenerate with
    /// `just gen-drift-surface`.
    ///
    /// A TEST rather than a CI-only job so it runs on every platform in
    /// `just test` — a stale fragment narrows the watch, which is exactly the
    /// failure that cannot announce itself.
    #[test]
    fn the_committed_fragment_matches_what_we_ship() {
        let want = serde_json::to_string_pretty(&surface()).expect("surface serializes") + "\n";
        let committed = snapbox::Data::read_from(
            std::path::Path::new(FRAGMENT),
            Some(snapbox::data::DataFormat::Text),
        );
        snapbox::assert_data_eq!(want, committed.raw());
    }

    /// No group or set is empty: an emptied set would narrow the watch to nothing
    /// while the gate above stayed green.
    #[test]
    fn no_emitted_set_is_empty() {
        let s = surface();
        for (g, entries) in s.as_object().expect("root is an object") {
            let o = entries.as_object().expect("group is an object");
            assert!(!o.is_empty(), "{g} is an empty group");
            for (k, v) in o {
                let ok = match v {
                    Value::Array(a) => {
                        !a.is_empty() && a.iter().all(|x| x.as_str().is_some_and(|s| !s.is_empty()))
                    }
                    _ => false,
                };
                assert!(ok, "{g}.{k} must be a non-empty array of non-empty names");
            }
        }
    }
}
