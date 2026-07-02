//! Per-agent palette (shirt / hair / skin) + frame recolor + color math
//! primitives (blend / lerp / bell / mix_lab).
//!
//! `agent_palette` picks the **outfit (shirt + pants) from the agent's normalized cwd**
//! (same working directory → same outfit, for glanceable team/org-chart grouping), while
//! **hair/skin stay per-agent** (`agent_id`-seeded). When `cwd` is unknown or empty, the outfit
//! falls back to the `agent_id` seed. `recolor_frame` rewrites a frame's pixels by RGB-equality
//! against the base pack palette. The color-math helpers live here too because the palette tint
//! code uses them directly and they're widely shared with background/effects.

use pixtuoid_core::source::decoder::normalize_path_key;
use pixtuoid_core::sprite::format::RECOLOR_KEYS;
use pixtuoid_core::sprite::{Frame, Palette, Pixel, Rgb, RgbBuffer};
use pixtuoid_core::AgentSlot;

/// A complete shirt + pants combo. Outfits are **keyed by the agent's normalized
/// working directory** (same cwd → same outfit, so the office reads as a color-coded org-chart),
/// not by per-agent hash. We pick *complete outfits* rather than independent shirt and pants
/// colors so the result is always a harmonious pairing (designed together by someone who knows
/// color) instead of a random clash. Hair/skin stay per-agent for individual distinctness.
/// Sources: Wes Anderson stills, Studio Ghibli character art, modern office capsule-wardrobe palettes.
#[derive(Clone, Copy)]
struct Outfit {
    shirt: Rgb,
    pants: Rgb,
}

/// Warm / extroverted outfits — earthy reds, ochres, terracottas paired
/// with deep neutrals. A warm aesthetic grouping within the 16-preset pool;
/// outfit selection is keyed on `cwd`, not personality (see `agent_palette`).
const OUTFITS_WARM: &[Outfit] = &[
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
];

