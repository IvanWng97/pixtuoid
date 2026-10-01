//! Backend-agnostic name-badge overlay model — the SINGLE source of truth for
//! "what label, what tone, where", shared by the TUI and floating painters.
//!
//! `scene` has no terminal/window deps (invariant #1), so the model carries an
//! activity-derived `LabelTone` and each painter maps it to its own color type.

use std::collections::HashMap;

use pixtuoid_core::sprite::Rgb;
use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentId, AgentSlot, SceneState};

use crate::layout::{DESK_W, Point};
use crate::pixel_painter::AgentFrame;
use crate::theme::Theme;

/// The separator between a label's source prefix and its cwd/disambiguation
/// tail (`cc·repo`, `cc·repo·1a2b`). The label is WRITTEN core-side as a bare
/// `·` at THREE sites — `decoder::cwd_basename_label` (the chokepoint),
/// `claude_code::cc_derive_label`'s project-dir fallback, and the reducer's
/// SessionStart back-fill, both of which document their bypass — and a crate
/// boundary keeps this const out of reach of all three, so it must MATCH that
/// char.
const LABEL_SEP: char = '\u{b7}';

/// Activity-derived label tone — backend-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// One agent name-badge to paint above its sprite. `text` is already
/// disambiguated + truncated and carries NO ●/▸ marker (each painter adds its own).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelElement {
    /// The drawn sprite's [`AgentFrame::label_anchor`], in SCENE-buffer pixels.
    pub anchor_px: Point,
    pub text: String,
    pub tone: LabelTone,
    pub hovered: bool,
}

/// The colours of a badge's two parts, the one decision every painter reads:
/// the name in the activity tone, which every theme holds at text contrast,
/// and the source's identity on the marker, a graphic, since a brand hue as
/// text can fall below that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadgeInk {
    /// The leading `●`: the source's badge hue, else the tone.
    pub marker: Rgb,
    /// The label text: the tone.
    pub name: Rgb,
}

/// The [`BadgeInk`] of a badge reading `text` in `tone`.
pub fn badge_ink(text: &str, tone: LabelTone, theme: &Theme) -> BadgeInk {
    let name = label_tone_rgb(tone, theme);
    BadgeInk {
        marker: badge_hue(text, theme).unwrap_or(name),
        name,
    }
}

/// One `LabelElement` per sprite in `drawn`, in its paint order: an agent the
/// painter did not draw gets no badge.
pub fn build_overlay(
    scene: &SceneState,
    drawn: &[AgentFrame],
    hovered: Option<AgentId>,
) -> Vec<LabelElement> {
    let namesakes = Namesakes::of(scene.agents.values());
    drawn
        .iter()
        .filter_map(|frame| {
            let agent = scene.agents.get(&frame.agent_id)?;
            Some(LabelElement {
                anchor_px: frame.label_anchor,
                text: namesakes.text(agent),
                tone: tone_of(agent),
                hovered: hovered == Some(agent.agent_id),
            })
        })
        .collect()
}

/// How many agents wear each label: an agent sharing its label carries its
/// session's suffix, even where the namesake is not drawn.
pub(crate) struct Namesakes<'a>(HashMap<&'a str, usize>);

impl<'a> Namesakes<'a> {
    pub(crate) fn of(agents: impl IntoIterator<Item = &'a AgentSlot>) -> Self {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for agent in agents {
            *counts.entry(&*agent.label).or_insert(0) += 1;
        }
        Self(counts)
    }

    /// `agent`'s badge text, disambiguated and truncated, with no marker.
    pub(crate) fn text(&self, agent: &AgentSlot) -> String {
        let needs_disambig = self.0.get(&*agent.label).copied().unwrap_or(0) > 1
            && agent.session_id.chars().count() >= 4;
        let raw: std::borrow::Cow<'_, str> = if needs_disambig {
            let id4 = disambig_suffix(&agent.session_id);
            std::borrow::Cow::Owned(format!("{}{LABEL_SEP}{id4}", agent.label))
        } else {
            std::borrow::Cow::Borrowed(&*agent.label)
        };
        const LABEL_BUDGET_PAD: u16 = 4;
        truncate_label(&raw, (DESK_W + LABEL_BUDGET_PAD) as usize).into_owned()
    }
}

/// `agent`'s badge tone: exiting over whatever it last did.
pub(crate) fn tone_of(agent: &AgentSlot) -> LabelTone {
    if agent.exiting_at.is_some() {
        return LabelTone::Exiting;
    }
    match &agent.state {
        ActivityState::Active { .. } => LabelTone::Active,
        ActivityState::Waiting { .. } => LabelTone::Waiting,
        ActivityState::Idle => LabelTone::Idle,
    }
}

