//! Backend-agnostic name-badge overlay model.
//!
//! The SINGLE source of truth for "what label, what tone, where" so both
//! painters — the TUI (ratatui `Paragraph`) and the floating window (an 8px
//! pixel font) — render the same name badges from one model and can't drift.
//! `scene` has no terminal/window deps (invariant #1), so the per-painter color
//! mapping (ratatui `Color` vs `Rgb`) stays in each painter; the model only
//! carries an activity-derived `LabelTone`.

use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentId, SceneState};

use crate::layout::{Layout, Point, DESK_W};
use crate::pixel_painter::character_anchor;
use crate::pose::RouteCtx;

/// Activity-derived label tone — backend-agnostic. Each painter maps it to its own
/// color (ratatui `Color` in tui, `Rgb` in floating). Mirrors the TUI's color tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelTone {
    Active,
    Waiting,
    Idle,
    Exiting,
}

/// One agent name-badge to paint above its sprite. `anchor_px` is the character anchor in
/// SCENE-buffer pixel space (what `character_anchor` returns); each painter converts to its
/// own coords. `text` is the final string WITHOUT the ●/▸ marker (the painter adds its own),
/// already disambiguated + truncated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelElement {
    pub anchor_px: Point,
    pub text: String,
    pub tone: LabelTone,
    pub hovered: bool,
}

/// Build one `LabelElement` per VISIBLE agent (those `character_anchor` places on this
/// layout — off-floor agents return `None` and are skipped, so labels align 1:1 with the
/// rendered sprites). The SINGLE source of truth for "what label, what tone, where," so the
/// tui and floating surfaces can't drift.
pub fn build_overlay(
    scene: &SceneState,
    layout: &Layout,
    now: SystemTime,
    rctx: &mut RouteCtx<'_>,
    hovered: Option<AgentId>,
) -> Vec<LabelElement> {
    let agents: Vec<_> = scene.agents.values().cloned().collect();
    let mut label_counts: HashMap<&str, usize> = HashMap::new();
    for agent in &agents {
        *label_counts.entry(&*agent.label).or_insert(0) += 1;
    }
    let mut out = Vec::new();
    for agent in &agents {
        let Some(anchor) = character_anchor(agent, layout, now, rctx) else {
            continue;
        };
        let needs_disambig = label_counts.get(&*agent.label).copied().unwrap_or(0) > 1
            && agent.session_id.chars().count() >= 4;
        let raw: std::borrow::Cow<'_, str> = if needs_disambig {
            let id4 = disambig_suffix(&agent.session_id);
            std::borrow::Cow::Owned(format!("{}·{id4}", agent.label))
        } else {
            std::borrow::Cow::Borrowed(&*agent.label)
        };
        // Label width budget: the desk width plus this much slack before truncation.
        const LABEL_BUDGET_PAD: u16 = 4;
        let text = truncate_label(&raw, (DESK_W + LABEL_BUDGET_PAD) as usize).into_owned();
        let tone = if agent.exiting_at.is_some() {
            LabelTone::Exiting
        } else {
            match &agent.state {
                ActivityState::Active { .. } => LabelTone::Active,
                ActivityState::Waiting { .. } => LabelTone::Waiting,
                ActivityState::Idle => LabelTone::Idle,
            }
        };
        out.push(LabelElement {
            anchor_px: anchor,
            text,
            tone,
            hovered: hovered == Some(agent.agent_id),
        });
    }
    out
}

/// Fit a label into `budget` chars without losing the `·xxxx` session-id
/// disambiguation suffix that the reducer appends to colliding cwds.
/// Truncates from the base (left side of the `·`), not from the suffix —
/// otherwise the disambig becomes useless ("TikTok-Android·a" tells us
/// nothing the base alone wouldn't).
pub(crate) fn truncate_label(label: &str, budget: usize) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    if label.chars().count() <= budget {
        return Cow::Borrowed(label);
    }
    if let Some(sep_byte) = label.rfind('\u{00b7}') {
        let suffix = &label[sep_byte..];
        let suffix_len = suffix.chars().count();
        if suffix_len < budget {
            let base = &label[..sep_byte];
            let base_take = budget - suffix_len;
            let truncated: String = base.chars().take(base_take).collect();
            return Cow::Owned(format!("{truncated}{suffix}"));
        }
    }
    Cow::Owned(label.chars().take(budget).collect())
}

