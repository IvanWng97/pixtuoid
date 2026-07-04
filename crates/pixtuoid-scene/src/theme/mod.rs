mod catppuccin;
mod cyberpunk;
mod dracula;
mod gruvbox;
mod normal;
mod tokyo_night;

use pixtuoid_core::sprite::Rgb;

pub use catppuccin::CATPPUCCIN;
pub use cyberpunk::CYBERPUNK;
pub use dracula::DRACULA;
pub use gruvbox::GRUVBOX;
pub use normal::NORMAL;
pub use tokyo_night::TOKYO_NIGHT;

/// Light vs Dark classification — drives effects that only look right on
/// one or the other (e.g. ceiling halos read as soft glow on dark themes
/// but as dirt smears on light themes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeKind {
    Light,
    Dark,
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: &'static str,
    pub kind: ThemeKind,
    pub surface: SurfaceColors,
    pub office: OfficeColors,
    pub lighting: LightingColors,
    pub furniture: FurnitureColors,
    pub effects: EffectColors,
    pub ui: UiColors,
    pub tool_glow: ToolGlowColors,
    pub appliance: ApplianceColors,
    pub source: SourceColors,
}

#[derive(Debug, Clone)]
pub struct SurfaceColors {
    pub wall: Rgb,
    pub wall_trim: Rgb,
    pub baseboard: Rgb,
    pub carpet_base: Rgb,
    pub carpet_light: Rgb,
    pub carpet_dark: Rgb,
    pub window_frame: Rgb,
    pub bg_fallback: Rgb,
}

#[derive(Debug, Clone)]
pub struct OfficeColors {
    pub room_wall_body: Rgb,
    pub room_wall_trim_light: Rgb,
    pub room_wall_trim_dark: Rgb,
    pub cubicle_divider: Rgb,
    pub runner_base: Rgb,
    pub runner_stripe: Rgb,
    pub runner_edge: Rgb,
    pub neon_panel_bg: Rgb,
    pub neon_frame_base: Rgb,
    pub building_dark: Rgb,
    pub building_light: Rgb,
    pub city_lit_windows: [Rgb; 3],
    pub city_dark_window: Rgb,
    pub clock_rim: Rgb,
    pub clock_face: Rgb,
    pub clock_hand: Rgb,
    pub shadow: Rgb,
}

#[derive(Debug, Clone)]
pub struct LightingColors {
    pub day_sky_a: Rgb,
    pub day_sky_b: Rgb,
    pub night_sky_a: Rgb,
    pub night_sky_b: Rgb,
    pub twilight_a: Rgb,
    pub twilight_b: Rgb,
    pub sun_spill: Rgb,
    pub ceiling_pool: Rgb,
    pub floor_lamp_halo: Rgb,
    pub night_tint: Rgb,
    /// The sun disc's core color (window-wall celestial body, `pixel_painter::background::celestial::compute_disc`).
    /// The soft halo ring reuses this SAME hue at lower alpha (no separate glow color) — must read
    /// warm (see `sun_and_moon_read_warm_and_cool_for_every_theme`).
    pub sun_core: Rgb,
    /// The moon disc's core color (lit side; the dark limb is the fixed `MOON_SHADOW` in
    /// `background/celestial.rs`, not per-theme). Must read cool.
    pub moon_core: Rgb,
}

#[derive(Debug, Clone)]
pub struct FurnitureColors {
    pub wood_top: Rgb,
    pub wood_trim: Rgb,
    pub rug_field: Rgb,
    pub rug_trim: Rgb,
    pub rug_accent: Rgb,
    pub magazine: Rgb,
    pub magazine_trim: Rgb,
    pub chair_seat: Rgb,
    pub chair_trim: Rgb,
    pub coffee_cup: Rgb,
    pub coffee_cup_shadow: Rgb,
}

#[derive(Debug, Clone)]
pub struct EffectColors {
    pub monitor_frame_lit: Rgb,
    pub sleep_z: Rgb,
    pub coffee_steam: Rgb,
    pub walking_dust: Rgb,
    pub waiting_bubble: Rgb,
}

