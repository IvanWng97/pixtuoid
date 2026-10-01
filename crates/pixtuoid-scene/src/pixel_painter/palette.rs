//! Per-agent colors and the color math the painters share.

pub(super) use crate::composite::{BLACK, WHITE, blend_rgb};
use pixtuoid_core::AgentSlot;
use pixtuoid_core::id::normalize_path_key;
use pixtuoid_core::sprite::{Frame, Pixel, Rgb, RgbBuffer};

/// A complete shirt + pants combo, keyed by the agent's normalized working
/// directory (same cwd → same outfit, so the office reads as a color-coded
/// org-chart). Whole outfits rather than independent shirt and pants colors, so
/// the pairing is always harmonious instead of a random clash.
#[derive(Clone, Copy)]
struct Outfit {
    shirt: Rgb,
    pants: Rgb,
}

/// The curated outfit pool, indexed by the cwd seed modulo its length (Team
/// Palette): warm `[0..8)` then cool `[8..16)` — an aesthetic grouping, NOT a
/// personality axis.
const OUTFITS: &[Outfit; 16] = &[
    // Warm
    // Wes Anderson — Grand Budapest concierge (cream + plum)
    Outfit {
        shirt: Rgb {
            r: 0xee,
            g: 0xe1,
            b: 0xc6,
        },
        pants: Rgb {
            r: 0x4a,
            g: 0x2b,
            b: 0x3d,
        },
    },
    // Ghibli earthy — terracotta + sand
    Outfit {
        shirt: Rgb {
            r: 0xc9,
            g: 0x7b,
            b: 0x5e,
        },
        pants: Rgb {
            r: 0x6b,
            g: 0x57,
            b: 0x3d,
        },
    },
    // 70s academic — mustard + olive
    Outfit {
        shirt: Rgb {
            r: 0xc9,
            g: 0xa2,
            b: 0x4b,
        },
        pants: Rgb {
            r: 0x4a,
            g: 0x52,
            b: 0x34,
        },
    },
    // Burgundy + warm stone (moody academic)
    Outfit {
        shirt: Rgb {
            r: 0x8a,
            g: 0x2c,
            b: 0x36,
        },
        pants: Rgb {
            r: 0x5a,
            g: 0x4e,
            b: 0x42,
        },
    },
    // Mediterranean — coral + dark navy
    Outfit {
        shirt: Rgb {
            r: 0xd7,
            g: 0x7a,
            b: 0x61,
        },
        pants: Rgb {
            r: 0x27,
            g: 0x33,
            b: 0x4a,
        },
    },
    // Camel + chocolate (luxury minimal)
    Outfit {
        shirt: Rgb {
            r: 0xb8,
            g: 0x99,
            b: 0x68,
        },
        pants: Rgb {
            r: 0x3d,
            g: 0x2a,
            b: 0x1f,
        },
    },
    // Rust + cream (autumn)
    Outfit {
        shirt: Rgb {
            r: 0xa5,
            g: 0x4f,
            b: 0x2c,
        },
        pants: Rgb {
            r: 0xcd,
            g: 0xc0,
            b: 0xa3,
        },
    },
    // Salmon + warm charcoal
    Outfit {
        shirt: Rgb {
            r: 0xe0,
            g: 0x90,
            b: 0x7c,
        },
        pants: Rgb {
            r: 0x3a,
            g: 0x32,
            b: 0x2e,
        },
    },
    // Cool
    // Modern minimal — sage + charcoal
    Outfit {
        shirt: Rgb {
            r: 0xa4,
            g: 0xb5,
            b: 0x95,
        },
        pants: Rgb {
            r: 0x33,
            g: 0x36,
            b: 0x3d,
        },
    },
    // Professional — pale blue + slate
    Outfit {
        shirt: Rgb {
            r: 0x9b,
            g: 0xb5,
            b: 0xc8,
        },
        pants: Rgb {
            r: 0x3c,
            g: 0x44,
            b: 0x52,
        },
    },
    // Soft moody — lavender + espresso
    Outfit {
        shirt: Rgb {
            r: 0xa2,
            g: 0x90,
            b: 0xb0,
        },
        pants: Rgb {
            r: 0x3c,
            g: 0x2a,
            b: 0x1e,
        },
    },
    // Outdoorsy — forest green + khaki
    Outfit {
        shirt: Rgb {
            r: 0x3f,
            g: 0x61,
            b: 0x4c,
        },
        pants: Rgb {
            r: 0x7a,
            g: 0x67,
            b: 0x48,
        },
    },
    // Confident — teal + cream
    Outfit {
        shirt: Rgb {
            r: 0x3e,
            g: 0x7a,
            b: 0x85,
        },
        pants: Rgb {
            r: 0xc7,
            g: 0xb6,
            b: 0x96,
        },
    },
    // Preppy — indigo + warm grey
    Outfit {
        shirt: Rgb {
            r: 0x3f,
            g: 0x4a,
            b: 0x75,
        },
        pants: Rgb {
            r: 0x8a,
            g: 0x84,
            b: 0x7a,
        },
    },
    // Nordic — dusty blue + navy
    Outfit {
        shirt: Rgb {
            r: 0x6b,
            g: 0x84,
            b: 0xa0,
        },
        pants: Rgb {
            r: 0x2a,
            g: 0x33,
            b: 0x4a,
        },
    },
    // Mossy — pine + bone
    Outfit {
        shirt: Rgb {
            r: 0x47,
            g: 0x69,
            b: 0x5a,
        },
        pants: Rgb {
            r: 0xb8,
            g: 0xae,
            b: 0x95,
        },
    },
];

