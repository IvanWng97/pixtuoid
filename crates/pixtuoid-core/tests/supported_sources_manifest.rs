//! Pins the marketing manifest's "supported" set to the code's
//! `REGISTERED_SOURCES`.
//!
//! `site/src/sources.json` single-sources the README "Supported Tools" glimpse
//! (`site/scripts/gen-readme.mjs`) AND the site's full tool × OS support matrix
//! (`SupportedTools.astro`). That rendering is *parity*; THIS test is *truth*:
//! the manifest can never claim `"status": "supported"` for a source that isn't
//! actually wired, and registering a new source forces a manifest row — the same
//! "registration is not coverage" guarantee the `registry_bridge_tests` give the
//! `SourceDescriptor` table, extended to the public-facing list.
//!
//! Runtime read (NOT `include_str!`): `include_str!` would make `cargo publish`'s
//! compile-only verify choke on a path outside the crate package. The runtime
//! read compiles cleanly there — but `cargo test` on an EXTRACTED .crate would
//! still panic (no workspace tree), so this file is in pixtuoid-core's `exclude`
//! list (alongside `socket_path_parity.rs`) to keep `cargo test` on the .crate
//! clean. Workspace-only test, workspace-only file.

use std::collections::BTreeSet;

use pixtuoid_core::source::REGISTERED_SOURCES;

const MANIFEST_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../site/src/sources.json");

fn manifest() -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(MANIFEST_PATH)
        .unwrap_or_else(|e| panic!("read {MANIFEST_PATH}: {e}"));
    serde_json::from_str::<serde_json::Value>(&text)
        .expect("sources.json is valid JSON")
        .as_array()
        .expect("sources.json is a JSON array")
        .clone()
}

fn str_field<'a>(s: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    s.get(key).and_then(|v| v.as_str())
}

/// The load-bearing invariant: manifest `supported` ⇔ code `REGISTERED_SOURCES`.
#[test]
fn manifest_supported_set_matches_registered_sources() {
    let manifest_supported: BTreeSet<String> = manifest()
        .iter()
        .filter(|s| str_field(s, "status") == Some("supported"))
        .map(|s| {
            str_field(s, "id")
                .unwrap_or_else(|| {
                    panic!("a `supported` source in sources.json has no string `id`: {s}")
                })
                .to_string()
        })
        .collect();

    let registered: BTreeSet<String> = REGISTERED_SOURCES
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    assert_eq!(
        manifest_supported,
        registered,
        "site/src/sources.json `supported` set must EXACTLY match REGISTERED_SOURCES.\n  \
         claims supported but NOT wired: {:?}\n  \
         wired but NOT 'supported' in the manifest: {:?}\n  \
         Fix: edit site/src/sources.json (then `just gen-readme`).",
        manifest_supported
            .difference(&registered)
            .collect::<Vec<_>>(),
        registered
            .difference(&manifest_supported)
            .collect::<Vec<_>>(),
    );
}

/// Shape guard so the site renderer + gen-readme can trust every row: required
/// fields present, statuses/platform values from a closed set, planned rows
/// carry no `id` (they aren't wired yet).
#[test]
fn manifest_rows_are_well_formed() {
    const OSES: [&str; 3] = ["macos", "linux", "windows"];
    for s in manifest() {
        let name = str_field(&s, "name").unwrap_or_else(|| panic!("row missing `name`: {s}"));
        assert!(
            str_field(&s, "url").is_some_and(|u| u.starts_with("http")),
            "{name}: `url` must be an http(s) link"
        );
        let status = str_field(&s, "status").unwrap_or_else(|| panic!("{name}: missing `status`"));
        assert!(
            matches!(status, "supported" | "planned"),
            "{name}: `status` must be supported|planned, got {status:?}"
        );
        assert!(
            s.get("featured").is_some_and(|v| v.is_boolean()),
            "{name}: `featured` must be a bool"
        );
        assert!(
            str_field(&s, "transport").is_some(),
            "{name}: missing `transport`"
        );

        let platforms = s
            .get("platforms")
            .and_then(|v| v.as_object())
            .unwrap_or_else(|| panic!("{name}: missing `platforms` object"));
        for os in OSES {
            let v = platforms
                .get(os)
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("{name}: `platforms.{os}` missing/not a string"));
            assert!(
                matches!(v, "yes" | "experimental" | "planned" | "no"),
                "{name}: `platforms.{os}` must be yes|experimental|planned|no, got {v:?}"
            );
        }

        if status == "planned" {
            assert!(
                s.get("id").map_or(true, |v| v.is_null()),
                "{name}: a `planned` source must not carry an `id` (it isn't wired yet)"
            );
        }
    }
}
