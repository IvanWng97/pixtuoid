//! Pins the site manifest's per-CLI `badge_color` hexes to the NORMAL theme's
//! `SourceColors` — the palette the site's live office actually renders with
//! (`pixtuoid-web` constructs its `Office` on `ALL_THEMES[0]`, i.e. normal).
//! The site chips are a cross-boundary COPY of these hues; copies are pinned,
//! not trusted (workspace CLAUDE.md "no magic numbers" rule 2) — the
//! `label_prefix` half of the same manifest is pinned by pixtuoid-core's
//! `supported_sources_manifest.rs`.
//!
//! Runtime read for the same reason as that core test: `include_str!` of a
//! path outside the crate breaks `cargo publish`'s verify. Workspace-only
//! test, excluded from the published package (`Cargo.toml` `exclude`).

use pixtuoid_scene::theme::theme_by_name;

const MANIFEST_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../site/src/sources.json");

#[test]
fn supported_badge_colors_match_the_normal_theme_source_hues() {
    let text = std::fs::read_to_string(MANIFEST_PATH)
        .unwrap_or_else(|e| panic!("read {MANIFEST_PATH}: {e}"));
    let rows = serde_json::from_str::<serde_json::Value>(&text)
        .expect("sources.json is valid JSON")
        .as_array()
        .expect("sources.json is a JSON array")
        .clone();
    let normal = theme_by_name("normal").expect("normal theme registered");

    let mut checked = 0usize;
    for row in rows.iter().filter(|r| r["status"] == "supported") {
        let badge = row["badge"]
            .as_str()
            .unwrap_or_else(|| panic!("supported row has no string `badge`: {row}"));
        let prefix = badge.trim_end_matches('\u{00b7}');
        let rgb = normal
            .source
            .by_prefix(prefix)
            .unwrap_or_else(|| panic!("no NORMAL source hue for prefix {prefix:?}"));
        let want = format!("#{:02x}{:02x}{:02x}", rgb.r, rgb.g, rgb.b);
        let got = row["badge_color"]
            .as_str()
            .unwrap_or_else(|| panic!("supported row {prefix:?} has no `badge_color`"));
        assert_eq!(
            got, want,
            "sources.json badge_color for {prefix:?} drifted from theme::NORMAL.source"
        );
        checked += 1;
    }
    assert!(
        checked > 0,
        "no supported rows found — manifest read failed?"
    );
}
