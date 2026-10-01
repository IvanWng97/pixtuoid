//! Each agent's colors: the outfit, hair and skin a character frame is
//! recolored with.

use pixtuoid_core::AgentSlot;
use pixtuoid_core::sprite::{Pixel, Rgb};

use crate::composite::blend_rgb;
use crate::sim::outfit_seed_for;

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

/// A burning agent's hair — an alias of the flame gradient's deep base, so a
/// gradient tweak can't desync the hair from the crown.
const EMBER_HAIR: Rgb = crate::effects::look::FLAME_DEEP;

/// The palette keys a character sprite draws its shirt, hair, skin and pants
/// in: the keys [`agent_overrides`] replaces, so a pack's own sprites take each
/// agent's colors.
pub(crate) const SHIRT_KEY: char = 'B';
/// See [`SHIRT_KEY`].
pub(crate) const HAIR_KEY: char = 'H';
/// See [`SHIRT_KEY`].
pub(crate) const SKIN_KEY: char = 'S';
/// See [`SHIRT_KEY`].
pub(crate) const PANTS_KEY: char = 'P';

/// One agent's colors, as the palette overrides a character frame is
/// recolored with. `Some(glow_tint)` blends the skin toward the monitor glow so
/// a seated agent reads as lit by their screen.
pub(crate) fn agent_overrides(
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

/// The monitor glow color for an agent's active tool, or `None` when the agent
/// is not Active.
pub(crate) fn tool_glow_tint(
    agent: &AgentSlot,
    glow: &crate::theme::ToolGlowColors,
) -> Option<Rgb> {
    use pixtuoid_core::state::ActivityState;
    match &agent.state {
        ActivityState::Active { kind, .. } => Some(glow.for_kind(*kind)),
        _ => None,
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
}
