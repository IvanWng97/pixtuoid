//! Backend-agnostic neon wall-board model — the SINGLE source of truth for the
//! office's "lit sign": brand + ★ CTA (L1), the mood pulse (L2: the tally and a
//! plain-English line, swapped by a split-flap roll), and the office-context row
//! (L3: uptime · floor · gateway chip), rendered by the TUI, the floating window,
//! and the wasm hero. It also owns [`OfficeMood`], which the sign's light reads.
//!
//! `scene` has no terminal/window deps (invariant #1), so the model carries a
//! backend-agnostic `BoardTone` and `tone_rgb` is the ONE tone→theme-role map all
//! three painters share. Also owns the per-scene activity tally the footer reads.

use std::time::SystemTime;

use pixtuoid_core::sprite::Rgb;
use pixtuoid_core::state::{ActivityState, DaemonPresence, DaemonState, MAX_FLOORS};
use pixtuoid_core::{AgentSlot, SceneState};

use crate::theme::Theme;

/// Per-scene tally of agent activity states, computed once per frame and shared
/// by the footer and the board so the two surfaces can't disagree. `exiting` is a
/// first-class bucket, NOT folded into idle, so the footer's `n/total` counts
/// walkouts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StateCounts {
    pub active: usize,
    pub waiting: usize,
    pub idle: usize,
    pub exiting: usize,
    pub total: usize,
}

/// Add one slot to `c` under the ONE exiting-first bucketing policy: an
/// **exiting** agent counts as `exiting` regardless of its last activity state.
fn bucket_slot(c: &mut StateCounts, slot: &AgentSlot) {
    c.total += 1;
    if slot.exiting_at.is_some() {
        c.exiting += 1;
        return;
    }
    match slot.state {
        ActivityState::Active { .. } => c.active += 1,
        ActivityState::Waiting { .. } => c.waiting += 1,
        ActivityState::Idle => c.idle += 1,
    }
}

/// Bucket every agent in `scene` — the office-wide (or current-projected-floor)
/// tally.
pub fn scene_stats(scene: &SceneState) -> StateCounts {
    let mut c = StateCounts::default();
    for slot in scene.agents.values() {
        bucket_slot(&mut c, slot);
    }
    debug_assert_eq!(c.active + c.waiting + c.idle + c.exiting, c.total);
    c
}

/// Per-floor [`StateCounts`], bucketed by `AgentSlot.floor_idx` (clamped to the
/// last floor). Computed from the FULL scene, deliberately distinct from
/// `scene_stats` on the projected floor — don't derive one from the other.
pub fn per_floor_counts(scene: &SceneState) -> [StateCounts; MAX_FLOORS] {
    let mut floors = [StateCounts::default(); MAX_FLOORS];
    for slot in scene.agents.values() {
        bucket_slot(&mut floors[slot.floor_idx.min(MAX_FLOORS - 1)], slot);
    }
    floors
}

/// The worst-of daemon-liveness rollup for the gateway chip. `None` = no daemon
/// configured (chip suppressed), distinct from `Some(DaemonState::Down)` (a
/// daemon was seen, then died). `DaemonState` has no `Ord`, hence the explicit
/// severity rank.
pub fn gateway_rollup<'a>(
    daemons: impl Iterator<Item = &'a DaemonPresence>,
) -> Option<DaemonState> {
    fn severity(s: DaemonState) -> u8 {
        match s {
            DaemonState::Idle => 0,
            DaemonState::Busy => 1,
            DaemonState::Degraded => 2,
            DaemonState::Down => 3,
        }
    }
    daemons
        .map(|p| p.display_state())
        .max_by_key(|s| severity(*s))
}

/// The oldest in-scene agent's age in seconds — every agent still in the scene
/// (live or walking out; swept ones are gone).
pub fn scene_uptime_secs(scene: &SceneState, now: SystemTime) -> u64 {
    scene
        .agents
        .values()
        .filter_map(|a| now.duration_since(a.created_at).ok())
        .max()
        .unwrap_or_default()
        .as_secs()
}

/// Format a duration in seconds as a compact `"{h}h{m}m"` / `"{m}m"` / `"<1m"`
/// string (no prefix — the board's uptime badge prepends "↑").
pub fn compact_hms(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        "<1m".to_string()
    }
}

