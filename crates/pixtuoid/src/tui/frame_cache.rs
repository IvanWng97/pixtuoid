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
