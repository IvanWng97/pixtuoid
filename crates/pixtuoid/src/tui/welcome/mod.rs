//! Pure model for the first-run onboarding "move-in" overlay — no ratatui.
//!
//! Mirrors the `tui::dashboard` / `tui::connection` split: this owns the roster
//! checklist + cursor; the painter is `widgets/welcome.rs`, and `run_tui`
//! sequences the cinematic beats (boot ramp → typewriter card → roster). The
//! typewriter/boot timing is elapsed-driven in the painter (like the dashboard
//! marquee), so this model holds no clock — only the interactive state.

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

/// The onboarding roster: the detected CLIs, a cursor, and per-row checked state.
/// Built once when the overlay opens (from `sources::detect()`), then driven by
/// key input until the user confirms (apply) or skips.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WelcomeUi {
    pub rows: Vec<WelcomeRow>,
    pub selected: usize,
}

impl WelcomeUi {
    /// Build the roster from detected (present) CLI source ids, all PRE-CHECKED
    /// (the office is empty by definition on first run, so "connect everything I
    /// have" is the friendly default — the user unchecks what they don't want).
    /// `label_prefix`/`display_name` resolve from the registry + install target;
    /// a detected source is always target-bearing (`detect()` filters on
    /// `by_source`), so the display name is the target's.
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

    /// The CONFIRM decision list: `(source_id, connect?)` for every row — a checked
    /// row connects, an unchecked row persists disconnected. `sources::apply_choices`
    /// applies it; writing every row (checked or not) makes `[sources]` non-empty so
    /// onboarding never re-triggers, while recording the user's explicit choices.
    pub fn decisions(&self) -> Vec<(&'static str, bool)> {
        self.rows.iter().map(|r| (r.source_id, r.checked)).collect()
    }
}

/// The per-frame render snapshot the event loop hands the renderer — these values
/// always travel together, so they're bundled rather than threaded as parallel
/// fields/params through `TuiRenderer` → `DrawCtx` → `paint_overlays`. The model
/// (`WelcomeUi`) holds no clock; the loop stamps `elapsed_ms` from when the overlay
/// opened and resolves `dim` via the ramp helpers below. `open` (paint the CARD)
/// is decoupled from `dim` (the office backdrop) so the CLOSE fade-out keeps dimming
/// the office for a beat AFTER the card is gone.
#[derive(Debug, Clone)]
pub struct OnboardingFrame {
    /// Paint the welcome card. False during the close fade-out (card gone, office
    /// still fading back up).
    pub open: bool,
    pub rows: Vec<WelcomeRow>,
    pub selected: usize,
    pub elapsed_ms: u64,
    /// Office brightness multiplier for the modal backdrop: 1.0 = no dim,
    /// `DIM_FLOOR` = fully dimmed. Computed by the loop (`dim_opening`/`dim_closing`).
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
/// times — in over `DIM_RAMP_MS` as the overlay opens, back out over the shorter
/// `DIM_FADE_OUT_MS` on close ("lights up" a touch quicker than they went down).
pub const DIM_FLOOR: f32 = 0.4;
pub const DIM_RAMP_MS: u64 = 450;
pub const DIM_FADE_OUT_MS: u64 = 300;

/// Dim factor `elapsed_ms` after the overlay OPENED — ramps `1.0 → DIM_FLOOR`.
pub fn dim_opening(elapsed_ms: u64) -> f32 {
    let t = elapsed_ms.min(DIM_RAMP_MS) as f32 / DIM_RAMP_MS as f32;
    1.0 - t * (1.0 - DIM_FLOOR)
}

/// Dim factor `elapsed_ms` into the CLOSE fade — ramps `DIM_FLOOR → 1.0`, then
/// `None` once fully restored (the caller drops the closing state at `None`).
pub fn dim_closing(elapsed_ms: u64) -> Option<f32> {
    if elapsed_ms >= DIM_FADE_OUT_MS {
        return None;
    }
    let t = elapsed_ms as f32 / DIM_FADE_OUT_MS as f32;
    Some(DIM_FLOOR + t * (1.0 - DIM_FLOOR))
}