/// Cool / homebody outfits — sages, slates, indigos paired with deeper
/// neutrals. A cool aesthetic grouping within the 16-preset pool;
/// outfit selection is keyed on `cwd`, not personality (see `agent_palette`).
const OUTFITS_COOL: &[Outfit] = &[
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

/// 8 hair colors — was 5. Added silver/grey for older-coded agents,
/// ginger / strawberry blonde / jet black for more silhouette variety.
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

/// Deterministic seed from a normalized cwd string: byte-fold (cf.
/// `pixel_painter/mod.rs` label hash) then the splitmix64 finalizer used
/// across the scene (`ambient.rs`, `core::id`). No `DefaultHasher` (its
/// per-process randomization would flicker colors across runs), no new dep.
fn cwd_outfit_seed(cwd_norm: &str) -> u64 {
    let folded = cwd_norm
        .bytes()
        .fold(0u64, |h, b| h.wrapping_mul(131).wrapping_add(b as u64));
    let z = (folded ^ (folded >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Build the per-agent palette. `glow_tint` carries the monitor-glow
/// color when the agent is seated at a lit screen (SeatedTyping). The
/// skin blends 18% toward that tint so the eye reads "the monitor is
/// lighting them up." `None` means no glow — skin stays natural.
///
/// The color varies by tool type so scanning a row of typing agents
/// gives an at-a-glance read of what they're working on:
///   green  = generic / default
///   blue   = Edit / Write
///   cyan   = Read
///   orange = Bash
///   purple = Agent / Task
/// The outfit-determining seed for `agent` — the ONE input `agent_palette`
/// keys the shirt+pants (Team Palette) on. Extracted so the frame cache can
/// watch it for the mid-lifetime cwd backfill (`FrameCache::note_outfit_seed`)
/// with the EXACT fallback logic the palette uses — a second copy of the
/// unknown-cwd rule would silently drift.
pub(super) fn outfit_seed_for(agent: &AgentSlot) -> u64 {
    if agent.unknown_cwd || agent.cwd.as_os_str().is_empty() {
        // No usable cwd (hook-only / pre-cwd) => fall back to the per-agent seed
        // so cwd-less agents still get a stable, individually-varied outfit.
        agent.agent_id.raw()
    } else {
        cwd_outfit_seed(&normalize_path_key(&agent.cwd.to_string_lossy()))
    }
}

pub(super) fn agent_palette(base: &Palette, agent: &AgentSlot, glow_tint: Option<Rgb>) -> Palette {
    // Hair/skin stay per-individual (agent_id); only the OUTFIT re-keys on cwd
    // so same-repo agents share a shirt (Team Palette). The old WARM/COOL split
    // was personality-derived (an agent_id artifact cwd-keying breaks anyway);
    // the outfit now spans the full 16-preset pool indexed by the cwd seed.
    let id_seed = agent.agent_id.raw() as usize;
    let outfit_seed = outfit_seed_for(agent);
    let pool_len = OUTFITS_WARM.len() + OUTFITS_COOL.len();
    let i = (outfit_seed as usize) % pool_len;
    let outfit = if i < OUTFITS_WARM.len() {
        OUTFITS_WARM[i]
    } else {
        OUTFITS_COOL[i - OUTFITS_WARM.len()]
    };
    let hair = HAIR_PRESETS[(id_seed / 7) % HAIR_PRESETS.len()];
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

/// Map an agent's active tool kind to a monitor glow color.
/// Returns `None` for non-Active states (no glow). Matches the typed
/// `ToolKind` carried in the slot (derived once at slot entry by the
/// reducer) — no per-frame re-parse of the human-facing `detail` string.
/// The match is exhaustive on purpose: a new `ToolKind` variant must
/// consciously pick a color here.
pub(super) fn tool_glow_tint(
    agent: &AgentSlot,
    glow: &crate::theme::ToolGlowColors,
) -> Option<Rgb> {
    use pixtuoid_core::state::{ActivityState, ToolKind};
    let kind = match &agent.state {
        ActivityState::Active { kind, .. } => *kind,
        _ => return None,
    };
    Some(match kind {
        ToolKind::Edit => glow.edit,
        ToolKind::Read => glow.read,
        ToolKind::Bash => glow.bash,
        ToolKind::Task => glow.agent,
        ToolKind::Search => glow.grep,
        ToolKind::Other => glow.default,
    })
}

pub(super) fn recolor_frame(frame: &Frame, pal: &Palette, base_pal: &Palette) -> Frame {
    // The base->agent color swap per recolor key, resolved ONCE (not per pixel).
    // Keyed off `RECOLOR_KEYS` (core's single source of truth, the same set
    // `validate_recolor_palette` guards for RGB-uniqueness) so the substitution
    // and the load-time guard can't drift. A `None` base never equals a `Some`
    // pixel, so a transparent/absent key naturally substitutes nothing.
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

/// Map one mascot pixel to its "degraded" look (#317): a gateway that is UP but
/// whose model backend is failing every run reads as UNWELL — drain saturation
/// toward grey, bias toward a dull blood-red, then dim. Transparent stays
/// transparent (handled by `degraded_frame`).
pub(super) fn degraded_pixel(c: Rgb) -> Rgb {
    let lum = ((c.r as f32) * 0.30 + (c.g as f32) * 0.59 + (c.b as f32) * 0.11) as u8;
    let gray = Rgb {
        r: lum,
        g: lum,
        b: lum,
    };
    let desat = blend_rgb(c, gray, 0.55); // drain saturation
    let sick = Rgb {
        r: 150,
        g: 40,
        b: 40,
    };
    let tinted = blend_rgb(desat, sick, 0.45); // bias toward a dull red
    blend_rgb(
        tinted,
        Rgb { r: 0, g: 0, b: 0 },
        0.18, // dim ~18% — the lobster looks drained
    )
}

/// A degraded copy of a mascot frame (#317): every opaque pixel runs through
/// [`degraded_pixel`]; transparency is preserved. Mirrors `recolor_frame`'s
/// pixel-map shape.
pub(super) fn degraded_frame(frame: &Frame) -> Frame {
    let pixels = frame
        .as_slice()
        .iter()
        .map(|&p| p.map(degraded_pixel))
        .collect();
    Frame::from_pixels(frame.width(), frame.height(), pixels)
}

// --- Color math primitives -----------------------------------------------

/// Bell curve centered at `c` with half-width `w` (so the bell is 0 at
/// `c ± w` and 1 at `c`). Used for dawn/dusk twilight tint.
pub(super) fn bell(x: f32, c: f32, w: f32) -> f32 {
    let d = (x - c) / w;
    (1.0 - d * d).max(0.0)
}

/// Per-channel sRGB lerp. Cheap; used for low-strength tints where
/// perceptual error doesn't matter (e.g. agent skin glow).
pub(super) fn blend(a: u8, b: u8, t: f32) -> u8 {
    ((a as f32) * (1.0 - t) + (b as f32) * t)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// Per-channel sRGB blend toward `b` by `t` — the `Rgb { r, g, b }` triple
/// (`blend` on each channel, one shared `t`) written once. Cheap; use `mix_lab`
/// where the perceptual difference is visible.
pub(super) fn blend_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    Rgb {
        r: blend(a.r, b.r, t),
        g: blend(a.g, b.g, t),
        b: blend(a.b, b.b, t),
    }
}

/// Composite `tint` over the existing buffer pixel at `(x, y)` by `t`. The
/// frosted-glass / haze / overlay primitive (was glass.rs's private `glass_over`).
pub(super) fn blend_over(buf: &RgbBuffer, x: u16, y: u16, tint: Rgb, t: f32) -> Rgb {
    blend_rgb(buf.get(x, y), tint, t)
}

/// Perceptually-correct Lab-space mix between two sRGB colors. Twilight
/// (orange → navy) and dim overlays travel cleanly through Lab without the
/// muddy desaturated midpoint that naive sRGB lerp produces. Slower than
/// `blend()` but only used where the perceptual difference is visible.
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