/// 4-hex-char disambiguation suffix, hashed from the whole `session_id` —
/// shape-agnostic where any SLICE of the id is not: a session_id can be a
/// UUID (CC/Codex — head and tail both unique), a normalized full transcript
/// path (Antigravity — constant head, varying stem tail), or a raw cwd
/// (Reasonix — labels collide exactly when BASENAMES collide, so head AND
/// tail are both constant: `/x/app` vs `/y/app`). Only a digest of the full
/// string distinguishes every shape. Hashing also sidesteps byte-slice
/// panics on multi-byte ids (e.g. `/naïveté/app`) by construction.
/// (`DefaultHasher` is deterministic within a process — the suffix is a
/// per-frame display aid, not a persisted identifier.)
pub(crate) fn disambig_suffix(session_id: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    session_id.hash(&mut h);
    format!("{:04x}", h.finish() & 0xffff)
}

#[cfg(test)]
mod tests {
    use super::{build_overlay, disambig_suffix, truncate_label, LabelElement, LabelTone};
    use crate::layout::Layout;
    use crate::motion::MotionState;
    use crate::pathfind::AStarRouter;
    use crate::pose::{PoseHistory, RouteCtx};
    use pixtuoid_core::state::{ActivityState, AgentSlot, GlobalDeskIndex, SceneState, ToolKind};
    use pixtuoid_core::walkable::OccupancyOverlay;
    use pixtuoid_core::AgentId;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    fn layout() -> Layout {
        Layout::compute(120, 96, Some(4)).expect("fits")
    }

    fn slot(label: &str, session_id: &str, desk: usize, state: ActivityState) -> AgentSlot {
        AgentSlot {
            agent_id: AgentId::from_transcript_path(&format!("/p/{label}-{session_id}.jsonl")),
            source: Arc::from("claude-code"),
            session_id: Arc::from(session_id),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: label.into(),
            state,
            state_started_at: SystemTime::UNIX_EPOCH,
            last_event_at: SystemTime::UNIX_EPOCH,
            created_at: SystemTime::UNIX_EPOCH,
            exiting_at: None,
            pending_idle_at: None,
            desk_index: GlobalDeskIndex(desk),
            floor_idx: 0,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id: None,
        }
    }

    fn active() -> ActivityState {
        ActivityState::Active {
            tool_use_id: Some(Arc::from("t")),
            detail: Some(Arc::from("Edit")),
            kind: ToolKind::Edit,
        }
    }

    fn scene_of(slots: Vec<AgentSlot>) -> SceneState {
        let mut s = SceneState::uniform(16);
        for slot in slots {
            s.agents.insert(slot.agent_id, slot);
        }
        s
    }

    /// Drive `build_overlay` with a real router/history/motion/overlay stack.
    fn overlay_of(scene: &SceneState, hovered: Option<AgentId>) -> Vec<LabelElement> {
        let l = layout();
        let mut router = AStarRouter::new();
        let occ = OccupancyOverlay::new();
        let mut history = PoseHistory::new();
        let mut motion: HashMap<AgentId, MotionState> = HashMap::new();
        let mut rctx = RouteCtx {
            router: &mut router,
            overlay: &occ,
            history: &mut history,
            motion: &mut motion,
        };
        build_overlay(scene, &l, now(), &mut rctx, hovered)
    }