/// The board text's tone — backend-agnostic. Deliberately NOT
/// `overlay::LabelTone`: the variant sets are disjoint (labels never show
/// Brand/Star/Dim; the board never shows a per-agent Exiting).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardTone {
    /// L1 brand — `neon_brand`.
    Brand,
    /// L1 ★ Star CTA — `neon_star`.
    Star,
    /// A working/active count — `label_active`.
    Active,
    /// A waiting "needs-you" count — `label_waiting`.
    Waiting,
    /// An idle count — `label_idle`.
    Idle,
    /// Muted context/separator text — `tooltip_dim`.
    Dim,
}

/// Resolve a `BoardTone` to its theme color role — the SINGLE authority the three
/// board painters share, so a `theme.ui` role change lands in ONE place.
pub fn tone_rgb(tone: BoardTone, theme: &Theme) -> Rgb {
    match tone {
        BoardTone::Brand => theme.ui.neon_brand,
        BoardTone::Star => theme.ui.neon_star,
        BoardTone::Active => theme.ui.label_active,
        BoardTone::Waiting => theme.ui.label_waiting,
        BoardTone::Idle => theme.ui.label_idle,
        BoardTone::Dim => theme.ui.tooltip_dim,
    }
}

/// One tone-tagged text run of the board. The model bakes in the inter-segment
/// separators so no painter re-derives them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardSegment {
    pub text: String,
    pub tone: BoardTone,
}

impl BoardSegment {
    fn new(text: impl Into<String>, tone: BoardTone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }
}

/// The whole board, as tone-tagged segments — L1 `brand` + `star`, L2 `mood`,
/// L3 `context`. No baked padding between brand/star (each painter right-flushes
/// in its own coordinate space); the mood + context separators ARE baked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardModel {
    pub brand: BoardSegment,
    pub star: BoardSegment,
    pub mood: Vec<BoardSegment>,
    pub context: Vec<BoardSegment>,
}

/// The board's brand line. It carries NO version: the board is in every committed
/// still, so a version here made each release re-render all of them.
pub const BOARD_BRAND: &str = "pixtuoid";

/// The board's L1 ★ CTA text — the ONE definition every painter renders AND the
/// TUI's `star_hit_rect` measures, so the clickable target can't drift.
pub const BOARD_STAR: &str = "\u{2605} Star";

/// The waiting/active/idle glyphs — one definition for the tally AND the persona
/// line, so the two L2 faces can't drift apart.
const GLYPH_WAITING: char = '\u{25b2}';
const GLYPH_ACTIVE: char = '\u{25cf}';
const GLYPH_IDLE: char = '\u{25cb}';

/// The gateway chip's GLYPH — one definition for the footer chip AND the board
/// context row (`⬢gw ok`).
pub const GATEWAY_GLYPH: char = '\u{2b22}';

/// The `⬢gw` chip's terse liveness word.
pub fn gateway_label(state: DaemonState) -> &'static str {
    match state {
        DaemonState::Idle => "ok",
        DaemonState::Busy => "busy",
        DaemonState::Degraded => "err",
        DaemonState::Down => "down",
    }
}

/// The gateway chip's tone — mirrors the footer's `FooterTone::Gateway` severity
/// map, but returns a plain `BoardTone` so `DaemonState` is only ever an INPUT to
/// the model and never leaks a color out of `scene`.
pub fn gateway_tone(state: DaemonState) -> BoardTone {
    match state {
        DaemonState::Idle => BoardTone::Idle,
        DaemonState::Busy => BoardTone::Active,
        DaemonState::Degraded | DaemonState::Down => BoardTone::Waiting,
    }
}

/// The office's mood — the ONE classification the tally's empty line, the persona
/// line and the sign's light all read, so the words and the glow can't disagree.
/// Exiting agents never count: a walkout isn't the mood.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficeMood {
    /// `waiting` agents are blocked on the user — outranks everything.
    Alert {
        /// How many agents wait.
        waiting: usize,
    },
    /// Nobody waits and `active` agents work.
    Busy {
        /// How many agents work.
        active: usize,
    },
    /// Everyone present is idle.
    Calm,
    /// Nobody is present.
    Empty,
}

impl OfficeMood {
    /// Classify `counts`.
    pub fn of(counts: StateCounts) -> Self {
        if counts.waiting > 0 {
            Self::Alert {
                waiting: counts.waiting,
            }
        } else if counts.active > 0 {
            Self::Busy {
                active: counts.active,
            }
        } else if counts.idle > 0 {
            Self::Calm
        } else {
            Self::Empty
        }
    }
}

