//! `character_frame` is the per-agent recolor both profiles draw through.

use std::time::SystemTime;

use pixtuoid_core::AgentSlot;
use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Frame, Rgb};

use crate::frame_cache::FrameCache;
use crate::sim::{CharacterGlow, outfit_seed_for};

mod colors;
mod hair;

#[cfg(test)]
pub(crate) use colors::{HAIR_KEY, PANTS_KEY, SHIRT_KEY, SKIN_KEY};
pub(crate) use colors::{agent_overrides, tool_glow_tint};
#[cfg(all(test, feature = "density-art"))]
pub(crate) use hair::dress;
pub(crate) use hair::{Dress, dress_for};

/// Which image of a character's art to draw: the part of its
/// [`FrameKey`](crate::frame_cache::FrameKey) the sim's placement decides; the
/// agent, its burn tier and the scale's density key the rest. A request, not
/// the cache's key, which is published and keys the image it made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpritePose {
    pub(crate) anim_name: &'static str,
    pub(crate) frame_idx: usize,
    /// Which way the character FACES: the art mirrored horizontally.
    pub(crate) flip_x: bool,
    /// The monitor glow blended into the skin by [`agent_overrides`], so a row
    /// of typing agents shows their tools at a glance; `None` unless typing or
    /// thinking at a desk.
    pub(crate) glow_tint: Option<Rgb>,
}

impl SpritePose {
    /// The pose `placement` draws `agent` in under `theme`. The ONE resolution
    /// of the sim's theme-free [`CharacterGlow`], shared by both profiles: were
    /// each to resolve it, the same agent would glow differently in the two for
    /// no reason anyone chose.
    pub(crate) fn of(
        placement: &crate::sim::CharacterPlacement,
        agent: &AgentSlot,
        theme: &crate::theme::Theme,
    ) -> Self {
        Self {
            anim_name: placement.anim_name,
            frame_idx: placement.frame_idx,
            flip_x: placement.flip_x,
            glow_tint: match placement.glow {
                CharacterGlow::None => None,
                CharacterGlow::Thinking => Some(theme.tool_glow.default),
                CharacterGlow::Tool => tool_glow_tint(agent, &theme.tool_glow),
            },
        }
    }
}

/// The per-agent RECOLORED sprite for one character, and where the art marks a
/// head, DRESSED, from the cache.
///
/// One fn so a second profile gets the identical palette without a second
/// copy of the rule. The ART and the BLIT differ
/// between profiles — the art is [`densest_frame`](crate::embedded_pack::densest_frame)'s at
/// `scale`, so the classic pass (at `RenderScale::ONE`) draws the base sprite
/// 1:1 and the cutaway the densest variant its scale lands — and a per-agent
/// palette is exactly the thing that must NOT differ: hair, skin and the
/// cwd-keyed outfit are how a viewer tells two agents apart, so an agent who is
/// auburn in one profile and default-brown in the other is two different
/// people to the eye.
pub(crate) fn character_frame<'c>(
    pose: SpritePose,
    agent: &AgentSlot,
    pack: &Pack,
    scale: crate::render_scale::RenderScale,
    cache: &'c mut FrameCache,
    now: SystemTime,
) -> Option<CharacterFrame<'c>> {
    let dense = crate::embedded_pack::densest_frame(pack, pose.anim_name, pose.frame_idx, scale)?;
    let key = character_key_at(&dense, pack, pose, agent, now);
    Some(recolor(dense, &key, pack, cache))
}

/// [`character_frame`] for a [`CharacterKey`] resolved at `scale`.
pub(crate) fn keyed_character_frame<'c>(
    key: &CharacterKey,
    pack: &Pack,
    scale: crate::render_scale::RenderScale,
    cache: &'c mut FrameCache,
) -> Option<CharacterFrame<'c>> {
    let dense =
        crate::embedded_pack::densest_frame(pack, key.frame.anim_name, key.frame.frame_idx, scale)?;
    Some(recolor(dense, key, pack, cache))
}

fn recolor<'c>(
    dense: crate::embedded_pack::DenseFrame<'_>,
    key: &CharacterKey,
    pack: &Pack,
    cache: &'c mut FrameCache,
) -> CharacterFrame<'c> {
    let flip_x = key.frame.flip_x;
    // A cwd backfill re-keys the outfit (Team Palette) mid-lifetime — flag the
    // change so the cache drops the agent's stale recolors before the lookup.
    cache.note_outfit_seed(key.frame.agent_id, key.outfit);
    let frame = cache.get_or_make(key.frame.clone(), || {
        let bare = dense.recolorable.recolored(&key.palette);
        // Dressed before the facing flip: a style's layers are drawn for the
        // art as authored. The agent id and the art's density pick the style,
        // and the cache's key carries both.
        let recolored = match &key.dress {
            Some(dress) => hair::dress(
                &bare,
                dress,
                dress
                    .style
                    .as_deref()
                    .and_then(|name| pack.hairstyle(name, dense.density)),
                &key.palette,
                pack.character_outline(),
            ),
            None => bare,
        };
        if flip_x {
            recolored.mirror_horizontal()
        } else {
            recolored
        }
    });
    let rise = key.dress.as_ref().map_or(0, |d| d.rise());
    CharacterFrame {
        frame,
        blit_at: dense.blit_at,
        rise,
    }
}