const HAIR_PRESETS: &[Rgb] = &[
    Rgb {
        r: 0x14,
        g: 0x0a,
        b: 0x06,
    }, // jet black
    Rgb {
        r: 0x2a,
        g: 0x1a,
        b: 0x0e,
    }, // near-black brown
    Rgb {
        r: 0x52,
        g: 0x32,
        b: 0x10,
    }, // dark brown
    Rgb {
        r: 0x8a,
        g: 0x5a,
        b: 0x36,
    }, // light brown
    Rgb {
        r: 0xc7,
        g: 0xa3,
        b: 0x4a,
    }, // blond
    Rgb {
        r: 0xd8,
        g: 0x68,
        b: 0x32,
    }, // ginger
    Rgb {
        r: 0x7a,
        g: 0x32,
        b: 0x10,
    }, // auburn
    Rgb {
        r: 0xa8,
        g: 0xa8,
        b: 0xb0,
    }, // silver-grey
];
const SKIN_PRESETS: &[Rgb] = &[
    Rgb {
        r: 0xf4,
        g: 0xc7,
        b: 0x9a,
    }, // light peach
    Rgb {
        r: 0xe0,
        g: 0xa8,
        b: 0x70,
    }, // medium
    Rgb {
        r: 0xb8,
        g: 0x80,
        b: 0x50,
    }, // tan
    Rgb {
        r: 0x8a,
        g: 0x5a,
        b: 0x36,
    }, // deep brown
    Rgb {
        r: 0xc8,
        g: 0x9a,
        b: 0x64,
    }, // warm tan
];

/// Deterministic seed from a normalized cwd string: byte-fold, then the
/// splitmix64 finalizer. NOT `DefaultHasher`: its algorithm may change between
/// Rust releases, which would re-dress every agent on a toolchain bump.
fn cwd_outfit_seed(cwd_norm: &str) -> u64 {
    let folded = cwd_norm
        .bytes()
        .fold(0u64, |h, b| h.wrapping_mul(131).wrapping_add(b as u64));
    pixtuoid_core::id::splitmix64(folded)
}

/// The outfit-determining seed for `agent`. Extracted so
/// `FrameCache::note_outfit_seed` watches the mid-lifetime cwd backfill through
/// the EXACT unknown-cwd fallback [`agent_overrides`] uses; a second copy would
/// drift.
pub(super) fn outfit_seed_for(agent: &AgentSlot) -> u64 {
    if agent.unknown_cwd || agent.cwd.as_os_str().is_empty() {
        agent.agent_id.raw()
    } else {
        cwd_outfit_seed(&normalize_path_key(&agent.cwd.to_string_lossy()))
    }
}

/// A burning agent's hair — an alias of the flame gradient's deep base, so a
/// gradient tweak can't desync the hair from the crown.
const EMBER_HAIR: Rgb = super::effects::FLAME_DEEP;

/// The palette keys a character sprite draws its shirt, hair, skin and pants
/// in: the keys [`agent_overrides`] replaces, so a pack's own sprites take each
/// agent's colors.
pub(super) const SHIRT_KEY: char = 'B';
/// See [`SHIRT_KEY`].
pub(super) const HAIR_KEY: char = 'H';
/// See [`SHIRT_KEY`].
pub(super) const SKIN_KEY: char = 'S';
/// See [`SHIRT_KEY`].
pub(super) const PANTS_KEY: char = 'P';

