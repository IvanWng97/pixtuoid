//! Backend-agnostic name-badge overlay model — the SINGLE source of truth for
//! "what label, what tone, where", shared by the TUI and floating painters.
//!
//! `scene` has no terminal/window deps (invariant #1), so the model carries an
//! activity-derived `LabelTone` and each painter maps it to its own color type.

use std::collections::HashMap;
use std::time::SystemTime;

use pixtuoid_core::sprite::Rgb;
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentId, SceneState};

use crate::layout::{Layout, Point, DESK_W};
use crate::pixel_painter::character_anchor;
use crate::pose::RouteCtx;
use crate::theme::Theme;

/// The separator between a label's source prefix and its cwd/disambiguation
/// tail (`cc·repo`, `cc·repo·1a2b`). The label is WRITTEN core-side as a bare
/// `·` at THREE sites — `decoder::cwd_basename_label` (the chokepoint),
/// `claude_code::cc_derive_label`'s project-dir fallback, and the reducer's
/// SessionStart back-fill, both of which document their bypass — and a crate
/// boundary keeps this const out of reach of all three, so it must MATCH that
/// char.
const LABEL_SEP: char = '\u{b7}';

/// Buffer rows a badge rises by when its agent sits at a desk that raises its
/// monitor above their head.
///
/// A badge anchors on the sprite TOP, which for a back-turned seat is the head
/// at `desk.y` — exactly where `desk_north.sprite` puts the screen. Without this
/// the label paints over the display the raised art exists to reveal.
///
/// It is a COPY of that sprite's extra height rather than a derivation:
/// [`build_overlay`] takes no `Pack` (three painters share the signature and two
/// of them have no reason to hold one), so the height difference is not
/// reachable here. Rounded UP to an even number of rows because a half-block
/// painter divides this by two to get a cell — an odd lift can round back onto
/// the screen. `desk_north_art_fits_under_the_label_lift` pins both properties
/// against the embedded pack, which is what keeps the copies honest.
const RAISED_MONITOR_LABEL_LIFT: u16 = 4;

/// Activity-derived label tone — backend-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelTone {
    Active,
    Waiting,
    Idle,
    Exiting,
}

/// Resolve a `LabelTone` to its theme color role — the SINGLE authority every
/// label painter shares. The `hovered` near-white highlight is deliberately NOT
/// a `LabelTone`; it stays a per-painter surface choice.
pub fn label_tone_rgb(tone: LabelTone, theme: &Theme) -> Rgb {
    match tone {
        LabelTone::Exiting => theme.ui.label_exiting,
        LabelTone::Active => theme.ui.label_active,
        LabelTone::Waiting => theme.ui.label_waiting,
        LabelTone::Idle => theme.ui.label_idle,
    }
}

/// The source's badge hue for a name-badge label, or `None` when the label has
/// no `LABEL_SEP` prefix (a bare-prefix / cwd-less label like `cx`) or the
/// prefix is unregistered.
pub fn badge_hue(text: &str, theme: &Theme) -> Option<Rgb> {
    text.split_once(LABEL_SEP)
        .and_then(|(prefix, _)| theme.source.by_prefix(prefix))
}

/// One agent name-badge to paint above its sprite. `anchor_px` is in SCENE-buffer
/// pixel space; `text` is already disambiguated + truncated and carries NO ●/▸
/// marker (each painter adds its own).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelElement {
    pub anchor_px: Point,
    pub text: String,
    pub tone: LabelTone,
    pub hovered: bool,
}

/// Raise a badge clear of a raised monitor, when this agent is the one sitting
/// under it.
///
/// Gated on the anchor MATCHING the desk's seated anchor rather than on the pose:
/// the same agent walking past their own desk gets the same `Facing::North` from
/// the layout, and lifting their badge mid-corridor would detach it from them.
fn lift_over_raised_monitor(
    anchor: Point,
    agent: &pixtuoid_core::AgentSlot,
    layout: &Layout,
) -> Point {
    let Some(desk) = layout.home_desk(agent.desk_index.single_floor_local()) else {
        return anchor;
    };
    let facing = layout.desk_facing_at(desk);
    if facing != crate::layout::Facing::North {
        return anchor;
    }
    let seated =
        crate::pixel_painter::seated_anchor_for(desk, crate::layout::CHARACTER_SPRITE_W, facing);
    if anchor != seated {
        return anchor;
    }
    Point {
        x: anchor.x,
        y: anchor.y.saturating_sub(RAISED_MONITOR_LABEL_LIFT),
    }
}

