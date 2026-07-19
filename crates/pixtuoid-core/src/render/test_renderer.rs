use std::sync::{Arc, Mutex};

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
    /// Direct snapshot capture — the e2e timeline records each frame's scene.
    pub fn record(&mut self, scene: &SceneState) {
        self.snapshots.lock().unwrap().push(scene.clone());
    }
}
