use pixtuoid_core::sprite::Rgb;

use super::*;

pub static CYBERPUNK: Theme = Theme {
    name: "cyberpunk",
    kind: ThemeKind::Dark,
    surface: SurfaceColors {
        wall: Rgb {
            r: 22,
            g: 18,
            b: 35,
        },
        wall_trim: Rgb {
            r: 50,
            g: 40,
            b: 70,
        },
        carpet_base: Rgb {
            r: 45,
            g: 42,
            b: 55,
        },
        carpet_light: Rgb {
            r: 60,
            g: 55,
            b: 72,
        },
        carpet_dark: Rgb {
            r: 32,
            g: 28,
            b: 42,
        },
        window_frame: Rgb {
            r: 18,
            g: 14,
            b: 28,
        },
        bg_fallback: Rgb {
            r: 12,
            g: 10,
            b: 20,
        },
    },
    office: OfficeColors {
        room_wall_trim_light: Rgb {
            r: 70,
            g: 55,
            b: 95,
        },
        room_wall_trim_dark: Rgb {
            r: 20,
            g: 16,
            b: 32,
        },
        cubicle_divider: Rgb {
            r: 50,
            g: 40,
            b: 75,
        },
        runner_base: Rgb {
            r: 40,
            g: 35,
            b: 55,
        },
        runner_stripe: Rgb {
            r: 60,
            g: 30,
            b: 80,
        },
        runner_edge: Rgb {
            r: 25,
            g: 20,
            b: 38,
        },
        neon_panel_bg: Rgb { r: 8, g: 6, b: 16 },
        neon_frame_base: Rgb {
            r: 80,
            g: 20,
            b: 60,
        },
        building_dark: Rgb {
            r: 12,
            g: 10,
            b: 22,
        },
        building_light: Rgb {
            r: 55,
            g: 45,
            b: 90,
        },
        city_lit_windows: [
            Rgb {
                r: 255,
                g: 60,
                b: 180,
            },
            Rgb {
                r: 0,
                g: 255,
                b: 220,
            },
            Rgb {
                r: 160,
                g: 0,
                b: 255,
            },
        ],
        city_dark_window: Rgb {
            r: 18,
            g: 14,
            b: 30,
        },
        clock_rim: Rgb {
            r: 120,
            g: 80,
            b: 200,
        },
        clock_face: Rgb {
            r: 20,
            g: 15,
            b: 35,
        },
        clock_hand: Rgb {
            r: 0,
            g: 255,
            b: 200,
        },
        shadow: Rgb { r: 10, g: 8, b: 18 },
    },
    lighting: LightingColors {
        day_sky_a: Rgb {
            r: 90,
            g: 50,
            b: 160,
        },
        day_sky_b: Rgb {
            r: 120,
            g: 65,
            b: 190,
        },
        night_sky_a: Rgb { r: 10, g: 6, b: 25 },
        night_sky_b: Rgb {
            r: 20,
            g: 12,
            b: 45,
        },
        twilight_a: Rgb {
            r: 180,
            g: 40,
            b: 120,
        },
        twilight_b: Rgb {
            r: 220,
            g: 60,
            b: 160,
        },
        sun_spill: Rgb {
            r: 200,
            g: 100,
            b: 255,
        },
        ceiling_pool: Rgb {
            r: 120,
            g: 60,
            b: 255,
        },
        floor_lamp_halo: Rgb {
            r: 0,
            g: 200,
            b: 255,
        },
        night_tint: Rgb { r: 8, g: 6, b: 18 },
        // Hot-magenta sun / electric-cyan moon — the theme's own neon duality.
        sun_core: Rgb {
            r: 255,
            g: 90,
            b: 190,
        },
        moon_core: Rgb {
            r: 100,
            g: 230,
            b: 255,
        },
    },
    furniture: FurnitureColors {
        wood_top: Rgb {
            r: 50,
            g: 45,
            b: 65,
        },
        wood_trim: Rgb {
            r: 30,
            g: 25,
            b: 42,
        },
        rug_field: Rgb {
            r: 40,
            g: 15,
            b: 60,
        },
        rug_trim: Rgb {
            r: 25,
            g: 10,
            b: 38,
        },
        rug_accent: Rgb {
            r: 150,
            g: 40,
            b: 120,
        },
        magazine: Rgb {
            r: 60,
            g: 180,
            b: 255,
        },
        magazine_trim: Rgb {
            r: 30,
            g: 90,
            b: 130,
        },
        chair_trim: Rgb {
            r: 28,
            g: 24,
            b: 38,
        },
        coffee_cup: Rgb {
            r: 80,
            g: 70,
            b: 100,
        },
        coffee_cup_shadow: Rgb {
            r: 55,
            g: 48,
            b: 72,
        },
        tank_water: Rgb {
            r: 0x0e,
            g: 0x4a,
            b: 0x5e,
        },
        tank_water_line: Rgb {
            r: 0x3f,
            g: 0xd4,
            b: 0xe6,
        },
        tank_fish: Rgb {
            r: 0xff,
            g: 0x9f,
            b: 0x43,
        },
        tank_fish_alt: Rgb {
            r: 0xff,
            g: 0x2d,
            b: 0x78,
        },
        tank_plant: Rgb {
            r: 0x2d,
            g: 0xe0,
            b: 0x6a,
        },
    },
    effects: EffectColors {
        monitor_frame_lit: Rgb {
            r: 100,
            g: 60,
            b: 200,
        },
        sleep_z: Rgb {
            r: 0,
            g: 200,
            b: 255,
        },
        coffee_steam: Rgb {
            r: 0,
            g: 255,
            b: 140,
        },
        walking_dust: Rgb {
            r: 60,
            g: 50,
            b: 80,
        },
        waiting_bubble: Rgb {
            r: 255,
            g: 60,
            b: 180,
        },
    },
    tool_glow: ToolGlowColors {
        edit: Rgb {
            r: 60,
            g: 120,
            b: 255,
        },
        read: Rgb {
            r: 255,
            g: 60,
            b: 180,
        },
        bash: Rgb {
            r: 255,
            g: 140,
            b: 0,
        },
        agent: Rgb {
            r: 180,
            g: 0,
            b: 255,
        },
        grep: Rgb {
            r: 0,
            g: 255,
            b: 140,
        },
        default: Rgb {
            r: 0,
            g: 255,
            b: 200,
        },
    },
    ui: UiColors {
        label_active: Rgb {
            r: 57,
            g: 255,
            b: 20,
        },
        label_waiting: Rgb {
            r: 255,
            g: 60,
            b: 180,
        },
        label_idle: Rgb {
            r: 80,
            g: 70,
            b: 120,
        },
        label_exiting: Rgb {
            r: 40,
            g: 35,
            b: 60,
        },
        tooltip_bg: Rgb { r: 10, g: 8, b: 20 },
        tooltip_title: Rgb {
            r: 0,
            g: 255,
            b: 200,
        },
        tooltip_text: Rgb {
            r: 180,
            g: 170,
            b: 210,
        },
        tooltip_dim: Rgb {
            r: 100,
            g: 90,
            b: 140,
        },
        neon_brand: Rgb {
            r: 255,
            g: 0,
            b: 200,
        },
        neon_star: Rgb {
            r: 0,
            g: 255,
            b: 200,
        },
    },
    appliance: ApplianceColors {
        vending_body: Rgb {
            r: 45,
            g: 35,
            b: 60,
        },
        vending_panel: Rgb {
            r: 255,
            g: 0,
            b: 200,
        },
        vending_drinks: [
            Rgb {
                r: 60,
                g: 120,
                b: 255,
            },
            Rgb {
                r: 255,
                g: 60,
                b: 180,
            },
            Rgb {
                r: 255,
                g: 140,
                b: 0,
            },
            Rgb {
                r: 0,
                g: 255,
                b: 140,
            },
        ],
        vending_trim: Rgb {
            r: 180,
            g: 140,
            b: 80,
        },
        vending_dark: Rgb {
            r: 15,
            g: 12,
            b: 25,
        },
        printer_body: Rgb {
            r: 200,
            g: 190,
            b: 230,
        },
        printer_top: Rgb {
            r: 50,
            g: 40,
            b: 75,
        },
        printer_glass: Rgb {
            r: 100,
            g: 180,
            b: 220,
        },
        printer_paper: Rgb {
            r: 250,
            g: 245,
            b: 255,
        },
        printer_tray: Rgb {
            r: 160,
            g: 150,
            b: 190,
        },
        coats: [
            Rgb {
                r: 60,
                g: 120,
                b: 255,
            },
            Rgb {
                r: 255,
                g: 60,
                b: 180,
            },
            Rgb {
                r: 0,
                g: 255,
                b: 140,
            },
        ],
    },
    source: SourceColors {
        claude_code: Rgb {
            r: 0xff,
            g: 0x8c,
            b: 0x00,
        }, // neon amber/orange
        codex: Rgb {
            r: 0x00,
            g: 0xe5,
            b: 0xff,
        }, // electric cyan
        reasonix: Rgb {
            r: 0xff,
            g: 0x00,
            b: 0xc8,
        }, // hot magenta
        antigravity: Rgb {
            r: 0x39,
            g: 0xff,
            b: 0x14,
        }, // neon green
        codewhale: Rgb {
            r: 0x1e,
            g: 0x90,
            b: 0xff,
        }, // electric azure
        opencode: Rgb {
            r: 0xff,
            g: 0x3b,
            b: 0x30,
        }, // neon red
        copilot: Rgb {
            r: 0xe0,
            g: 0x60,
            b: 0x9c,
        }, // copilot rose
        cursor: Rgb {
            r: 0xaa,
            g: 0xb4,
            b: 0xc8,
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
    },
};