/// Build one `LabelElement` per VISIBLE agent — off-floor agents get no
/// `character_anchor` and are skipped, so labels align 1:1 with the sprites.
pub fn build_overlay(
    scene: &SceneState,
    layout: &Layout,
    now: SystemTime,
    rctx: &mut RouteCtx<'_>,
    hovered: Option<AgentId>,
) -> Vec<LabelElement> {
    let mut label_counts: HashMap<&str, usize> = HashMap::new();
    for agent in scene.agents.values() {
        *label_counts.entry(&*agent.label).or_insert(0) += 1;
    }
    let mut out = Vec::new();
    for agent in scene.agents.values() {
        let Some(anchor) = character_anchor(agent, layout, now, rctx) else {
            continue;
        };
        let anchor = lift_over_raised_monitor(anchor, agent, layout);
        let needs_disambig = label_counts.get(&*agent.label).copied().unwrap_or(0) > 1
            && agent.session_id.chars().count() >= 4;
        let raw: std::borrow::Cow<'_, str> = if needs_disambig {
            let id4 = disambig_suffix(&agent.session_id);
            std::borrow::Cow::Owned(format!("{}{LABEL_SEP}{id4}", agent.label))
        } else {
            std::borrow::Cow::Borrowed(&*agent.label)
        };
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
/// disambiguation suffix. Truncates from the base (left of the `·`), not the
/// suffix — otherwise the disambig becomes useless ("TikTok-Android·a" tells us
/// nothing the base alone wouldn't).
pub(crate) fn truncate_label(label: &str, budget: usize) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    if label.chars().count() <= budget {
        return Cow::Borrowed(label);
    }
    if let Some(sep_byte) = label.rfind(LABEL_SEP) {
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

/// 4-hex-char disambiguation suffix, hashed from the WHOLE `session_id` —
/// shape-agnostic where any slice of the id is not (a raw-cwd id collides on
/// both head and tail: `/x/app` vs `/y/app`), and multi-byte-safe by
/// construction. `DefaultHasher` is per-process: a display aid, not an id.
pub fn disambig_suffix(session_id: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    session_id.hash(&mut h);
    format!("{:04x}", h.finish() & 0xffff)
}

#[cfg(test)]
mod tests {
    use super::{
        badge_hue, build_overlay, disambig_suffix, truncate_label, LabelElement, LabelTone,
    };
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
            pid: None,
            model: None,
            effort: None,
            tokens_used: 0,
            last_usage: None,
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
    fn badge_hue_resolves_a_registered_prefix() {
        let theme = &crate::theme::NORMAL;
        assert!(badge_hue("cc·repo", theme).is_some());
        assert_eq!(badge_hue("cc·repo", theme), theme.source.by_prefix("cc"));
    }

    #[test]
    fn badge_hue_is_none_for_a_bare_prefix_without_separator() {
        assert_eq!(badge_hue("cx", &crate::theme::NORMAL), None);
    }

    #[test]
    fn badge_hue_is_none_for_an_unregistered_prefix() {
        assert_eq!(badge_hue("zz·repo", &crate::theme::NORMAL), None);
    }

    #[test]
    fn colliding_labels_get_disambig_suffixes() {
        let a = slot("cc", "session-aaaa", 0, active());
        let b = slot("cc", "session-bbbb", 1, active());
        let (ida, idb) = (a.session_id.clone(), b.session_id.clone());
        let s = scene_of(vec![a, b]);
        let els = overlay_of(&s, None);
        assert_eq!(els.len(), 2);
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
        let a = disambig_suffix("/users/me/.gravity/sessions/proj/alpha-01.jsonl");
        let b = disambig_suffix("/users/me/.gravity/sessions/proj/beta-02.jsonl");
        assert_ne!(a, b);
    }

    #[test]
    fn rx_cwd_ids_with_colliding_basenames_get_distinct_suffixes() {
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

    /// The one copied number in this file, checked against the art it copies.
    ///
    /// Two properties, both load-bearing: the lift must COVER the raised art's
    /// extra height (or the badge still lands on the screen), and it must be
    /// EVEN, because a half-block painter divides the anchor by two to get a
    /// terminal cell and an odd lift can round straight back down onto it.
    #[test]
    fn desk_north_art_fits_under_the_label_lift() {
        let pack = crate::embedded_pack::load_sprite_pack(None).expect("embedded pack loads");
        let h = |name: &str| {
            pack.animation(name)
                .and_then(|a| a.frames.first())
                .unwrap_or_else(|| panic!("the embedded pack ships {name}"))
                .height()
        };
        let extra = h("desk_north") - h("desk");
        let lift = super::RAISED_MONITOR_LABEL_LIFT;
        assert!(
            lift >= extra,
            "desk_north rises {extra} rows above desk, but the badge lifts only \
             {lift} — it would paint over the screen"
        );
        assert_eq!(
            lift % 2,
            0,
            "an odd lift can round back onto the monitor once a half-block \
             painter halves the anchor into a terminal cell"
        );
    }
}