/// The board's "mood pulse" tally — one tone-tagged segment per non-zero
/// present state. Exiting agents are absent by design: a walkout isn't the mood.
///
/// The vocabulary is all single-column (the geometric glyphs `▲●○` are East-Asian
/// *ambiguous* = 1 col in a non-CJK terminal, the rest ASCII), so `chars().count()`
/// equals the terminal display width — no `unicode-width` dep in `scene`.
pub fn board_mood_segments(counts: StateCounts) -> Vec<BoardSegment> {
    if OfficeMood::of(counts) == OfficeMood::Empty {
        return vec![BoardSegment::new(
            "\u{2014} office empty \u{2014}",
            BoardTone::Dim,
        )];
    }
    let build = |words: [&str; 3]| -> Vec<BoardSegment> {
        let rows = [
            (counts.waiting, GLYPH_WAITING, words[0], BoardTone::Waiting),
            (counts.active, GLYPH_ACTIVE, words[1], BoardTone::Active),
            (counts.idle, GLYPH_IDLE, words[2], BoardTone::Idle),
        ];
        let mut segs: Vec<BoardSegment> = Vec::new();
        for (n, glyph, word, tone) in rows {
            if n == 0 {
                continue;
            }
            if !segs.is_empty() {
                segs.push(BoardSegment::new("  ", BoardTone::Dim));
            }
            segs.push(BoardSegment::new(format!("{glyph}{n} {word}"), tone));
        }
        segs
    };
    let full = build(["wait", "work", "idle"]);
    let width: usize = full.iter().map(|s| s.text.chars().count()).sum();
    if width <= crate::pixel_painter::NEON_PANEL_INNER_W as usize {
        full
    } else {
        build(["wt", "wk", "id"])
    }
}

/// L2's plain-English pools; `{n}` is the mood's agent count. The `_ONE` pools
/// carry no count, so one agent is never pluralised.
const PERSONA_ALERT_ONE: &[&str] = &["someone needs you!", "psst. your turn."];
const PERSONA_ALERT_MANY: &[&str] = &["{n} agents need you!", "{n} waiting on you..."];
const PERSONA_BUSY_ONE: &[&str] = &["heads down, shipping", "do not disturb :)"];
const PERSONA_BUSY_MANY: &[&str] = &[
    "heads down, shipping",
    "{n} brains at work",
    "do not disturb :)",
];
const PERSONA_CALM: &[&str] = &["quiet... too quiet", "coffee break?"];

/// L2's plain-English face for `mood`; `pick` rotates the pool. Same 1-col
/// vocabulary as the tally (see [`board_mood_segments`]). `None` = L2 stays on the
/// tally: a line too wide for the panel (the tally abbreviates, this can't), and
/// an EMPTY office — its tally already reads in plain English, and the floating
/// window paints an empty office at 1 fps, which would freeze a roll mid-scramble.
fn board_persona_segments(mood: OfficeMood, pick: u64) -> Option<Vec<BoardSegment>> {
    let (glyph, pool, n, tone) = match mood {
        OfficeMood::Alert { waiting: 1 } => {
            (GLYPH_WAITING, PERSONA_ALERT_ONE, 1, BoardTone::Waiting)
        }
        OfficeMood::Alert { waiting } => (
            GLYPH_WAITING,
            PERSONA_ALERT_MANY,
            waiting,
            BoardTone::Waiting,
        ),
        OfficeMood::Busy { active: 1 } => (GLYPH_ACTIVE, PERSONA_BUSY_ONE, 1, BoardTone::Active),
        OfficeMood::Busy { active } => (GLYPH_ACTIVE, PERSONA_BUSY_MANY, active, BoardTone::Active),
        OfficeMood::Calm => (GLYPH_IDLE, PERSONA_CALM, 0, BoardTone::Idle),
        OfficeMood::Empty => return None,
    };
    let line = pool[(pick % pool.len() as u64) as usize].replace("{n}", &n.to_string());
    let text = format!("{glyph} {line}");
    (text.chars().count() <= crate::pixel_painter::NEON_PANEL_INNER_W as usize)
        .then(|| vec![BoardSegment::new(text, tone)])
}

/// L2 alternates in equal halves — tally, persona, tally, … — each HOLDING and
/// then rolling into the next so the roll ends exactly on the hand-over. An even
/// half is the tally, and a whole hour is an even count of halves, so the stills
/// gen-media renders on the hour open on a settled tally.
const FLAP_HALF_MS: u64 = 4_000;
/// A column's flap lands this long into a roll, plus [`FLAP_SETTLE_STEP_MS`] per
/// column to its left — so a roll settles left to right.
const FLAP_SETTLE_BASE_MS: u64 = 140;
const FLAP_SETTLE_STEP_MS: u64 = 24;
/// How long one drum glyph shows.
const FLAP_TICK_MS: u64 = 60;
/// The flap drum, in rolling order. ASCII, so a rolling line keeps the 1-col
/// vocabulary [`board_mood_segments`] depends on.
const FLAP_DRUM: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789#%&*+=<>/?";