/// The pack key of a monitor's glass.
pub(crate) const SCREEN_GLASS_KEY: char = 'j';

/// The pack key of the dim content an idle screen shows on its glass.
pub(crate) const SCREEN_TEXT_KEY: char = 'J';

/// The pack key of a desk lamp's bulb, which glows of its own at any hour.
pub(crate) const DESK_BULB_KEY: char = '9';

/// The pack key of the wall clock's face, inside its rim.
pub(crate) const CLOCK_FACE_KEY: char = 'ц';

/// The fixtures' [`appliance_overrides`].
pub(crate) fn fixture_overrides(theme: &crate::theme::Theme) -> [(char, Pixel); 16] {
    let (f, o) = (&theme.furniture, &theme.office);
    let [c0, c1, c2] = theme.appliance.coats;
    [
        ('Д', Some(f.tank_water)),
        ('З', Some(f.tank_water_line)),
        ('И', Some(f.tank_fish)),
        ('Л', Some(f.tank_fish_alt)),
        ('Ь', Some(f.tank_plant)),
        ('ж', Some(o.room_wall_trim_dark)),
        ('б', Some(o.building_light)),
        ('ы', Some(f.magazine)),
        ('э', Some(f.magazine_trim)),
        ('ч', Some(c0)),
        ('ш', Some(c1)),
        ('щ', Some(c2)),
        ('ф', Some(o.clock_rim)),
        (CLOCK_FACE_KEY, Some(o.clock_face)),
        ('з', Some(o.clock_hand)),
        // Un-themed: the classic draws the cooler's bottle in its own blue.
        ('χ', Some(super::furniture::COOLER_WATER)),
    ]
}

/// The pack keys a corridor appliance's art is drawn in, each with the
/// [`ApplianceColors`](crate::theme::ApplianceColors) role it takes: the art
/// owns the form, the theme the palette. The pack's `[ramps]` of these keys are
/// the shading, re-derived by the recolour.
pub(crate) fn appliance_overrides(a: &crate::theme::ApplianceColors) -> [(char, Pixel); 13] {
    let [d0, d1, d2, d3] = a.vending_drinks;
    [
        ('Б', Some(a.vending_body)),
        ('П', Some(a.vending_panel)),
        ('Ч', Some(d0)),
        ('Ш', Some(d1)),
        ('Щ', Some(d2)),
        ('Э', Some(d3)),
        ('Ф', Some(a.vending_trim)),
        ('Ы', Some(a.vending_dark)),
        ('Ю', Some(a.printer_body)),
        ('Я', Some(a.printer_top)),
        ('Ё', Some(a.printer_glass)),
        ('Й', Some(a.printer_paper)),
        ('Ц', Some(a.printer_tray)),
    ]
}

/// One agent's colors, as the palette overrides a character frame is
/// recolored with. `Some(glow_tint)` blends the skin toward the monitor glow so
/// a seated agent reads as lit by their screen.
pub(super) fn agent_overrides(
    agent: &AgentSlot,
    glow_tint: Option<Rgb>,
    burn: crate::burn::BurnTier,
) -> [(char, Pixel); 4] {
    let id_seed = agent.agent_id.raw() as usize;
    let outfit_seed = outfit_seed_for(agent);
    let outfit = OUTFITS[outfit_seed as usize % OUTFITS.len()];
    let hair = if burn == crate::burn::BurnTier::Normal {
        HAIR_PRESETS[(id_seed / 7) % HAIR_PRESETS.len()]
    } else {
        EMBER_HAIR
    };
    let skin = SKIN_PRESETS[(id_seed / 13) % SKIN_PRESETS.len()];
    let final_skin = if let Some(tint) = glow_tint {
        blend_rgb(skin, tint, 0.18)
    } else {
        skin
    };
    [
        (SHIRT_KEY, Some(outfit.shirt)),
        (HAIR_KEY, Some(hair)),
        (SKIN_KEY, Some(final_skin)),
        (PANTS_KEY, Some(outfit.pants)),
    ]
}

/// The exhaustive `ToolKind → hue` map. Read by the office monitor glow AND, via
/// re-export, by the binary's footer tool-segment tint, so the two share one hue
/// per tool.
pub fn tool_glow_for_kind(
    kind: pixtuoid_core::state::ToolKind,
    glow: &crate::theme::ToolGlowColors,
) -> Rgb {
    use pixtuoid_core::state::ToolKind;
    match kind {
        ToolKind::Edit => glow.edit,
        ToolKind::Read => glow.read,
        ToolKind::Bash => glow.bash,
        ToolKind::Task => glow.agent,
        ToolKind::Search => glow.grep,
        ToolKind::Other => glow.default,
    }
}

