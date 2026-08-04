//! Per-agent palette (shirt / hair / skin) + frame recolor + color math
//! primitives (blend / lerp / mix_lab).

use pixtuoid_core::id::normalize_path_key;
use pixtuoid_core::sprite::format::RECOLOR_KEYS;
use pixtuoid_core::sprite::{Frame, Palette, Pixel, Rgb, RgbBuffer};
use pixtuoid_core::AgentSlot;

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
    }, // light peach (matches base palette S)
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
/// splitmix64 finalizer. NOT `DefaultHasher` — its per-process randomization
/// would flicker colors across runs.
fn cwd_outfit_seed(cwd_norm: &str) -> u64 {
    let folded = cwd_norm
        .bytes()
        .fold(0u64, |h, b| h.wrapping_mul(131).wrapping_add(b as u64));
    let z = (folded ^ (folded >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The outfit-determining seed for `agent`. Extracted so
/// `FrameCache::note_outfit_seed` watches the mid-lifetime cwd backfill through
/// the EXACT unknown-cwd fallback the palette uses; a second copy would drift.
pub(super) fn outfit_seed_for(agent: &AgentSlot) -> u64 {
    if agent.unknown_cwd || agent.cwd.as_os_str().is_empty() {
        agent.agent_id.raw()
    } else {
        cwd_outfit_seed(&normalize_path_key(&agent.cwd.to_string_lossy()))
    }
}

/// Burn-tier ember hair — aliases the flame gradient's deep base so a gradient
/// tweak can't desync the hair from the crown.
const EMBER_HAIR: Rgb = super::effects::FLAME_DEEP;

/// Build the per-agent palette. `Some(glow_tint)` blends the skin toward the
/// monitor glow so a seated agent reads as lit by their screen.
pub(super) fn agent_palette(
    base: &Palette,
    agent: &AgentSlot,
    glow_tint: Option<Rgb>,
    burn: crate::burn::BurnTier,
) -> Palette {
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
    base.with_override('B', Some(outfit.shirt))
        .with_override('H', Some(hair))
        .with_override('S', Some(final_skin))
        .with_override('P', Some(outfit.pants))
}

/// The exhaustive `ToolKind → hue` map. Read by the office monitor glow AND, via
/// re-export, by the binary's footer tool-segment tint, so the footer's colour
/// matches the sprite's screen glow exactly.
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

pub(super) fn recolor_frame(frame: &Frame, pal: &Palette, base_pal: &Palette) -> Frame {
    // Keyed off `RECOLOR_KEYS` (core's single source of truth, the same set
    // `validate_recolor_palette` guards for RGB-uniqueness) so the substitution
    // and the load-time guard can't drift. A `None` base never equals a `Some`
    // pixel, so an absent key naturally substitutes nothing.
    let swaps: Vec<(Pixel, Pixel)> = RECOLOR_KEYS
        .iter()
        .map(|&k| (base_pal.get(k).flatten(), pal.get(k).flatten()))
        .collect();
    let pixels: Vec<Pixel> = frame
        .as_slice()
        .iter()
        .map(|p| match p {
            Some(rgb) => swaps
                .iter()
                .find(|(base, _)| *base == Some(*rgb))
                .map_or(*p, |(_, agent)| *agent),
            None => None,
        })
        .collect();
    Frame::from_pixels(frame.width(), frame.height(), pixels)
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
        tinted,
        Rgb { r: 0, g: 0, b: 0 },
        0.18, // dim: the mascot looks drained
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

/// Per-channel sRGB lerp. Cheap; used for low-strength tints where
/// perceptual error doesn't matter (e.g. agent skin glow).
pub(super) fn blend(a: u8, b: u8, t: f32) -> u8 {
    ((a as f32) * (1.0 - t) + (b as f32) * t)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// [`blend`] on each channel of an `Rgb` triple, with one shared `t`.
pub(super) fn blend_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    Rgb {
        r: blend(a.r, b.r, t),
        g: blend(a.g, b.g, t),
        b: blend(a.b, b.b, t),
    }
}

/// Composite `tint` over the existing buffer pixel at `(x, y)` by `t` — the
/// frosted-glass / haze / overlay primitive.
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

/// Perceptually-correct Lab-space mix between two sRGB colors — twilight
/// (orange → navy) and dim overlays travel through Lab without the muddy
/// desaturated midpoint naive sRGB lerp produces. Slower than [`blend`].
pub(super) fn mix_lab(a: Rgb, b: Rgb, t: f32) -> Rgb {
    use palette::{FromColor, IntoColor, Lab, Mix, Srgb};
    let sa = Srgb::new(a.r as f32 / 255.0, a.g as f32 / 255.0, a.b as f32 / 255.0);
    let sb = Srgb::new(b.r as f32 / 255.0, b.g as f32 / 255.0, b.b as f32 / 255.0);
    let la = Lab::from_color(sa);
    let lb = Lab::from_color(sb);
    let mixed: Srgb = la.mix(lb, t.clamp(0.0, 1.0)).into_color();
    Rgb {
        r: (mixed.red.clamp(0.0, 1.0) * 255.0).round() as u8,
        g: (mixed.green.clamp(0.0, 1.0) * 255.0).round() as u8,
        b: (mixed.blue.clamp(0.0, 1.0) * 255.0).round() as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