/// Every input [`character_frame`] recolors from: the cache's key, the outfit
/// seed the cache drops an agent's entries on, and the palette itself. Within
/// one pack, two equal keys recolor to the same frame.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct CharacterKey {
    pub(crate) frame: crate::frame_cache::FrameKey,
    pub(crate) outfit: u64,
    /// The agent's colours, resolved: the recolor reads nothing of the agent
    /// beyond what this and `frame` carry.
    palette: [(char, pixtuoid_core::sprite::Pixel); 4],
    /// How the frame is dressed, resolved once for every reader: the recolor
    /// and the cutaway's figure box.
    pub(crate) dress: Option<Dress>,
}

/// [`character_frame`]'s [`CharacterKey`], without the recolor; `None` where it
/// draws nothing.
pub(crate) fn character_key(
    pose: SpritePose,
    agent: &AgentSlot,
    pack: &Pack,
    scale: crate::render_scale::RenderScale,
    now: SystemTime,
) -> Option<CharacterKey> {
    let dense = crate::embedded_pack::densest_frame(pack, pose.anim_name, pose.frame_idx, scale)?;
    Some(character_key_at(&dense, pack, pose, agent, now))
}

fn character_key_at(
    dense: &crate::embedded_pack::DenseFrame<'_>,
    pack: &Pack,
    pose: SpritePose,
    agent: &AgentSlot,
    now: SystemTime,
) -> CharacterKey {
    let SpritePose {
        anim_name,
        frame_idx,
        flip_x,
        glow_tint,
    } = pose;
    let burn = crate::burn::slot_burn_tier(agent, now);
    let density = dense.density;
    CharacterKey {
        frame: crate::frame_cache::FrameKey {
            agent_id: agent.agent_id,
            anim_name,
            frame_idx,
            flip_x,
            glow_tint,
            burn,
            density,
        },
        outfit: outfit_seed_for(agent),
        palette: agent_overrides(agent, glow_tint, burn),
        dress: dress_for(pack, agent.agent_id, dense.frame, dense.head, density),
    }
}

/// A recolored character frame and how to draw it at the scale it was picked
/// for; `blit_at` as in [`DenseFrame`](crate::embedded_pack::DenseFrame).
pub(crate) struct CharacterFrame<'c> {
    pub(crate) frame: &'c Frame,
    pub(crate) blit_at: std::num::NonZeroU16,
    /// Art rows the frame reaches above its logical top: its
    /// [`Dress::rise`].
    pub(crate) rise: u16,
}

#[cfg(test)]
pub(crate) fn make_slot(
    id: pixtuoid_core::AgentId,
    state: pixtuoid_core::state::ActivityState,
) -> AgentSlot {
    let now = SystemTime::UNIX_EPOCH;
    AgentSlot {
        agent_id: id,
        source: std::sync::Arc::from("claude-code"),
        session_id: std::sync::Arc::from("s"),
        cwd: std::sync::Arc::from(std::path::Path::new("/x")),
        label: "x".into(),
        state,
        state_started_at: now,
        created_at: now,
        last_event_at: now,
        exiting_at: None,
        pending_idle_at: None,

        desk_index: pixtuoid_core::state::GlobalDeskIndex(0),
        floor_idx: 0,
        tool_call_count: 0,
        active_ms: 0,
        unknown_cwd: false,
        parent_id: None,
        pid: None,
        model: None,
        effort: None,
        tokens_used: 0,
        last_usage: None,
    }
}

#[cfg(test)]
pub(crate) fn make_slot_cwd(id_path: &str, cwd: &str, unknown_cwd: bool) -> AgentSlot {
    let id = pixtuoid_core::AgentId::from_transcript_path(id_path);
    let mut s = make_slot(id, pixtuoid_core::state::ActivityState::Idle);
    s.cwd = std::sync::Arc::from(std::path::Path::new(cwd));
    s.unknown_cwd = unknown_cwd;
    s
}

/// `key`'s color for `slot`, unlit and unburnt.
#[cfg(test)]
pub(crate) fn color_of(slot: &AgentSlot, key: char) -> pixtuoid_core::sprite::Pixel {
    override_of(
        &agent_overrides(slot, None, crate::burn::BurnTier::Normal),
        key,
    )
}

/// `key`'s color in an agent's overrides.
#[cfg(test)]
fn override_of(
    overrides: &[(char, pixtuoid_core::sprite::Pixel)],
    key: char,
) -> pixtuoid_core::sprite::Pixel {
    overrides
        .iter()
        .find(|(k, _)| *k == key)
        .unwrap_or_else(|| panic!("no override for {key:?}"))
        .1
}
