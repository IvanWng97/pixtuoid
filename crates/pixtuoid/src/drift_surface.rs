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
    registered.insert("grok", json!(crate::install::grok::GROK_EVENTS));
    registered.insert("hermes", json!(crate::install::hermes::HERMES_EVENTS));
    registered.insert("kimi", json!(crate::install::kimi::KIMI_EVENTS));
    registered.insert("openclaw", json!(crate::install::openclaw::OPENCLAW_EVENTS));
    registered.insert("reasonix", json!(crate::install::reasonix::REASONIX_EVENTS));

    // A VALUE, not a name set: the gateway port IS the daemon's runtime identity
    // (`pixtuoid-core/SHARP-EDGES.md`), so a silent upstream bump collapses two
    // live gateways onto one mascot. Read out of the shipped template rather than
    // copied, so the watch compares what actually installs.
    let mut values: BTreeMap<&str, Value> = BTreeMap::new();
    values.insert(
        "openclaw.default_gateway_port",
        json!([openclaw_default_gateway_port()
            .expect("openclaw_plugin.js declares DEFAULT_GATEWAY_PORT")]),
    );

    let mut root: BTreeMap<&str, Value> = BTreeMap::new();
    root.insert("registered", json!(registered));
    root.insert("values", json!(values));
    json!(root)
}

/// The fallback port the bundled OpenClaw plugin ships with, read out of the
/// template itself so the drift watch never compares against a second copy.
fn openclaw_default_gateway_port() -> Option<&'static str> {
    let after = crate::install::openclaw::PLUGIN_TEMPLATE
        .split_once("const DEFAULT_GATEWAY_PORT")?
        .1
        .split_once('=')?
        .1;
    let digits = after.trim_start();
    let end = digits.find(|c: char| !c.is_ascii_digit())?;
    (end > 0).then(|| &digits[..end])
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
            "{FRAGMENT} is stale — regenerate with `just gen-drift-surface`",
        );
    }

    /// The port reader finds the shipped literal, and refuses a shape it cannot
    /// read rather than emitting a plausible wrong answer.
    #[test]
    fn the_gateway_port_is_read_from_the_template_not_copied() {
        let got = openclaw_default_gateway_port().expect("the template declares it");
        assert!(
            crate::install::openclaw::PLUGIN_TEMPLATE
                .contains(&format!("const DEFAULT_GATEWAY_PORT = {got};")),
            "the emitted port {got} is not the literal the template ships",
        );
        assert!(got.chars().all(|c| c.is_ascii_digit()) && !got.is_empty());
    }

    /// Every `*_EVENTS` registration set in `install/` reaches the fragment.
    ///
    /// The other direction — emitted ⊆ roster — is below. This is the one the
    /// rule needs: a hook-registered CLI whose names the watcher never sees has
    /// no upstream watch at all, and every gate stays green because both sides
    /// omit it symmetrically. Derived from the consts themselves, so there is no
    /// exemption list to drift (opencode registers through its TS plugin's
    /// FORWARD set and declares no `*_EVENTS` const, so it is absent by
    /// construction, not by exception).
    #[test]
    fn every_install_events_const_reaches_the_fragment() {
        // Recursive: `src/install/hook_cmd/` already exists, and a registration
        // set declared in a subdirectory would otherwise be invisible to the
        // census that exists to see it.
        fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("the install dir is readable") {
                let path = entry.expect("a readable entry").path();
                if path.is_dir() {
                    rs_files(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let install_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/install");
        let mut paths = Vec::new();
        rs_files(&install_dir, &mut paths);
        let emitter = include_str!("drift_surface.rs");
        let mut found = 0;
        for path in paths {
            // The path RELATIVE to install/, not the file stem: `hook_cmd/mod.rs`
            // must not report itself as `mod.rs`, and the emitter's reference
            // spells the module path anyway.
            let rel = path
                .strip_prefix(&install_dir)
                .unwrap_or(&path)
                .with_extension("");
            let rel = rel
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "::");
            let module = rel.strip_suffix("::mod").unwrap_or(&rel).to_owned();
            let src = std::fs::read_to_string(&path).expect("the module is readable");
            for line in src.lines() {
                let Some((_, rest)) = line.split_once("const ") else {
                    continue;
                };
                let Some((name, _)) = rest.split_once(':') else {
                    continue;
                };
                let name = name.trim();
                if !name.ends_with("EVENTS")
                    || !name.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                {
                    continue;
                }
                found += 1;
                assert!(
                    emitter.contains(&format!("{module}::{name}")),
                    "install/{module} registers hooks as {name}, but surface() never emits \
                     them — check_upstream_drift.py cannot watch a name it never receives, and \
                     a rename would leave the registration inert with nothing to say so. Add a \
                     `registered.insert` here, run `just gen-drift-surface`, then add its \
                     SURFACE_ROWS row in scripts/check_upstream_drift.py.",
                );
            }
        }
        assert!(
            found > 0,
            "the *_EVENTS declaration shape moved — this census reads nothing"
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

    /// Byte-stability across feature unification — the WHY is on
    /// `pixtuoid_core`'s twin of this test, which this deliberately duplicates
    /// rather than share: a 15-line walker is under the extract-a-helper bar,
    /// and each crate's fragment has to be gated where it is emitted.
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