    #[test]
    fn single_active_agent_yields_bare_label_active_tone_unhovered() {
        let s = scene_of(vec![slot("cc", "sess-abcd", 0, active())]);
        let els = overlay_of(&s, None);
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].text, "cc");
        assert_eq!(els[0].tone, LabelTone::Active);
        assert!(!els[0].hovered);
    }

    #[test]
    fn colliding_labels_get_disambig_suffixes() {
        let a = slot("cc", "session-aaaa", 0, active());
        let b = slot("cc", "session-bbbb", 1, active());
        let (ida, idb) = (a.session_id.clone(), b.session_id.clone());
        let s = scene_of(vec![a, b]);
        let els = overlay_of(&s, None);
        assert_eq!(els.len(), 2);
        // Both carry a `·<id4>` suffix derived from their distinct session ids.
        let want_a = format!("cc\u{00b7}{}", disambig_suffix(&ida));
        let want_b = format!("cc\u{00b7}{}", disambig_suffix(&idb));
        let texts: Vec<&str> = els.iter().map(|e| e.text.as_str()).collect();
        assert!(texts.contains(&want_a.as_str()), "got {texts:?}");
        assert!(texts.contains(&want_b.as_str()), "got {texts:?}");
        assert_ne!(want_a, want_b);
    }

    #[test]
    fn hovered_agent_marks_its_element() {
        let a = slot("cc", "sess-abcd", 0, active());
        let b = slot("cx", "sess-efgh", 1, active());
        let hovered_id = b.agent_id;
        let s = scene_of(vec![a, b]);
        let els = overlay_of(&s, Some(hovered_id));
        let cc = els.iter().find(|e| e.text == "cc").expect("cc present");
        let cx = els.iter().find(|e| e.text == "cx").expect("cx present");
        assert!(!cc.hovered);
        assert!(cx.hovered);
    }

    #[test]
    fn tone_maps_state_and_exiting_overrides_active() {
        let waiting = slot(
            "wa",
            "sess-w",
            0,
            ActivityState::Waiting {
                reason: Arc::from("perm"),
            },
        );
        let idle = slot("id", "sess-i", 1, ActivityState::Idle);
        // Active state but `exiting_at` set ⇒ Exiting tone (override).
        let mut exiting = slot("ex", "sess-e", 2, active());
        exiting.exiting_at = Some(now());

        let s = scene_of(vec![waiting, idle, exiting]);
        let els = overlay_of(&s, None);
        let tone_of = |t: &str| els.iter().find(|e| e.text == t).map(|e| e.tone);
        assert_eq!(tone_of("wa"), Some(LabelTone::Waiting));
        assert_eq!(tone_of("id"), Some(LabelTone::Idle));
        assert_eq!(tone_of("ex"), Some(LabelTone::Exiting));
    }

    #[test]
    fn truncate_label_passes_short_labels_through() {
        assert_eq!(truncate_label("hello", 16), "hello");
    }

    #[test]
    fn truncate_label_preserves_disambig_suffix() {
        let out = truncate_label("TikTok-Android\u{00b7}a09a", 16);
        assert_eq!(out.chars().count(), 16);
        assert!(out.ends_with("\u{00b7}a09a"), "suffix lost: {out}");
        assert!(out.starts_with("TikTok"), "base over-truncated: {out}");
    }

    #[test]
    fn truncate_label_falls_back_to_plain_truncate_when_no_separator() {
        let out = truncate_label("a-very-long-project-name", 8);
        assert_eq!(out, "a-very-l");
    }

    #[test]
    fn truncate_label_plain_take_when_suffix_exceeds_budget() {
        // The disambig suffix ("·abcdefgh") is longer than budget=4, so the
        // suffix-preserving branch can't fit and it falls through to a plain
        // budget-char take from the front.
        let out = truncate_label("x\u{00b7}abcdefgh", 4);
        assert_eq!(out.chars().count(), 4);
        assert_eq!(out, "x\u{00b7}ab");
    }

    #[test]
    fn uuid_ids_get_distinct_suffixes() {
        let a = disambig_suffix("c0f7fb3f-dc9c-47c3-840d-f775dd2855a3");
        let b = disambig_suffix("019ea57d-7fa7-7812-b864-bdcb9b6c7e17");
        assert_ne!(a, b);
        assert_eq!(a.len(), 4);
    }

    #[test]
    fn ag_full_path_ids_get_distinct_suffixes() {
        // Antigravity session_ids are normalized full transcript paths: two
        // same-cwd sessions share the whole prefix; only the stem differs.
        let a = disambig_suffix("/users/me/.gravity/sessions/proj/alpha-01.jsonl");
        let b = disambig_suffix("/users/me/.gravity/sessions/proj/beta-02.jsonl");
        assert_ne!(a, b);
    }

    #[test]
    fn rx_cwd_ids_with_colliding_basenames_get_distinct_suffixes() {
        // The Reasonix shape that defeats ANY slice of the id: labels collide
        // exactly when basenames collide, so both the head and the tail are
        // constant across the collision (`/work/client-x/app` vs `-y/app`).
        let a = disambig_suffix("/work/client-x/app");
        let b = disambig_suffix("/work/client-y/app");
        assert_ne!(a, b);
    }

    #[test]
    fn multibyte_ids_are_safe_and_deterministic() {
        let a = disambig_suffix("/naïveté/app");
        assert_eq!(a, disambig_suffix("/naïveté/app"));
        assert_eq!(a.len(), 4);
    }
}