/// The monitor glow color for an agent's active tool, or `None` when the agent
/// is not Active.
pub(super) fn tool_glow_tint(
    agent: &AgentSlot,
    glow: &crate::theme::ToolGlowColors,
) -> Option<Rgb> {
    use pixtuoid_core::state::ActivityState;
    match &agent.state {
        ActivityState::Active { kind, .. } => Some(tool_glow_for_kind(*kind, glow)),
        _ => None,
    }
}

/// Map one mascot pixel to its "degraded" look: a gateway that is UP but whose
/// model backend fails every run must read as UNWELL.
pub(super) fn degraded_pixel(c: Rgb) -> Rgb {
    // By eye: unwell, but not so grey that the dull-red bias below stops showing.
    const SATURATION_DRAIN: f32 = 0.55;
    let lum = ((c.r as f32) * 0.30 + (c.g as f32) * 0.59 + (c.b as f32) * 0.11) as u8;
    let gray = Rgb {
        r: lum,
        g: lum,
        b: lum,
    };
    let desat = blend_rgb(c, gray, SATURATION_DRAIN);
    let sick = Rgb {
        r: 150,
        g: 40,
        b: 40,
    };
    let tinted = blend_rgb(desat, sick, 0.45);
    blend_rgb(
        tinted, BLACK, 0.18, // dim: the mascot looks drained
    )
}

/// A degraded copy of a mascot frame — every opaque pixel through
/// [`degraded_pixel`], transparency preserved.
pub(super) fn degraded_frame(frame: &Frame) -> Frame {
    let pixels = frame
        .as_slice()
        .iter()
        .map(|&p| p.map(degraded_pixel))
        .collect();
    Frame::from_pixels(frame.width(), frame.height(), pixels)
}

/// A pixel transform tabulated over the diagonal greys — byte-identical to
/// calling `f` per pixel, but three L1 loads instead of the f32 chain, ONLY
/// for channel-separable `f` (every constant-tint [`blend`](crate::composite::blend) chain is; a
/// transform where one output channel reads another input channel tabulates
/// wrong). Amortizes when a pass touches ≫256 pixels.
pub(super) struct RgbLut {
    r: [u8; 256],
    g: [u8; 256],
    b: [u8; 256],
}

impl RgbLut {
    pub(super) fn tabulate(f: impl Fn(Rgb) -> Rgb) -> Self {
        let mut lut = RgbLut {
            r: [0; 256],
            g: [0; 256],
            b: [0; 256],
        };
        for i in 0..256 {
            let v = i as u8;
            let o = f(Rgb { r: v, g: v, b: v });
            lut.r[i] = o.r;
            lut.g[i] = o.g;
            lut.b[i] = o.b;
        }
        lut
    }

    #[inline]
    pub(super) fn apply(&self, c: Rgb) -> Rgb {
        Rgb {
            r: self.r[c.r as usize],
            g: self.g[c.g as usize],
            b: self.b[c.b as usize],
        }
    }
}

/// Composite `tint` over the existing buffer pixel at `(x, y)` by `t` — the
/// haze / overlay primitive.
pub(super) fn blend_over(buf: &RgbBuffer, x: u16, y: u16, tint: Rgb, t: f32) -> Rgb {
    blend_rgb(buf.get(x, y), tint, t)
}

