use pixtuoid_core::sprite::Rgb;

use super::*;

/// Catppuccin Mocha — warm pastels on dark chocolate.
/// Based on https://github.com/catppuccin/catppuccin
/// Base: #1e1e2e, Surface0: #313244, Overlay0: #6c7086
/// Rosewater: #f5e0dc, Flamingo: #f2cdcd, Pink: #f5c2e7
/// Mauve: #cba6f7, Red: #f38ba8, Maroon: #eba0ac
/// Peach: #fab387, Yellow: #f9e2af, Green: #a6e3a1
/// Teal: #94e2d5, Sky: #89dceb, Sapphire: #74c7ec
/// Blue: #89b4fa, Lavender: #b4befe
pub static CATPPUCCIN: Theme = Theme {
    name: "catppuccin",
    kind: ThemeKind::Dark,
    surface: SurfaceColors {
        wall: Rgb {
            r: 30,
            g: 30,
            b: 46,
        },
        wall_trim: Rgb {
            r: 69,
            g: 71,
            b: 90,
        },
        carpet_base: Rgb {
            r: 49,
            g: 50,
            b: 68,
        },
        carpet_light: Rgb {
            r: 59,
            g: 60,
            b: 80,
        },
        carpet_dark: Rgb {
            r: 39,
            g: 39,
            b: 55,
        },
        window_frame: Rgb {
            r: 24,
            g: 24,
            b: 37,
        },
        bg_fallback: Rgb {
            r: 30,
            g: 30,
            b: 46,
        },
    },
    office: OfficeColors {
        room_wall_trim_light: Rgb {
            r: 69,
            g: 71,
            b: 90,
        },
        room_wall_trim_dark: Rgb {
            r: 24,
            g: 24,
            b: 37,
        },
        cubicle_divider: Rgb {
            r: 69,
            g: 71,
            b: 90,
        },
        runner_base: Rgb {
            r: 55,
            g: 50,
            b: 65,
        },
        runner_stripe: Rgb {
            r: 65,
            g: 58,
            b: 78,
        },
        runner_edge: Rgb {
            r: 39,
            g: 36,
            b: 50,
        },
        neon_panel_bg: Rgb {
            r: 24,
            g: 24,
            b: 37,
        },
        neon_frame_base: Rgb {
            r: 137,
            g: 180,
            b: 250,
        },
        building_dark: Rgb {
            r: 20,
            g: 20,
            b: 32,
        },
        building_light: Rgb {
            r: 70,
            g: 72,
            b: 95,
        },
        city_lit_windows: [
            Rgb {
                r: 249,
                g: 226,
                b: 175,
            },
            Rgb {
                r: 245,
                g: 194,
                b: 231,
            },
            Rgb {
                r: 148,
                g: 226,
                b: 213,
            },
        ],
        city_dark_window: Rgb {
            r: 24,
            g: 24,
            b: 37,
        },
        clock_rim: Rgb {
            r: 180,
            g: 190,
            b: 254,
        },
        clock_face: Rgb {
            r: 205,
            g: 214,
            b: 244,
        },
        clock_hand: Rgb {
            r: 30,
            g: 30,
            b: 46,
        },
        shadow: Rgb {
            r: 17,
            g: 17,
            b: 27,
        },
    },
    lighting: LightingColors {
        day_sky_a: Rgb {
            r: 110,
            g: 115,
            b: 150,
        },
        day_sky_b: Rgb {
            r: 135,
            g: 140,
            b: 175,
        },
        night_sky_a: Rgb {
            r: 17,
            g: 17,
            b: 27,
        },
        night_sky_b: Rgb {
            r: 24,
            g: 24,
            b: 37,
        },
        twilight_a: Rgb {
            r: 250,
            g: 179,
            b: 135,
        },
        twilight_b: Rgb {
            r: 245,
            g: 194,
            b: 231,
        },
        sun_spill: Rgb {
            r: 249,
            g: 226,
            b: 175,
        },
        ceiling_pool: Rgb {
            r: 205,
            g: 214,
            b: 244,
        },
        floor_lamp_halo: Rgb {
            r: 249,
            g: 226,
            b: 175,
        },
        night_tint: Rgb {
            r: 17,
            g: 17,
            b: 27,
        },
        // Catppuccin peach sun / lavender moon.
        sun_core: Rgb {
            r: 250,
            g: 179,
            b: 135,
        },
        moon_core: Rgb {
            r: 180,
            g: 190,
            b: 254,
        },
    },
    furniture: FurnitureColors {
        wood_top: Rgb {
            r: 69,
            g: 71,
            b: 90,
        },
        wood_trim: Rgb {
            r: 49,
            g: 50,
            b: 68,
        },
        rug_field: Rgb {
            r: 60,
            g: 45,
            b: 65,
        },
        rug_trim: Rgb {
            r: 42,
            g: 32,
            b: 48,
        },
        rug_accent: Rgb {
            r: 203,
            g: 166,
            b: 247,
        },
        magazine: Rgb {
            r: 137,
            g: 180,
            b: 250,
        },
        magazine_trim: Rgb {
            r: 68,
            g: 90,
            b: 125,
        },
        chair_trim: Rgb {
            r: 39,
            g: 39,
            b: 55,
        },
        coffee_cup: Rgb {
            r: 127,
            g: 132,
            b: 156,
        },
        coffee_cup_shadow: Rgb {
            r: 108,
            g: 112,
            b: 134,
        },
        tank_water: Rgb {
            r: 0x45,
            g: 0x64,
            b: 0x7e,
        },
        tank_water_line: Rgb {
            r: 0x89,
            g: 0xdc,
            b: 0xeb,
        },
        tank_fish: Rgb {
            r: 0xfa,
            g: 0xb3,
            b: 0x87,
        },
        tank_fish_alt: Rgb {
            r: 0xf3,
            g: 0x8b,
            b: 0xa8,
        },
        tank_plant: Rgb {
            r: 0xa6,
            g: 0xe3,
            b: 0xa1,
        },
    },
    effects: EffectColors {
        monitor_frame_lit: Rgb {
            r: 69,
            g: 71,
            b: 90,
        },
        sleep_z: Rgb {
            r: 137,
            g: 220,
            b: 235,
        },
        coffee_steam: Rgb {
            r: 203,
            g: 166,
            b: 247,
        },
        walking_dust: Rgb {
            r: 59,
            g: 60,
            b: 80,
        },
        waiting_bubble: Rgb {
            r: 249,
            g: 226,
            b: 175,
        },
    },
    tool_glow: ToolGlowColors {
        edit: Rgb {
            r: 137,
            g: 180,
            b: 250,
        },
        read: Rgb {
            r: 116,
            g: 199,
            b: 236,
        },
        bash: Rgb {
            r: 250,
            g: 179,
            b: 135,
        },
        agent: Rgb {
            r: 203,
            g: 166,
            b: 247,
        },
        grep: Rgb {
            r: 166,
            g: 227,
            b: 161,
        },
        default: Rgb {
            r: 148,
            g: 226,
            b: 213,
        },
    },
    ui: UiColors {
        label_active: Rgb {
            r: 166,
            g: 227,
            b: 161,
        },
        label_waiting: Rgb {
            r: 249,
            g: 226,
            b: 175,
        },
        label_idle: Rgb {
            r: 108,
            g: 112,
            b: 134,
        },
        label_exiting: Rgb {
            r: 69,
            g: 71,
            b: 90,
        },
        tooltip_bg: Rgb {
            r: 24,
            g: 24,
            b: 37,
        },
        tooltip_title: Rgb {
            r: 205,
            g: 214,
            b: 244,
        },
        tooltip_text: Rgb {
            r: 166,
            g: 173,
            b: 200,
        },
        tooltip_dim: Rgb {
            r: 108,
            g: 112,
            b: 134,
        },
        neon_brand: Rgb {
            r: 137,
            g: 180,
            b: 250,
        },
        neon_star: Rgb {
            r: 245,
            g: 194,
            b: 231,
        },
    },
    appliance: ApplianceColors {
        vending_body: Rgb {
            r: 55,
            g: 56,
            b: 74,
        },
        vending_panel: Rgb {
            r: 137,
            g: 180,
            b: 250,
        },
        vending_drinks: [
            Rgb {
                r: 250,
                g: 179,
                b: 135,
            },
            Rgb {
                r: 116,
                g: 199,
                b: 236,
            },
            Rgb {
                r: 166,
                g: 227,
                b: 161,
            },
            Rgb {
                r: 203,
                g: 166,
                b: 247,
            },
        ],
        vending_trim: Rgb {
            r: 210,
            g: 180,
            b: 160,
        },
        vending_dark: Rgb {
            r: 24,
            g: 24,
            b: 37,
        },
        printer_body: Rgb {
            r: 205,
            g: 214,
            b: 244,
        },
        printer_top: Rgb {
            r: 69,
            g: 71,
            b: 90,
        },
        printer_glass: Rgb {
            r: 148,
            g: 226,
            b: 213,
        },
        printer_paper: Rgb {
            r: 245,
            g: 240,
            b: 235,
        },
        printer_tray: Rgb {
            r: 137,
            g: 140,
            b: 165,
        },
        coats: [
            Rgb {
                r: 250,
                g: 179,
                b: 135,
            },
            Rgb {
                r: 137,
                g: 180,
                b: 250,
            },
            Rgb {
                r: 203,
                g: 166,
                b: 247,
            },
        ],
    },
    source: SourceColors {
        claude_code: Rgb {
            r: 0xfa,
            g: 0xb3,
            b: 0x87,
        }, // catppuccin peach
        codex: Rgb {
            r: 0x89,
            g: 0xdc,
            b: 0xeb,
        }, // catppuccin sky
        reasonix: Rgb {
            r: 0xcb,
            g: 0xa6,
            b: 0xf7,
        }, // catppuccin mauve
        antigravity: Rgb {
            r: 0xa6,
            g: 0xe3,
            b: 0xa1,
        }, // catppuccin green
        codewhale: Rgb {
            r: 0x33,
            g: 0xc0,
            b: 0xac,
        }, // deep teal — pulled off the pale sky codex uses (badge legibility)
        opencode: Rgb {
            r: 0xf3,
            g: 0x8b,
            b: 0xa8,
        }, // catppuccin red
        copilot: Rgb {
            r: 0xe0,
            g: 0x60,
            b: 0x9c,
        }, // copilot rose
        cursor: Rgb {
            r: 0x96,
            g: 0xa2,
            b: 0xbe,
        }, // cursor slate-blue (monochrome brand; distinct from all 7)
        openclaw: Rgb {
            r: 0xff,
            g: 0xaa,
            b: 0x30,
        }, // openclaw marigold (lobster; warm, clears claude-amber + opencode-red)
        hermes: Rgb {
            r: 0x68,
            g: 0x58,
            b: 0xe0,
        }, // Hermes/Nous indigo (brand-constant like openclaw marigold; clears reasonix-violet + cursor-slate + codex-blue on every theme, min Manhattan >=92)
        omp: Rgb {
            r: 0x8a,
            g: 0xc8,
            b: 0x32,
        }, // omp lime (brand-constant; clears antigravity-green + codewhale-teal + claude-amber on every theme)
        grok: Rgb {
            r: 0x30,
            g: 0xd0,
            b: 0xe8,
        }, // grok cyan (brand-constant; nearest neighbor codewhale-teal at Manhattan 78 ≥ the 60 floor, and the SITE's chip transforms clear WCAG AA in both themes: ×0.55-on-day-chip 5.3:1, ×0.8+white-on-night-chip 11.6:1 — the silver e6e6e6 failed the day chip at 3.83:1, caught by the site e2e badge sweep)
    },
};