/// When column `col` lands, in ms from the roll's start.
fn flap_settle_ms(col: usize) -> u64 {
    FLAP_SETTLE_BASE_MS + col as u64 * FLAP_SETTLE_STEP_MS
}

/// How long a `cols`-wide roll takes: until its LAST column lands.
fn flap_roll_ms(cols: usize) -> u64 {
    flap_settle_ms(cols.saturating_sub(1))
}

fn flap_cells(segs: &[BoardSegment]) -> Vec<(char, BoardTone)> {
    segs.iter()
        .flat_map(|s| s.text.chars().map(move |c| (c, s.tone)))
        .collect()
}

/// `from` rolling into `to`, `since_ms` into the roll. A real flap only rolls
/// FORWARD and stops ON its letter, so a column shows the drum glyphs leading up
/// to its target, never noise; a column that isn't changing doesn't move.
fn flap_roll(
    from: &[(char, BoardTone)],
    to: &[(char, BoardTone)],
    since_ms: u64,
) -> Vec<BoardSegment> {
    const BLANK: (char, BoardTone) = (' ', BoardTone::Dim);
    let drum_len = FLAP_DRUM.len() as u64;
    let mut cells: Vec<(char, BoardTone)> = (0..from.len().max(to.len()))
        .map(|col| {
            let old = from.get(col).copied().unwrap_or(BLANK);
            let new = to.get(col).copied().unwrap_or(BLANK);
            let settle = flap_settle_ms(col);
            // A blank's tone is invisible, so blank-to-blank is "not changing" too.
            let unchanged = old == new || (old.0 == ' ' && new.0 == ' ');
            if since_ms >= settle || unchanged {
                return new;
            }
            // A glyph the drum lacks (▲●○, punctuation) rolls in from a per-column spot.
            let home = FLAP_DRUM
                .iter()
                .position(|&b| b as char == new.0.to_ascii_uppercase())
                .map_or(col as u64 % drum_len, |i| i as u64);
            let flips_left = (settle - since_ms).div_ceil(FLAP_TICK_MS);
            let idx = (home + drum_len - flips_left % drum_len) % drum_len;
            (FLAP_DRUM[idx as usize] as char, BoardTone::Dim)
        })
        .collect();
    while cells.last() == Some(&BLANK) {
        cells.pop();
    }
    let mut segs: Vec<BoardSegment> = Vec::new();
    for (ch, tone) in cells {
        match segs.last_mut() {
            Some(last) if last.tone == tone => last.text.push(ch),
            _ => segs.push(BoardSegment::new(ch.to_string(), tone)),
        }
    }
    segs
}

/// L2 at `now_ms` — see [`FLAP_HALF_MS`] for the timeline.
fn board_mood_at(counts: StateCounts, now_ms: u64) -> Vec<BoardSegment> {
    let tally = board_mood_segments(counts);
    let half = now_ms / FLAP_HALF_MS;
    let Some(persona) = board_persona_segments(OfficeMood::of(counts), half / 2) else {
        return tally;
    };
    let (showing, next) = if half.is_multiple_of(2) {
        (tally, persona)
    } else {
        (persona, tally)
    };
    let (from, to) = (flap_cells(&showing), flap_cells(&next));
    let roll_ms = flap_roll_ms(from.len().max(to.len()));
    match (now_ms % FLAP_HALF_MS + roll_ms).checked_sub(FLAP_HALF_MS) {
        Some(since_ms) => flap_roll(&from, &to, since_ms),
        None => showing,
    }
}