#[derive(Debug, Clone)]
pub struct ToolGlowColors {
    pub edit: Rgb,
    pub read: Rgb,
    pub bash: Rgb,
    pub agent: Rgb,
    pub grep: Rgb,
    pub default: Rgb,
}

#[derive(Debug, Clone)]
pub struct UiColors {
    pub label_active: Rgb,
    pub label_waiting: Rgb,
    pub label_idle: Rgb,
    pub label_exiting: Rgb,
    pub tooltip_bg: Rgb,
    pub tooltip_title: Rgb,
    pub tooltip_text: Rgb,
    pub tooltip_dim: Rgb,
    pub neon_brand: Rgb,
    pub neon_star: Rgb,
}

/// Corridor appliance colors (vending machine, printer, coat rack). These were
/// hardcoded RGB literals in `pixel_painter/drawable.rs`, so the appliances
/// rendered with the NORMAL theme's palette on every theme — clashing on the
/// dark/neon/pastel ones. Each theme now supplies its own harmonized set.
#[derive(Debug, Clone)]
pub struct ApplianceColors {
    /// Vending machine chassis (the dark box body).
    pub vending_body: Rgb,
    /// Vending front sign / accent strip — the theme's signature accent.
    pub vending_panel: Rgb,
    /// Four distinct drink-bottle colors behind the glass.
    pub vending_drinks: [Rgb; 4],
    /// Warm small detail (coin-slot trim).
    pub vending_trim: Rgb,
    /// Darkest recess / slot.
    pub vending_dark: Rgb,
    /// Printer chassis — a light neutral.
    pub printer_body: Rgb,
    /// Printer lid / top — darker.
    pub printer_top: Rgb,
    /// Scanner glass — a cool tint.
    pub printer_glass: Rgb,
    /// Paper stack — near-white.
    pub printer_paper: Rgb,
    /// Output tray — mid neutral.
    pub printer_tray: Rgb,
    /// Three hanging coats on the coat rack.
    pub coats: [Rgb; 3],
}

/// Per-source badge hues. One color per registered source — the 9 agent CLIs +
/// the OpenClaw daemon (`all()` returns `[Rgb; 10]`, count-pinned to
/// `REGISTERED_SOURCES` by `source_colors_cover_every_registered_source`) — drawn
/// as a leading `[xx]` badge in the agent-dashboard popup (agents only) and the
/// Sources panel (all sources, incl. the daemon). Each theme supplies its own so
/// the badge harmonizes with the palette and stays legible on `tooltip_bg`
/// (guarded by `source_badges_legible_for_every_theme`).
#[derive(Debug, Clone)]
pub struct SourceColors {
    pub claude_code: Rgb,
    pub codex: Rgb,
    pub reasonix: Rgb,
    pub antigravity: Rgb,
    pub codewhale: Rgb,
    pub opencode: Rgb,
    pub copilot: Rgb,
    pub cursor: Rgb,
    pub openclaw: Rgb,
    pub hermes: Rgb,
}

impl SourceColors {
    /// All badge hues in declaration order. The ONE enumeration the legibility
    /// guard and the count-pin test share, so adding a source forces a new field
    /// HERE (caught by `source_colors_cover_every_registered_source`) instead of
    /// silently escaping the per-theme distinctness check.
    pub fn all(&self) -> [Rgb; 10] {
        [
            self.claude_code,
            self.codex,
            self.reasonix,
            self.antigravity,
            self.codewhale,
            self.opencode,
            self.copilot,
            self.cursor,
            self.openclaw,
            self.hermes,
        ]
    }

    /// Badge hue for a source's 2-char label prefix (`SourceDescriptor::label_prefix`
    /// in `pixtuoid_core::source::registry`), or `None` for an unknown prefix. The
    /// painters (dashboard / connection) resolve a badge color from the prefix
    /// without name-matching each source inline. The accepted prefixes are the
    /// registry's authoritative `SourceDescriptor::label_prefix` values — one arm
    /// per registered source (kept in lockstep by the badge-coverage guards).
    pub fn by_prefix(&self, prefix: &str) -> Option<Rgb> {
        Some(match prefix {
            "cc" => self.claude_code,
            "cx" => self.codex,
            "rx" => self.reasonix,
            "ag" => self.antigravity,
            "cw" => self.codewhale,
            "oc" => self.opencode,
            "cp" => self.copilot,
            "cu" => self.cursor,
            "ok" => self.openclaw,
            "hm" => self.hermes,
            _ => return None,
        })
    }
}

