//! Per-agent cache of recolored sprite frames.
//!
//! `recolor_frame` clones a Frame and rewrites pixels — cheap per call,
//! but called once per agent per render tick (~30fps). With N agents the
//! per-second work scales linearly. The recolored frame is stable for as
//! long as its palette inputs are: hair/skin are `agent_id`-seeded (fixed
//! for the agent's lifetime), and the OUTFIT is keyed on the agent's cwd
//! (Team Palette) — mutable mid-lifetime via a cwd backfill, which
//! [`FrameCache::note_outfit_seed`] detects to drop the agent's stale
//! entries. With that one invalidation, caching is safe.

use std::collections::hash_map::Entry;
use std::collections::HashMap;

use pixtuoid_core::sprite::{Frame, Rgb};
use pixtuoid_core::{AgentId, SceneState};

/// Cache identity for one recolored frame. `flip_x` is part of the key so
/// mirrored (left-facing) walkers cache separately; `glow_tint` so each
/// monitor-glow color variant caches separately from the base.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct FrameKey {
    pub agent_id: AgentId,
    pub anim_name: &'static str,
    pub frame_idx: usize,
    pub flip_x: bool,
    pub glow_tint: Option<Rgb>,
    /// Burn tier keys the recolor too (ember hair) — a tier flip mid-life
    /// (model switch / effort TTL decay) simply misses to a fresh entry, the
    /// same containment as `glow_tint` variants (evicted with the agent).
    pub burn: crate::burn::BurnTier,
}

#[derive(Default)]
pub struct FrameCache {
    entries: HashMap<FrameKey, Frame>,
    /// Last-seen outfit-determining seed per agent (see `note_outfit_seed`).
    outfit_seeds: HashMap<AgentId, u64>,
}

impl FrameCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Lookup a cached frame by its [`FrameKey`], or compute and insert one and
    /// return a borrow.
    pub fn get_or_make<F: FnOnce() -> Frame>(&mut self, key: FrameKey, compute: F) -> &Frame {
        self.entries.entry(key).or_insert_with(compute)
    }

    /// Record the outfit-determining seed for `id`. The outfit (shirt+pants)
    /// is keyed on the agent's cwd (Team Palette) with an `agent_id` fallback
    /// while the cwd is unknown — and cwd is mutable post-registration (a
    /// hook-first slot heals it on the next `SessionStart`). A seed CHANGE
    /// therefore drops the agent's cached frames, or already-cached poses
    /// would keep the stale outfit for the agent's lifetime while new poses
    /// render the healed one. Callers pass the exact seed the palette derives
    /// (`pixel_painter::palette::outfit_seed_for`), before `get_or_make`.
    pub fn note_outfit_seed(&mut self, id: AgentId, seed: u64) {
        match self.outfit_seeds.entry(id) {
            Entry::Occupied(mut e) => {
                if *e.get() != seed {
                    e.insert(seed);
                    self.entries.retain(|k, _| k.agent_id != id);
                }
            }
            Entry::Vacant(v) => {
                v.insert(seed);
            }
        }
    }

    /// Drop cached frames for agents no longer present in the scene.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        self.entries
            .retain(|k, _| scene.agents.contains_key(&k.agent_id));
        self.outfit_seeds
            .retain(|id, _| scene.agents.contains_key(id));
    }

    /// Test-only inspection seam (entry count). `#[doc(hidden)]`: not part of the
    /// rendering API — the cache is opaque to consumers; only the unit tests read it.
    #[doc(hidden)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Test-only inspection seam (paired with `len` so clippy's `len_without_is_empty`
    /// is satisfied). `#[doc(hidden)]` for the same reason as `len`.
    #[doc(hidden)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_frame() -> Frame {
        Frame::from_pixels(1, 1, vec![None])
    }

    fn key_for(id: AgentId) -> FrameKey {
        FrameKey {
            agent_id: id,
            anim_name: "standing",
            frame_idx: 0,
            flip_x: false,
            glow_tint: None,
            burn: crate::burn::BurnTier::Normal,
        }
    }

    fn key() -> FrameKey {
        key_for(AgentId::from_transcript_path("/fc/a.jsonl"))
    }

    #[test]
    fn new_cache_is_empty_then_populated_after_get_or_make() {
        let mut cache = FrameCache::new();
        assert!(cache.is_empty(), "fresh cache must be empty");
        assert_eq!(cache.len(), 0);

        let _ = cache.get_or_make(key(), dummy_frame);
        assert!(!cache.is_empty(), "cache must be non-empty after a make");
        assert_eq!(cache.len(), 1);

        // A second get_or_make for the SAME key must reuse, not grow the cache.
        let mut computed_again = false;
        let _ = cache.get_or_make(key(), || {
            computed_again = true;
            dummy_frame()
        });
        assert!(!computed_again, "cached key must not recompute");
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn outfit_seed_change_drops_only_that_agents_entries() {
        let a = AgentId::from_transcript_path("/fc/a.jsonl");
        let b = AgentId::from_transcript_path("/fc/b.jsonl");
        let mut cache = FrameCache::new();
        cache.note_outfit_seed(a, 1);
        cache.note_outfit_seed(b, 7);
        let _ = cache.get_or_make(key_for(a), dummy_frame);
        let _ = cache.get_or_make(key_for(b), dummy_frame);
        assert_eq!(cache.len(), 2);

        // Re-noting the SAME seed (the per-frame steady state) keeps the cache.
        cache.note_outfit_seed(a, 1);
        let mut recomputed = false;
        let _ = cache.get_or_make(key_for(a), || {
            recomputed = true;
            dummy_frame()
        });
        assert!(!recomputed, "an unchanged outfit seed must not evict");

        // A CHANGED seed (the cwd backfill) drops a's entries; b's survive.
        cache.note_outfit_seed(a, 2);
        let mut recomputed = false;
        let _ = cache.get_or_make(key_for(a), || {
            recomputed = true;
            dummy_frame()
        });
        assert!(
            recomputed,
            "a changed outfit seed must drop the agent's stale-outfit frames"
        );
        let mut b_recomputed = false;
        let _ = cache.get_or_make(key_for(b), || {
            b_recomputed = true;
            dummy_frame()
        });
        assert!(!b_recomputed, "another agent's cache must be untouched");
    }
}
