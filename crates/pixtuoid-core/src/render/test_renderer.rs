use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::Result;

use crate::sprite::format::Pack;
use crate::state::SceneState;

/// Captures every SceneState handed to it. Used in e2e tests.
#[derive(Clone, Default)]
pub struct TestRenderer {
    pub snapshots: Arc<Mutex<Vec<SceneState>>>,
}

impl TestRenderer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn count(&self) -> usize {
        self.snapshots.lock().unwrap().len()
    }
    /// Direct snapshot capture — avoids the test having to construct a dummy
    /// `Pack` just to call [`Self::render`].
    pub fn record(&mut self, scene: &SceneState) {
        self.snapshots.lock().unwrap().push(scene.clone());
    }

    /// Inherent render — was the legacy `Renderer` trait impl (retired #483).
    /// Kept with the full 4-arg signature so the e2e harness drives the same
    /// shape as the real `TuiRenderer::render`.
    pub fn render(&mut self, scene: &SceneState, _pack: &Pack, _now: SystemTime) -> Result<()> {
        self.snapshots.lock().unwrap().push(scene.clone());
        Ok(())
    }
}