pub static ALL_THEMES: &[&Theme] = &[
    &NORMAL,
    &CYBERPUNK,
    &DRACULA,
    &TOKYO_NIGHT,
    &CATPPUCCIN,
    &GRUVBOX,
];

pub fn theme_by_name(name: &str) -> Option<&'static Theme> {
    ALL_THEMES.iter().find(|t| t.name == name).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_themes_resolve_by_name() {
        for t in ALL_THEMES {
            assert!(
                theme_by_name(t.name).is_some(),
                "theme '{}' not found",
                t.name
            );
        }
    }

    #[test]
    fn unknown_theme_returns_none() {
        assert!(theme_by_name("doesnotexist").is_none());
    }

    #[test]
    fn theme_gallery_manifest_matches_all_themes() {
        // site/src/themes.json drives the site's theme switcher + the gen-media
        // render loop; ALL_THEMES drives what `--theme` actually accepts. Site CI
        // never runs the binary, so this test is the bridge (same pattern as
        // `weather_gallery_manifest_matches_the_weather_enum`). Set equality, not
        // order: the manifest's order is a site presentation choice (`featured`).
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../site/src/themes.json");
        let json = match std::fs::read_to_string(path) {
            Ok(s) => s,
            // crates.io-packaged test runs don't ship the repo's site/ tree.
            Err(_) => {
                eprintln!("skipping: {path} not present (packaged build)");
                return;
            }
        };
        let manifest: Vec<serde_json::Value> =
            serde_json::from_str(&json).expect("themes.json parses");
        let mut ids: Vec<&str> = manifest
            .iter()
            .map(|t| t["id"].as_str().expect("themes.json entry has a string id"))
            .collect();
        let mut names: Vec<&str> = ALL_THEMES.iter().map(|t| t.name).collect();
        ids.sort_unstable();
        names.sort_unstable();
        assert_eq!(
            ids, names,
            "site/src/themes.json ids must match ALL_THEMES names — update the \
             manifest + run `just gen-media` when the registry changes"
        );
    }

    #[test]
    fn dark_themes_marked_dark() {
        assert_eq!(CYBERPUNK.kind, ThemeKind::Dark);
        assert_eq!(DRACULA.kind, ThemeKind::Dark);
        assert_eq!(TOKYO_NIGHT.kind, ThemeKind::Dark);
        assert_eq!(GRUVBOX.kind, ThemeKind::Dark);
        assert_eq!(CATPPUCCIN.kind, ThemeKind::Dark);
    }

    #[test]
    fn light_themes_marked_light() {
        assert_eq!(NORMAL.kind, ThemeKind::Light);
    }

    // The window-wall celestial disc (Task 7) must read as a WARM sun and a
    // COOL moon on every theme, or the day/night disc stops selling its
    // identity regardless of how well it otherwise matches the palette.
    #[test]
    fn sun_and_moon_read_warm_and_cool_for_every_theme() {
        for t in ALL_THEMES {
            let l = &t.lighting;
            assert!(
                l.sun_core.r > l.sun_core.b,
                "{}: sun_core {:?} should read warm (r > b)",
                t.name,
                l.sun_core
            );
            assert!(
                l.moon_core.b > l.moon_core.r,
                "{}: moon_core {:?} should read cool (b > r)",
                t.name,
                l.moon_core
            );
        }
    }

    // Every theme's appliance palette must keep the appliances LEGIBLE — the
    // bug was a hardcoded normal-theme set on all themes, so this guards both
    // that each theme supplies its own AND that the supplied set reads right.
    #[test]
    fn appliance_palette_is_legible_for_every_theme() {
        fn lum(c: Rgb) -> u32 {
            c.r as u32 + c.g as u32 + c.b as u32
        }
        for t in ALL_THEMES {
            let a = &t.appliance;
            // Printer: paper is the lightest, the lid/top the darkest — so the
            // scanner + paper read against the chassis in every theme.
            assert!(
                lum(a.printer_paper) > lum(a.printer_body)
                    && lum(a.printer_body) > lum(a.printer_top),
                "{}: printer must layer paper > body > top by luminance",
                t.name
            );
            // Vending: the accent panel + each drink must be visible against the
            // dark chassis (not collapse into it).
            assert_ne!(
                a.vending_panel, a.vending_body,
                "{}: vending panel invisible",
                t.name
            );
            for (i, d) in a.vending_drinks.iter().enumerate() {
                assert_ne!(
                    *d, a.vending_body,
                    "{}: drink {i} invisible on body",
                    t.name
                );
            }
            // The chassis is darker than its brightest drink (the box reads as a
            // box, the bottles pop).
            let brightest_drink = a.vending_drinks.iter().map(|c| lum(*c)).max().unwrap();
            assert!(
                lum(a.vending_body) < brightest_drink,
                "{}: vending body should be darker than its drinks",
                t.name
            );
        }
    }

    // Every theme's per-CLI badge palette must read on the popup bg (tooltip_bg)
    // and be mutually distinguishable, so a glance tells cc from cx from rx.
    #[test]
    fn source_badges_legible_for_every_theme() {
        fn lum(c: Rgb) -> u32 {
            c.r as u32 + c.g as u32 + c.b as u32
        }
        // Per-channel sum-of-abs-diff. Distinct from `lum` on purpose: two hues
        // can share a luminance yet read as different colors (catppuccin's sky
        // and teal were lum 592 vs 587 — a lum-only floor would miss them), so
        // mutual distinguishability is a Manhattan-distance question, not a
        // brightness one.
        fn manhattan(a: Rgb, b: Rgb) -> u32 {
            (a.r as u32).abs_diff(b.r as u32)
                + (a.g as u32).abs_diff(b.g as u32)
                + (a.b as u32).abs_diff(b.b as u32)
        }
        // Floor at which two source badges read as different colors at the 2-char
        // badge scale. The tightest legitimate pair across the bundled themes is
        // 82 (normal codex-vs-codewhale: blue vs teal), so 60 leaves margin while
        // still failing loudly on a near-collision (a 39-distance regression once
        // shipped on catppuccin). New themes/sources must clear this, not merely
        // differ by one bit.
        const MIN_SOURCE_HUE_DIST: u32 = 60;
        for t in ALL_THEMES {
            let s = &t.source;
            let bg = t.ui.tooltip_bg;
            let hues = s.all();
            // Each hue must contrast the popup bg (lum-sum delta >= 80).
            for (i, h) in hues.iter().enumerate() {
                assert!(
                    lum(*h).abs_diff(lum(bg)) >= 80,
                    "{}: source hue {i} too close to tooltip_bg",
                    t.name
                );
            }
            // Every pair must be mutually distinguishable, not merely unequal.
            for i in 0..hues.len() {
                for j in (i + 1)..hues.len() {
                    let d = manhattan(hues[i], hues[j]);
                    assert!(
                        d >= MIN_SOURCE_HUE_DIST,
                        "{}: source hues {i} and {j} too close ({d} < {MIN_SOURCE_HUE_DIST})",
                        t.name
                    );
                }
            }
        }
    }

    // A newly registered source must get a SourceColors field (→ a hue in every
    // theme + an entry in `all()`), or its badge escapes the distinctness guard
    // above. Pinned by count so the omission fails loudly HERE rather than
    // shipping an unchecked badge color.
    #[test]
    fn source_colors_cover_every_registered_source() {
        use pixtuoid_core::source::REGISTERED_SOURCES;
        assert_eq!(
            NORMAL.source.all().len(),
            REGISTERED_SOURCES.len(),
            "SourceColors has a different hue count than the registered sources — add the \
             new source's field to SourceColors + all() (and a hue in every theme file)"
        );
    }
}
