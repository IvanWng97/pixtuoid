//! Pins `scripts/readme-palette.json` — the office colours the README's pixel
//! art is painted with — to the theme each variant names, by struct access,
//! and `scripts/readme-pack-ramps.json` to the pack's ramps as [`Rgb::ramp`]
//! resolves them. The renderers are Python and can only copy; this is the
//! copies' guard, the same shape as `site_badge_colors.rs`.
//!
//! Reads the JSON at RUNTIME because `include_str!` of a path outside the
//! crate fails `cargo test` on the extracted .crate (no workspace tree).
//! Workspace-only test, excluded from the published package (`Cargo.toml`
//! `exclude`).

use pixtuoid_core::sprite::Rgb;
use pixtuoid_scene::theme::{Theme, theme_by_name};

const PALETTE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/readme-palette.json"
);
const RAMPS_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/readme-pack-ramps.json"
);
const PACK_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/sprites/default/pack.toml");

fn hex(rgb: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb.r, rgb.g, rgb.b)
}

/// Every field the renderer paints with, and the theme role it copies.
fn roles(t: &Theme) -> [(&'static str, Rgb); 19] {
    [
        ("wall", t.surface.wall),
        ("trim", t.surface.wall_trim),
        ("carpet_light", t.surface.carpet_light),
        ("carpet_dark", t.surface.carpet_dark),
        ("title", t.ui.tooltip_title),
        ("text", t.ui.tooltip_text),
        ("star", t.lighting.desk_lamp),
        ("window_frame", t.surface.window_frame),
        ("sky_top", t.lighting.night_sky_b),
        ("sky_horizon", t.lighting.night_sky_a),
        ("building_dark", t.office.building_dark),
        ("building_light", t.office.building_light),
        ("city_dark_window", t.office.city_dark_window),
        ("city_lit_a", t.office.city_lit_windows[0]),
        ("city_lit_b", t.office.city_lit_windows[1]),
        ("city_lit_c", t.office.city_lit_windows[2]),
        ("moon", t.lighting.moon_core),
        ("neon_panel", t.office.neon_panel_bg),
        ("neon_brand", t.ui.neon_brand),
    ]
}

#[test]
fn readme_chart_palette_matches_the_named_themes_verbatim() {
    let text = std::fs::read_to_string(PALETTE_PATH)
        .unwrap_or_else(|e| panic!("read {PALETTE_PATH}: {e}"));
    let variants = serde_json::from_str::<serde_json::Value>(&text)
        .expect("readme-palette.json is valid JSON");
    let variants = variants
        .as_object()
        .expect("readme-palette.json is a JSON object keyed by README variant");

    let mut checked = 0usize;
    for (variant, row) in variants {
        let name = row["theme"]
            .as_str()
            .unwrap_or_else(|| panic!("variant {variant:?} has no string `theme`"));
        let theme = theme_by_name(name)
            .unwrap_or_else(|| panic!("variant {variant:?} names unregistered theme {name:?}"));
        for (field, rgb) in roles(theme) {
            let got = row[field]
                .as_str()
                .unwrap_or_else(|| panic!("variant {variant:?} has no `{field}`"));
            assert_eq!(
                got,
                hex(rgb),
                "readme-palette.json {variant}.{field} drifted from theme {name:?}"
            );
            checked += 1;
        }
        let extra: Vec<_> = row
            .as_object()
            .expect("variant row is an object")
            .keys()
            .filter(|k| *k != "theme" && !roles(theme).iter().any(|(f, _)| f == k))
            .collect();
        assert!(
            extra.is_empty(),
            "variant {variant:?} carries fields no theme role backs: {extra:?}"
        );
    }
    assert!(checked > 0, "no variants found — palette read failed?");
}

fn parse_hex(s: &str) -> Rgb {
    let channel = |i: usize| {
        u8::from_str_radix(&s[i..i + 2], 16).unwrap_or_else(|e| panic!("{s:?} is not #rrggbb: {e}"))
    };
    Rgb {
        r: channel(1),
        g: channel(3),
        b: channel(5),
    }
}

#[test]
fn readme_pack_ramps_match_the_bundled_pack() {
    let pack: toml_edit::DocumentMut = std::fs::read_to_string(PACK_PATH)
        .unwrap_or_else(|e| panic!("read {PACK_PATH}: {e}"))
        .parse()
        .expect("pack.toml parses");
    let palette = &pack["palette"];
    let expected: std::collections::BTreeMap<String, String> = pack["ramps"]
        .as_table()
        .expect("pack.toml has a [ramps] table")
        .iter()
        .map(|(key, ramp)| {
            let of = ramp["of"]
                .as_str()
                .unwrap_or_else(|| panic!("ramp {key:?} has no `of`"));
            let level = ramp["level"]
                .as_integer()
                .unwrap_or_else(|| panic!("ramp {key:?} has no `level`"));
            let base = palette[of]
                .as_str()
                .unwrap_or_else(|| panic!("ramp {key:?} is of {of:?}, which has no colour"));
            let level = i8::try_from(level)
                .unwrap_or_else(|_| panic!("ramp {key:?}'s level {level} is out of range"));
            (key.to_owned(), hex(parse_hex(base).ramp(level)))
        })
        .collect();
    let got: std::collections::BTreeMap<String, String> = std::fs::read_to_string(RAMPS_PATH)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    assert_eq!(
        got,
        expected,
        "readme-pack-ramps.json drifted from pack.toml's [ramps]; it should read:\n{}",
        serde_json::to_string_pretty(&expected).expect("a map serializes")
    );
}
