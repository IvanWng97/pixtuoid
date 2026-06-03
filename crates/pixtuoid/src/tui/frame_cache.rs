//! Per-agent cache of recolored sprite frames.
//!
//! `recolor_frame` clones a Frame and rewrites pixels — cheap per call,
//! but called once per agent per render tick (~30fps). With N agents the
//! per-second work scales linearly. Since shirt+hair colors are deterministic
//! from agent_id, the recolored frame is stable across the agent's lifetime
//! and can be cached.

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
}

#[derive(Default)]
pub struct FrameCache {
    entries: HashMap<FrameKey, Frame>,
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

    /// Drop cached frames for agents no longer present in the scene.
    pub fn evict_missing(&mut self, scene: &SceneState) {
        self.entries
            .retain(|k, _| scene.agents.contains_key(&k.agent_id));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_frame() -> Frame {
        Frame {
            width: 1,
            height: 1,
            pixels: vec![None],
        }
    }

    fn key() -> FrameKey {
        FrameKey {
            agent_id: AgentId::from_transcript_path("/fc/a.jsonl"),
            anim_name: "standing",
            frame_idx: 0,
            flip_x: false,
            glow_tint: None,
        }
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
}