/// The plate a badge drawn in pixels sits on, so its text keeps one contrast
/// whatever the room behind it
/// (`every_badge_ink_reads_on_its_plate_in_every_theme`).
pub(crate) fn badge_plate(theme: &Theme) -> Rgb {
    theme.ui.tooltip_bg
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
        LabelElement, LabelTone, badge_hue, build_overlay, disambig_suffix, truncate_label,
    };
    use crate::layout::Point;
    use crate::pixel_painter::AgentFrame;
    use pixtuoid_core::AgentId;
    use pixtuoid_core::state::{ActivityState, AgentSlot, GlobalDeskIndex, SceneState, ToolKind};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
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

    /// `slot`'s sprite as drawn, its badge hung at `at`.
    fn drawn(slot: &AgentSlot, at: Point) -> AgentFrame {
        AgentFrame {
            agent_id: slot.agent_id,
            anchor: at,
            w: 8,
            h: 12,
            label_anchor: at,
        }
    }

    /// Every agent in `scene` drawn, the order immaterial to the test.
    fn overlay_of(scene: &SceneState, hovered: Option<AgentId>) -> Vec<LabelElement> {
        let frames: Vec<_> = scene
            .agents
            .values()
            .map(|a| drawn(a, Point { x: 0, y: 0 }))
            .collect();
        build_overlay(scene, &frames, hovered)
    }

    #[test]
    fn badges_follow_the_drawn_frames_in_paint_order() {
        let a = slot("aa", "sess-aaaa", 0, active());
        let b = slot("bb", "sess-bbbb", 1, active());
        let (at_a, at_b) = (Point { x: 30, y: 9 }, Point { x: 12, y: 40 });
        let frames = [drawn(&b, at_b), drawn(&a, at_a)];
        let s = scene_of(vec![a, b]);
        let els = build_overlay(&s, &frames, None);
        let got: Vec<_> = els.iter().map(|e| (e.text.as_str(), e.anchor_px)).collect();
        assert_eq!(got, [("bb", at_b), ("aa", at_a)]);
    }

    /// The missing-anim case: the painter skips a sprite it has no art for, and its
    /// badge goes with it.
    #[test]
    fn an_agent_the_painter_did_not_draw_gets_no_badge() {
        let a = slot("aa", "sess-aaaa", 0, active());
        let b = slot("bb", "sess-bbbb", 1, active());
        let frames = [drawn(&a, Point { x: 4, y: 4 })];
        let s = scene_of(vec![a, b]);
        let texts: Vec<_> = build_overlay(&s, &frames, None)
            .into_iter()
            .map(|e| e.text)
            .collect();
        assert_eq!(texts, ["aa"]);
    }

    /// Disambiguation is over the scene, not the sprites: an undrawn namesake still
    /// makes the drawn one's badge carry its id.
    #[test]
    fn an_undrawn_namesake_still_disambiguates_the_drawn_badge() {
        let a = slot("cc", "session-aaaa", 0, active());
        let b = slot("cc", "session-bbbb", 1, active());
        let frames = [drawn(&a, Point { x: 4, y: 4 })];
        let want = format!("cc\u{00b7}{}", disambig_suffix(&a.session_id));
        let s = scene_of(vec![a, b]);
        let els = build_overlay(&s, &frames, None);
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].text, want);
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

    /// WCAG 2.2's relative luminance
    /// (<https://www.w3.org/TR/WCAG22/#dfn-relative-luminance>).
    fn luminance(c: pixtuoid_core::sprite::Rgb) -> f64 {
        let linear = |v: u8| {
            let s = f64::from(v) / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
    }

    /// WCAG 2.2's contrast ratio (<https://www.w3.org/TR/WCAG22/#dfn-contrast-ratio>).
    fn contrast(a: pixtuoid_core::sprite::Rgb, b: pixtuoid_core::sprite::Rgb) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// A badge's name is small text, never large-scale, so it holds WCAG 2.2
    /// AA's 4.5:1 (<https://www.w3.org/TR/WCAG22/#contrast-minimum>) on its
    /// plate; its marker is a graphic, held to 3:1
    /// (<https://www.w3.org/TR/WCAG22/#non-text-contrast>). Over every source's
    /// label and a bare one, in every tone: the ink every painter reads. #873
    /// measured an idle badge at 2.05:1 straight on the floor.
    #[test]
    fn every_badge_ink_reads_on_its_plate_in_every_theme() {
        use super::{badge_ink, badge_plate};
        let labels: Vec<String> = pixtuoid_core::source::registry::REGISTRY
            .iter()
            .map(|s| format!("{}\u{b7}repo", s.label_prefix))
            .chain(["bare".to_owned()])
            .collect();
        for theme in crate::theme::ALL_THEMES {
            let plate = badge_plate(theme);
            for tone in [
                LabelTone::Active,
                LabelTone::Waiting,
                LabelTone::Idle,
                LabelTone::Exiting,
            ] {
                for label in &labels {
                    let ink = badge_ink(label, tone, theme);
                    let name = contrast(ink.name, plate);
                    let marker = contrast(ink.marker, plate);
                    let at = format!("{} {tone:?} {label}", theme.name);
                    assert!(name >= 4.5, "{at}: name {name:.2}:1");
                    assert!(marker >= 3.0, "{at}: marker {marker:.2}:1");
                }
            }
        }
    }

    #[test]
    fn multibyte_ids_are_safe_and_deterministic() {
        let a = disambig_suffix("/naïveté/app");
        assert_eq!(a, disambig_suffix("/naïveté/app"));
        assert_eq!(a.len(), 4);
    }
}
