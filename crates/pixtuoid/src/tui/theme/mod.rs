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
    pub desk_plant_light: Rgb,
    pub desk_plant_dark: Rgb,
    pub desk_plant_pot: Rgb,
    pub photo_frame: Rgb,
    pub photo_bg: Rgb,
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
    pub neon_ticker: Rgb,
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

    // Every theme's appliance palette must keep the appliances LEGIBLE — the
    // bug was a hardcoded normal-theme set on all themes, so this guards both
    // that each theme supplies its own AND that the supplied set reads right.
    #[test]
    fn appliance_palette_is_legible_for_every_theme() {
        fn lum(c: Rgb) -> u32 {
            c.0 as u32 + c.1 as u32 + c.2 as u32
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
}