/// Assemble the whole board model. `floor` is `(current, total_floors)` — a
/// single-floor office passes `None`; `gateway` is the [`gateway_rollup`], where
/// `None` suppresses the chip; `now` drives L2's flap. The context separators (`"  "`) are baked into each
/// following segment so painters just concatenate.
pub fn build_board(
    counts: StateCounts,
    uptime_secs: u64,
    floor: Option<(usize, usize)>,
    gateway: Option<DaemonState>,
    now: SystemTime,
) -> BoardModel {
    let mut context = vec![BoardSegment::new(
        format!("\u{2191}{}", compact_hms(uptime_secs)),
        BoardTone::Dim,
    )];
    if let Some((current, total)) = floor {
        context.push(BoardSegment::new(
            format!("  F{current}/{total}"),
            BoardTone::Dim,
        ));
    }
    if let Some(state) = gateway {
        context.push(BoardSegment::new(
            format!("  {GATEWAY_GLYPH}gw {}", gateway_label(state)),
            gateway_tone(state),
        ));
    }
    BoardModel {
        brand: BoardSegment::new(BOARD_BRAND, BoardTone::Brand),
        star: BoardSegment::new(BOARD_STAR, BoardTone::Star),
        mood: board_mood_at(counts, crate::anim::epoch_ms(now)),
        context,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn mood_text(counts: StateCounts) -> String {
        board_mood_segments(counts)
            .into_iter()
            .map(|s| s.text)
            .collect()
    }

    fn counts(active: usize, waiting: usize, idle: usize) -> StateCounts {
        StateCounts {
            active,
            waiting,
            idle,
            exiting: 0,
            total: active + waiting + idle,
        }
    }

    fn text_of(segs: &[BoardSegment]) -> String {
        segs.iter().map(|s| s.text.as_str()).collect()
    }

    fn persona_of(c: StateCounts) -> Vec<BoardSegment> {
        board_persona_segments(OfficeMood::of(c), 0).expect("this office has a persona line")
    }

    /// How long the roll between `c`'s tally and its first persona line takes.
    fn roll_ms(c: StateCounts) -> u64 {
        let tally = text_of(&board_mood_segments(c)).chars().count();
        let persona = text_of(&persona_of(c)).chars().count();
        flap_roll_ms(tally.max(persona))
    }

    /// Where gen-media captures a committed frame, in ms past a whole UTC hour: a
    /// still's own hour, a clip's `poster` second, a wasm still's
    /// `t0_ms + advance_ms`. Read from the manifest, so a new poster time is
    /// covered the day it is added.
    pub(crate) fn committed_frame_offsets_ms() -> Vec<u64> {
        const HOUR_MS: u64 = 3_600_000;
        const MANIFEST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/media.json");
        let raw = std::fs::read_to_string(MANIFEST).expect("scripts/media.json reads");
        let jobs: serde_json::Value = serde_json::from_str(&raw).expect("media.json is JSON");
        let mut offsets = vec![0];
        for job in jobs.as_array().expect("media.json is an array") {
            let base = job["t0_ms"].as_u64().unwrap_or(0) % HOUR_MS;
            if let Some(poster_secs) = job["poster"].as_f64() {
                offsets.push(base + (poster_secs * 1000.0) as u64);
            }
            if let Some(advance_ms) = job["advance_ms"].as_u64() {
                offsets.push(base + advance_ms);
            }
        }
        assert!(
            offsets.len() > 1,
            "the manifest still names poster/advance times"
        );
        offsets
    }

    #[test]
    fn the_brand_carries_no_version_so_a_release_cannot_drift_committed_media() {
        assert_eq!(BOARD_BRAND, "pixtuoid");
        assert!(!BOARD_BRAND.contains(env!("CARGO_PKG_VERSION")));
        let b = build_board(counts(1, 0, 0), 0, None, None, SystemTime::UNIX_EPOCH);
        assert_eq!(b.brand.text, BOARD_BRAND);
    }

    #[test]
    fn mood_is_alert_over_busy_over_calm_over_empty() {
        assert_eq!(
            OfficeMood::of(counts(3, 2, 5)),
            OfficeMood::Alert { waiting: 2 }
        );
        assert_eq!(
            OfficeMood::of(counts(3, 0, 5)),
            OfficeMood::Busy { active: 3 }
        );
        assert_eq!(OfficeMood::of(counts(0, 0, 5)), OfficeMood::Calm);
        assert_eq!(OfficeMood::of(counts(0, 0, 0)), OfficeMood::Empty);
        let walkout_only = StateCounts {
            exiting: 2,
            total: 2,
            ..StateCounts::default()
        };
        assert_eq!(
            OfficeMood::of(walkout_only),
            OfficeMood::Empty,
            "a walkout isn't the mood"
        );
    }

    #[test]
    fn l2_holds_the_tally_then_the_persona_and_rolls_only_between_them() {
        let c = counts(4, 2, 6);
        let tally = board_mood_segments(c);
        let persona = persona_of(c);
        assert_ne!(tally, persona);
        let at = |ms| board_mood_at(c, ms);
        let roll_starts = FLAP_HALF_MS - roll_ms(c);
        assert_eq!(at(0), tally, "an even half OPENS on the settled tally");
        assert_eq!(at(roll_starts - 1), tally, "held until the roll");
        assert_ne!(at(roll_starts + FLAP_TICK_MS), tally, "rolling");
        assert_ne!(at(FLAP_HALF_MS - 1), persona, "last column still rolling");
        assert_eq!(at(FLAP_HALF_MS), persona, "the roll ENDS on the hand-over");
        assert_eq!(
            at(FLAP_HALF_MS + roll_starts - 1),
            persona,
            "held until the roll"
        );
        assert_ne!(
            at(2 * FLAP_HALF_MS - 1),
            tally,
            "last column still rolling back"
        );
        assert_eq!(
            at(2 * FLAP_HALF_MS),
            tally,
            "and the next pair opens settled"
        );
    }

    #[test]
    fn the_persona_pool_rotates_once_per_pair_of_halves() {
        let c = counts(4, 0, 6);
        let shown = |pair: u64| board_mood_at(c, (2 * pair + 1) * FLAP_HALF_MS);
        assert_eq!(
            shown(0),
            board_persona_segments(OfficeMood::of(c), 0).unwrap()
        );
        assert_eq!(
            shown(1),
            board_persona_segments(OfficeMood::of(c), 1).unwrap()
        );
        assert_ne!(shown(0), shown(1));
    }

    #[test]
    fn a_roll_settles_left_to_right_in_dim_and_never_outgrows_the_panel() {
        let c = counts(4, 2, 6);
        let persona = text_of(&persona_of(c));
        let mid = FLAP_HALF_MS - roll_ms(c) / 2;
        let cells: Vec<(char, BoardTone)> = board_mood_at(c, mid)
            .iter()
            .flat_map(|s| s.text.chars().map(move |ch| (ch, s.tone)))
            .collect();
        let settled = persona
            .chars()
            .zip(cells.iter())
            .take_while(|(want, (got, _))| want == got)
            .count();
        assert!(
            settled > 0 && settled < persona.chars().count(),
            "mid-roll the LEFT has landed and the right has not: {settled}"
        );
        let first_rolling = cells[settled];
        assert_eq!(first_rolling.1, BoardTone::Dim, "a rolling flap is dim");
        assert!(
            FLAP_DRUM.contains(&(first_rolling.0 as u8)),
            "a rolling flap shows a drum glyph: {first_rolling:?}"
        );
        for ms in (0..2 * FLAP_HALF_MS).step_by(FLAP_TICK_MS as usize / 2) {
            let w = text_of(&board_mood_at(c, ms)).chars().count();
            assert!(
                w <= crate::pixel_painter::NEON_PANEL_INNER_W as usize,
                "{w} cols at {ms}ms"
            );
        }
    }

    #[test]
    fn the_last_flip_before_a_letter_lands_is_its_drum_predecessor() {
        let c = counts(0, 0, 3);
        let persona = text_of(&persona_of(c));
        let (col, target) = persona
            .chars()
            .enumerate()
            .find(|(_, ch)| ch.is_ascii_alphabetic())
            .expect("a persona line has a letter");
        let just_before = FLAP_HALF_MS - roll_ms(c) + flap_settle_ms(col) - 1;
        let shown = text_of(&board_mood_at(c, just_before))
            .chars()
            .nth(col)
            .expect("the column exists mid-roll");
        let home = FLAP_DRUM
            .iter()
            .position(|&b| b as char == target.to_ascii_uppercase())
            .expect("letters are on the drum");
        let predecessor = FLAP_DRUM[(home + FLAP_DRUM.len() - 1) % FLAP_DRUM.len()] as char;
        assert_eq!(shown, predecessor, "{target:?} lands after {predecessor:?}");
    }

    #[test]
    fn a_column_blank_on_both_sides_stays_blank_through_the_roll() {
        let c = counts(4, 2, 6);
        let tally = text_of(&board_mood_segments(c));
        let persona = text_of(&persona_of(c));
        let cell = |text: &str, col: usize| text.chars().nth(col).unwrap_or(' ');
        let cols = tally.chars().count().max(persona.chars().count());
        // Not the last column: a roll's trailing blanks are trimmed, which would
        // pass this vacuously.
        let col = (0..cols - 1)
            .find(|&col| cell(&tally, col) == ' ' && cell(&persona, col) == ' ')
            .expect("the fixture pair shares an interior blank column");
        let while_it_would_roll = FLAP_HALF_MS - roll_ms(c) + flap_settle_ms(col) / 2;
        let rolling = text_of(&board_mood_at(c, while_it_would_roll));
        assert_eq!(rolling.chars().nth(col), Some(' '));
    }

    #[test]
    fn a_persona_line_fits_the_panel_or_is_withheld() {
        for n in [1usize, 2, 999, usize::MAX] {
            for mood in [
                OfficeMood::Alert { waiting: n },
                OfficeMood::Busy { active: n },
                OfficeMood::Calm,
            ] {
                for pick in 0..8 {
                    let Some(line) = board_persona_segments(mood, pick) else {
                        continue;
                    };
                    let line = text_of(&line);
                    assert!(
                        line.chars().count() <= crate::pixel_painter::NEON_PANEL_INNER_W as usize,
                        "{mood:?}/{pick}: {line:?}"
                    );
                }
            }
        }
        assert!(board_persona_segments(OfficeMood::Busy { active: 999 }, 1).is_some());
        let absurd = OfficeMood::Alert {
            waiting: usize::MAX,
        };
        assert_eq!(
            board_persona_segments(absurd, 0),
            None,
            "withheld, not clipped"
        );
    }

    #[test]
    fn an_office_without_a_persona_never_leaves_the_tally() {
        let absurd = StateCounts {
            waiting: usize::MAX,
            total: usize::MAX,
            ..StateCounts::default()
        };
        for c in [StateCounts::default(), absurd] {
            let tally = board_mood_segments(c);
            for ms in (0..2 * FLAP_HALF_MS).step_by(FLAP_TICK_MS as usize) {
                assert_eq!(board_mood_at(c, ms), tally, "{ms}ms");
            }
        }
    }

    #[test]
    fn one_agent_is_never_pluralised() {
        for pick in 0..8 {
            for mood in [
                OfficeMood::Alert { waiting: 1 },
                OfficeMood::Busy { active: 1 },
            ] {
                let line = text_of(&board_persona_segments(mood, pick).expect("fits"));
                assert!(!line.contains("1 "), "{mood:?}/{pick}: {line:?}");
            }
        }
    }

    /// A committed frame must never catch L2 mid-roll: at every instant gen-media
    /// captures one, even the widest roll has not begun.
    #[test]
    fn no_committed_frame_catches_l2_mid_roll() {
        let widest_roll = flap_roll_ms(crate::pixel_painter::NEON_PANEL_INNER_W as usize);
        for offset in committed_frame_offsets_ms() {
            let into_half = offset % FLAP_HALF_MS;
            assert!(
                into_half + widest_roll < FLAP_HALF_MS,
                "a frame at +{offset}ms lands {into_half}ms into a half — inside the roll"
            );
        }
        assert_eq!(
            board_mood_at(counts(4, 2, 6), 0),
            board_mood_segments(counts(4, 2, 6)),
            "and a whole-hour still shows the tally"
        );
    }

    #[test]
    fn uptime_is_the_oldest_in_scene_agent_in_whole_seconds() {
        use pixtuoid_core::state::{ActivityState, GlobalDeskIndex};
        use pixtuoid_core::AgentId;
        use std::path::PathBuf;
        use std::sync::Arc;
        use std::time::Duration;
        fn slot(secs: u64) -> AgentSlot {
            AgentSlot {
                agent_id: AgentId::from_transcript_path(&format!("/p/{secs}.jsonl")),
                source: Arc::from("claude-code"),
                session_id: Arc::from("s"),
                cwd: Arc::from(PathBuf::from("/p").as_path()),
                label: "l".into(),
                state: ActivityState::Idle,
                state_started_at: SystemTime::UNIX_EPOCH,
                last_event_at: SystemTime::UNIX_EPOCH,
                created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
                exiting_at: None,
                pending_idle_at: None,
                desk_index: GlobalDeskIndex(0),
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
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut scene = SceneState::uniform(16);
        for s in [slot(40), slot(10)] {
            scene.agents.insert(s.agent_id, s);
        }
        assert_eq!(scene_uptime_secs(&scene, now), 90);
        assert_eq!(scene_uptime_secs(&SceneState::uniform(16), now), 0);
    }

    #[test]
    fn mood_echoes_counts_with_waiting_beacon_leading() {
        let c = StateCounts {
            active: 3,
            waiting: 2,
            idle: 5,
            exiting: 1,
            total: 11,
        };
        let segs = board_mood_segments(c);
        let text = mood_text(c);
        assert!(text.contains("\u{25b2}2 wait"), "waiting beacon: {text}");
        assert!(text.contains("\u{25cf}3 work"), "active: {text}");
        assert!(text.contains("\u{25cb}5 idle"), "idle: {text}");
        assert!(!text.contains("exit"), "no exiting on the mood: {text}");
        let w = text.find('\u{25b2}').unwrap();
        let a = text.find('\u{25cf}').unwrap();
        assert!(w < a, "waiting leads active: {text}");
        let tone_of = |glyph: char| {
            segs.iter()
                .find(|s| s.text.starts_with(glyph))
                .map(|s| s.tone)
        };
        assert_eq!(tone_of('\u{25b2}'), Some(BoardTone::Waiting));
        assert_eq!(tone_of('\u{25cf}'), Some(BoardTone::Active));
        assert_eq!(tone_of('\u{25cb}'), Some(BoardTone::Idle));
    }

    #[test]
    fn mood_calm_office_drops_the_beacon() {
        let c = StateCounts {
            active: 1,
            waiting: 0,
            idle: 2,
            exiting: 0,
            total: 3,
        };
        let text = mood_text(c);
        assert!(
            !text.contains('\u{25b2}'),
            "no beacon when nobody waits: {text}"
        );
        assert!(text.contains("\u{25cf}1 work") && text.contains("\u{25cb}2 idle"));
    }

    #[test]
    fn mood_empty_office_reads_plainly_and_is_dim() {
        let segs = board_mood_segments(StateCounts::default());
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "\u{2014} office empty \u{2014}");
        assert_eq!(segs[0].tone, BoardTone::Dim);
    }

    #[test]
    fn mood_big_office_abbreviates_to_fit_the_panel_interior() {
        let c = StateCounts {
            active: 150,
            waiting: 150,
            idle: 150,
            exiting: 0,
            total: 450,
        };
        let text = mood_text(c);
        let width = text.chars().count();
        assert!(
            width <= crate::pixel_painter::NEON_PANEL_INNER_W as usize,
            "fits the panel interior: {text} = {width}"
        );
        assert!(text.contains("\u{25b2}150 wt"), "abbreviated big-N: {text}");
    }

    #[test]
    fn gateway_tone_mirrors_the_footer_severity_map() {
        assert_eq!(gateway_tone(DaemonState::Idle), BoardTone::Idle);
        assert_eq!(gateway_tone(DaemonState::Busy), BoardTone::Active);
        assert_eq!(gateway_tone(DaemonState::Degraded), BoardTone::Waiting);
        assert_eq!(gateway_tone(DaemonState::Down), BoardTone::Waiting);
        assert_eq!(gateway_label(DaemonState::Idle), "ok");
        assert_eq!(gateway_label(DaemonState::Down), "down");
        let theme = crate::theme::theme_by_name("normal").expect("normal theme");
        for st in [
            DaemonState::Idle,
            DaemonState::Busy,
            DaemonState::Degraded,
            DaemonState::Down,
        ] {
            assert_eq!(
                crate::footer::footer_tone_rgb(crate::footer::FooterTone::Gateway(st), theme),
                tone_rgb(gateway_tone(st), theme),
                "board and footer must resolve the same gateway color for {st:?}"
            );
        }
    }

    #[test]
    fn build_board_assembles_all_three_rows() {
        let c = StateCounts {
            active: 2,
            waiting: 0,
            idle: 1,
            exiting: 0,
            total: 3,
        };
        let b = build_board(c, 3661, None, None, SystemTime::UNIX_EPOCH);
        assert_eq!(b.brand.tone, BoardTone::Brand);
        assert_eq!(b.mood, board_mood_segments(c), "the epoch is a whole hour");
        assert_eq!(b.star.text, BOARD_STAR);
        assert_eq!(b.star.tone, BoardTone::Star);
        assert_eq!(b.context.len(), 1);
        assert_eq!(b.context[0].text, "\u{2191}1h1m");
        assert_eq!(b.context[0].tone, BoardTone::Dim);

        let b2 = build_board(
            c,
            30,
            Some((2, 3)),
            Some(DaemonState::Busy),
            SystemTime::UNIX_EPOCH,
        );
        let ctx: String = b2.context.iter().map(|s| s.text.clone()).collect();
        assert_eq!(ctx, "\u{2191}<1m  F2/3  \u{2b22}gw busy");
        let chip = b2.context.last().unwrap();
        assert_eq!(chip.tone, BoardTone::Active, "busy gateway chip tone");
    }
}