/// Composite `tint` over the buffer pixel at `(x, y)` by `t` AND write it back.
/// Clips like [`RgbBuffer::put_checked`]: a no-op outside the buffer. Use
/// [`blend_over`] instead when the blended color feeds a further composite
/// rather than landing straight back on the buffer.
pub(super) fn blend_pixel(buf: &mut RgbBuffer, x: u16, y: u16, tint: Rgb, t: f32) {
    if x < buf.width() && y < buf.height() {
        let blended = blend_over(buf, x, y, tint, t);
        buf.put(x, y, blended);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins `MAX_RAMP_LEVEL` against the colors a recolor feeds a ramp: every
    /// agent color keeps a shade of its own at every level a pack may declare.
    #[test]
    fn every_agent_color_keeps_a_distinct_shade_at_every_ramp_level() {
        use pixtuoid_core::sprite::format::MAX_RAMP_LEVEL;
        let outfits = OUTFITS.iter().flat_map(|o| [o.shirt, o.pants]);
        let colors = HAIR_PRESETS
            .iter()
            .chain(SKIN_PRESETS)
            .copied()
            .chain(outfits)
            .chain([EMBER_HAIR]);
        for c in colors {
            let shades: Vec<Rgb> = (-MAX_RAMP_LEVEL..=MAX_RAMP_LEVEL)
                .map(|n| c.ramp(n))
                .collect();
            assert!(shades.windows(2).all(|w| w[0] != w[1]), "{c:?}: {shades:?}");
        }
    }

    #[test]
    fn blend_pixel_composites_in_bounds_and_noops_out_of_bounds() {
        let base = Rgb {
            r: 100,
            g: 100,
            b: 100,
        };
        let tint = Rgb { r: 0, g: 0, b: 0 };
        let mut buf = RgbBuffer::filled(2, 2, base);
        blend_pixel(&mut buf, 1, 1, tint, 0.5);
        assert_eq!(buf.get(1, 1), blend_rgb(base, tint, 0.5));
        assert_eq!(buf.get(0, 0), base, "neighbor untouched");
        blend_pixel(&mut buf, 2, 0, tint, 0.5);
        blend_pixel(&mut buf, 0, 2, tint, 0.5);
        assert_eq!(buf.get(0, 0), base);
        assert_eq!(
            buf.get(1, 1),
            blend_rgb(base, tint, 0.5),
            "in-bounds pixel unchanged"
        );
    }

    /// The appliance art's keys are the bundled pack's, and the pack's colour
    /// for each is the normal theme's, as `pack.toml` says: a painter that does
    /// not recolour still shows the normal office.
    #[test]
    fn the_packs_appliance_keys_are_the_normal_themes_colours() {
        let pack = crate::embedded_pack::test_default_pack();
        let normal = crate::theme::theme_by_name("normal").expect("theme");
        for (key, pixel) in appliance_overrides(&normal.appliance) {
            assert_eq!(pack.palette().get(key), Some(pixel), "key {key:?}");
        }
    }

    /// The pack's own colours for [`fixture_overrides`]' keys are the normal
    /// theme's.
    #[test]
    fn the_packs_fixture_keys_are_the_normal_themes_colours() {
        let pack = crate::embedded_pack::test_default_pack();
        let normal = crate::theme::theme_by_name("normal").expect("theme");
        for (key, pixel) in fixture_overrides(normal) {
            assert_eq!(pack.palette().get(key), Some(pixel), "key {key:?}");
        }
    }

    /// A theme's fixtures take its colours.
    #[test]
    #[cfg(feature = "density-art")]
    fn a_recolour_rethemes_the_fixture_art() {
        let pack = crate::embedded_pack::test_default_pack();
        let art = pack
            .animation("fish_tank@4x")
            .and_then(|a| a.recolorable(0))
            .expect("the aquarium art");
        let water = pack.palette().get('Д').flatten().expect("the water");
        let plain = art.recolored(&[]);
        let (x, y) = (0..plain.height())
            .flat_map(|y| (0..plain.width()).map(move |x| (x, y)))
            .find(|&(x, y)| plain.get(x, y) == Some(&Some(water)))
            .expect("the art draws its water");
        let [a, b] = ["normal", "cyberpunk"].map(|name| {
            let theme = crate::theme::theme_by_name(name).expect("theme");
            art.recolored(&fixture_overrides(theme))
        });
        assert_ne!(a.get(x, y), b.get(x, y), "the water kept the pack's colour");
    }

    /// A recolour re-derives the shading: one shaded cell of the vending art
    /// is a different colour in two themes whose appliance bodies differ.
    #[test]
    #[cfg(feature = "density-art")]
    fn a_recolour_reshades_the_appliance_art() {
        let pack = crate::embedded_pack::test_default_pack();
        let art = pack
            .animation("vending_machine@4x")
            .and_then(|a| a.recolorable(0))
            .expect("the vending art");
        let [a, b] = ["normal", "cyberpunk"].map(|name| {
            let theme = crate::theme::theme_by_name(name).expect("theme");
            art.recolored(&appliance_overrides(&theme.appliance))
        });
        let shade = pack.palette().get('ъ').flatten().expect("the body's shade");
        let plain = art.recolored(&[]);
        let (x, y) = (0..plain.height())
            .flat_map(|y| (0..plain.width()).map(move |x| (x, y)))
            .find(|&(x, y)| plain.get(x, y) == Some(&Some(shade)))
            .expect("the art draws the body's shade");
        assert_ne!(a.get(x, y), b.get(x, y), "the shade kept the pack's colour");
    }
}
