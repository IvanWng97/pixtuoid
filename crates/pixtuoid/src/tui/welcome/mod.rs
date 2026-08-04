//! Pure model for the first-run onboarding "move-in" overlay — no ratatui.
//!
//! The typewriter/boot timing is elapsed-driven in the painter
//! (`widgets/welcome.rs`), so this model holds no clock — only interactive state.

use crate::install::target::by_source;
use pixtuoid_core::source::registry::descriptor_for;

#[cfg(test)]
mod tests;

/// One roster row = one DETECTED agent CLI the user can opt into connecting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WelcomeRow {
    pub source_id: &'static str,
    /// 2-char badge id (`cc`/`cx`/…) — the same one the dashboard/panel render.
    pub label_prefix: &'static str,
    pub display_name: String,
    pub checked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WelcomeUi {
    pub rows: Vec<WelcomeRow>,
    pub selected: usize,
}

impl WelcomeUi {
    /// Build the roster from detected (present) CLI source ids, all PRE-CHECKED:
    /// the office is empty by definition on a first run, so "connect everything I
    /// have" is the friendly default and the user unchecks what they don't want.
    pub fn from_detected(detected: &[&'static str]) -> Self {
        let rows = detected
            .iter()
            .map(|&sid| WelcomeRow {
                source_id: sid,
                label_prefix: descriptor_for(sid).map_or("??", |d| d.label_prefix),
                display_name: by_source(sid).map_or(sid, |t| t.display_name).to_string(),
                checked: true,
            })
            .collect();
        Self { rows, selected: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.rows.len() {
            self.selected += 1;
        }
    }

    pub fn toggle_selected(&mut self) {
        if let Some(r) = self.rows.get_mut(self.selected) {
            r.checked = !r.checked;
        }
    }

    /// The CONFIRM decision list for `sources::apply_choices`. EVERY row is
    /// written, checked or not: that makes `[sources]` non-empty, so onboarding
    /// never re-triggers.
    pub fn decisions(&self) -> Vec<(&'static str, bool)> {
        self.rows.iter().map(|r| (r.source_id, r.checked)).collect()
    }
}

/// The per-frame render snapshot the event loop hands the renderer. `open` (paint
/// the CARD) is decoupled from `dim` (the office backdrop) so the CLOSE fade-out
/// keeps dimming the office for a beat AFTER the card is gone.
#[derive(Debug, Clone)]
pub struct OnboardingFrame {
    pub open: bool,
    pub rows: Vec<WelcomeRow>,
    pub selected: usize,
    pub elapsed_ms: u64,
    /// Office brightness multiplier: 1.0 = no dim, `DIM_FLOOR` = fully dimmed.
    pub dim: f32,
}

impl Default for OnboardingFrame {
    fn default() -> Self {
        // `dim: 1.0` (NOT the f32 default 0.0, which would render the office black).
        Self {
            open: false,
            rows: Vec::new(),
            selected: 0,
            elapsed_ms: 0,
            dim: 1.0,
        }
    }
}

/// Office brightness the backdrop dims to (0 = black, 1 = unchanged) and the ramp
/// times — the shorter fade-out "lights up" a touch quicker than it went down.
pub const DIM_FLOOR: f32 = 0.4;
pub const DIM_RAMP_MS: u64 = 450;
pub const DIM_FADE_OUT_MS: u64 = 300;

/// Dim factor `elapsed_ms` after the overlay OPENED — ramps `1.0 → DIM_FLOOR`.
pub fn dim_opening(elapsed_ms: u64) -> f32 {
    let t = elapsed_ms.min(DIM_RAMP_MS) as f32 / DIM_RAMP_MS as f32;
    1.0 - t * (1.0 - DIM_FLOOR)
}

/// Dim factor `elapsed_ms` into the CLOSE fade — ramps `from → 1.0`, then `None`
/// once fully restored. `from` is the dim the OPEN ramp was INTERRUPTED at, not
/// `DIM_FLOOR`: an overlay skipped mid-ramp would otherwise snap the whole office
/// darker for a frame before climbing back.
pub fn dim_closing(from: f32, elapsed_ms: u64) -> Option<f32> {
    if elapsed_ms >= DIM_FADE_OUT_MS {
        return None;
    }
    let t = elapsed_ms as f32 / DIM_FADE_OUT_MS as f32;
    Some(from + t * (1.0 - from))
}
