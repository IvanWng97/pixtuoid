use pixtuoid_core::sprite::Rgb;

use super::*;

pub static TOKYO_NIGHT: Theme = Theme {
    name: "tokyo-night",
    kind: ThemeKind::Dark,
    surface: SurfaceColors {
        wall: Rgb {
            r: 26,
            g: 27,
            b: 38,
        },
        wall_trim: Rgb {
            r: 65,
            g: 72,
            b: 104,
        },
        baseboard: Rgb {
            r: 18,
            g: 18,
            b: 28,
        },
        carpet_base: Rgb {
            r: 36,
            g: 40,
            b: 59,
        },
        carpet_light: Rgb {
            r: 48,
            g: 52,
            b: 72,
        },
        carpet_dark: Rgb {
            r: 26,
            g: 28,
            b: 42,
        },
        window_frame: Rgb {
            r: 18,
            g: 18,
            b: 28,
        },
        bg_fallback: Rgb {
            r: 26,
            g: 27,
            b: 38,
        },
    },
    office: OfficeColors {
        room_wall_body: Rgb {
            r: 36,
            g: 40,
            b: 59,
        },
        room_wall_trim_light: Rgb {
            r: 65,
            g: 72,
            b: 104,
        },
        room_wall_trim_dark: Rgb {
            r: 22,
            g: 24,
            b: 36,
        },
        cubicle_divider: Rgb {
            r: 65,
            g: 72,
            b: 104,
        },
        runner_base: Rgb {
            r: 40,
            g: 44,
            b: 62,
        },
        runner_stripe: Rgb {
            r: 52,
            g: 56,
            b: 78,
        },
        runner_edge: Rgb {
            r: 28,
            g: 30,
            b: 44,
        },
        neon_panel_bg: Rgb {
            r: 18,
            g: 18,
            b: 28,
        },
        neon_frame_base: Rgb {
            r: 122,
            g: 162,
            b: 247,
        },
        building_dark: Rgb {
            r: 18,
            g: 18,
            b: 28,
        },
        building_light: Rgb {
            r: 55,
            g: 62,
            b: 90,
        },
        city_lit_windows: [
            Rgb {
                r: 180,
                g: 200,
                b: 255,
            },
            Rgb {
                r: 224,
                g: 175,
                b: 104,
            },
            Rgb {
                r: 158,
                g: 206,
                b: 106,
            },
        ],
        city_dark_window: Rgb {
            r: 22,
            g: 24,
            b: 36,
        },
        clock_rim: Rgb {
            r: 169,
            g: 177,
            b: 214,
        },
        clock_face: Rgb {
            r: 192,
            g: 202,
            b: 230,
        },
        clock_hand: Rgb {
            r: 26,
            g: 27,
            b: 38,
        },
        shadow: Rgb {
            r: 14,
            g: 14,
            b: 22,
        },
    },
    lighting: LightingColors {
        day_sky_a: Rgb {
            r: 85,
            g: 115,
            b: 180,
        },
        day_sky_b: Rgb {
            r: 110,
            g: 140,
            b: 200,
        },
        night_sky_a: Rgb {
            r: 14,
            g: 16,
            b: 32,
        },
        night_sky_b: Rgb {
            r: 22,
            g: 26,
            b: 50,
        },
        twilight_a: Rgb {
            r: 122,
            g: 162,
            b: 247,
        },
        twilight_b: Rgb {
            r: 125,
            g: 207,
            b: 255,
        },
        sun_spill: Rgb {
            r: 180,
            g: 200,
            b: 255,
        },
        ceiling_pool: Rgb {
            r: 160,
            g: 190,
            b: 255,
        },
        floor_lamp_halo: Rgb {
            r: 122,
            g: 162,
            b: 247,
        },
        night_tint: Rgb {
            r: 10,
            g: 12,
            b: 26,
        },
    },
    furniture: FurnitureColors {
        wood_top: Rgb {
            r: 42,
            g: 52,
            b: 82,
        },
        wood_trim: Rgb {
            r: 28,
            g: 36,
            b: 60,
        },
        rug_field: Rgb {
            r: 30,
            g: 40,
            b: 70,
        },
        rug_trim: Rgb {
            r: 20,
            g: 28,
            b: 50,
        },
        rug_accent: Rgb {
            r: 122,
            g: 162,
            b: 247,
        },
        magazine: Rgb {
            r: 122,
            g: 162,
            b: 247,
        },
        magazine_trim: Rgb {
            r: 60,
            g: 80,
            b: 124,
        },
        chair_seat: Rgb {
            r: 42,
            g: 46,
            b: 64,
        },
        chair_trim: Rgb {
            r: 28,
            g: 30,
            b: 44,
        },
        coffee_cup: Rgb {
            r: 110,
            g: 115,
            b: 140,
        },
        coffee_cup_shadow: Rgb {
            r: 80,
            g: 84,
            b: 108,
        },
    },
    effects: EffectColors {
        monitor_frame_lit: Rgb {
            r: 65,
            g: 72,
            b: 104,
        },
        sleep_z: Rgb {
            r: 125,
            g: 207,
            b: 255,
        },
        coffee_steam: Rgb {
            r: 187,
            g: 154,
            b: 247,
        },
        walking_dust: Rgb {
            r: 48,
            g: 52,
            b: 72,
        },
        waiting_bubble: Rgb {
            r: 224,
            g: 175,
            b: 104,
        },
    },
    tool_glow: ToolGlowColors {
        edit: Rgb {
            r: 122,
            g: 162,
            b: 247,
        },
        read: Rgb {
            r: 125,
            g: 207,
            b: 255,
        },
        bash: Rgb {
            r: 224,
            g: 175,
            b: 104,
        },
        agent: Rgb {
            r: 187,
            g: 154,
            b: 247,
        },
        grep: Rgb {
            r: 158,
            g: 206,
            b: 106,
        },
        default: Rgb {
            r: 125,
            g: 207,
            b: 255,
        },
    },
    ui: UiColors {
        label_active: Rgb {
            r: 158,
            g: 206,
            b: 106,
        },
        label_waiting: Rgb {
            r: 224,
            g: 175,
            b: 104,
        },
        label_idle: Rgb {
            r: 65,
            g: 72,
            b: 104,
        },
        label_exiting: Rgb {
            r: 45,
            g: 48,
            b: 65,
        },
        tooltip_bg: Rgb {
            r: 18,
            g: 18,
            b: 28,
        },
        tooltip_title: Rgb {
            r: 192,
            g: 202,
            b: 230,
        },
        tooltip_text: Rgb {
            r: 169,
            g: 177,
            b: 214,
        },
        tooltip_dim: Rgb {
            r: 65,
            g: 72,
            b: 104,
        },
        neon_brand: Rgb {
            r: 122,
            g: 162,
            b: 247,
        },
        neon_star: Rgb {
            r: 247,
            g: 118,
            b: 142,
        },
        neon_ticker: Rgb {
            r: 125,
            g: 207,
            b: 255,
        },
    },
    appliance: ApplianceColors {
        vending_body: Rgb {
            r: 36,
            g: 40,
            b: 59,
        },
        vending_panel: Rgb {
            r: 122,
            g: 162,
            b: 247,
        },
        vending_drinks: [
            Rgb {
                r: 122,
                g: 162,
                b: 247,
            },
            Rgb {
                r: 125,
                g: 207,
                b: 255,
            },
            Rgb {
                r: 158,
                g: 206,
                b: 106,
            },
            Rgb {
                r: 224,
                g: 175,
                b: 104,
            },
        ],
        vending_trim: Rgb {
            r: 127,
            g: 115,
            b: 80,
        },
        vending_dark: Rgb {
            r: 18,
            g: 18,
            b: 28,
        },
        printer_body: Rgb {
            r: 192,
            g: 202,
            b: 230,
        },
        printer_top: Rgb {
            r: 36,
            g: 40,
            b: 59,
        },
        printer_glass: Rgb {
            r: 125,
            g: 207,
            b: 255,
        },
        printer_paper: Rgb {
            r: 245,
            g: 245,
            b: 240,
        },
        printer_tray: Rgb {
            r: 110,
            g: 120,
            b: 150,
        },
        coats: [
            Rgb {
                r: 122,
                g: 162,
                b: 247,
            },
            Rgb {
                r: 247,
                g: 118,
                b: 142,
            },
            Rgb {
                r: 220,
                g: 220,
                b: 225,
            },
        ],
    },
    source: SourceColors {
        claude_code: Rgb {
            r: 0xe0,
            g: 0xaf,
            b: 0x68,
        }, // tokyo-night warm gold
        codex: Rgb {
            r: 0x7d,
            g: 0xcf,
            b: 0xff,
        }, // tokyo-night sky blue
        reasonix: Rgb {
            r: 0xbb,
            g: 0x9a,
            b: 0xf7,
        }, // tokyo-night purple
        antigravity: Rgb {
            r: 0x9e,
            g: 0xce,
            b: 0x6a,
        }, // tokyo-night green
        codewhale: Rgb {
            r: 0x2a,
            g: 0xc0,
            b: 0xb0,
        }, // tokyo-night teal
        opencode: Rgb {
            r: 0xf7,
            g: 0x76,
            b: 0x8e,
        }, // tokyo-night red
    },
};
