//! Backend-agnostic status-footer model.
//!
//! The SINGLE source of truth for the office's bottom status line — the
//! `n/total` headcount, the per-state activity rungs (`●◐○◌`), the aggregate
//! tool tally, the gateway chip, and the right-flushed `♩`/floor/keys suffix,
//! with a source-death/decode-drift WARNING tier that preempts the stats. Two
//! painters render it: the TUI (ratatui `Paragraph`) and the floating window (AA
//! Monaspace Neon blitted into its surface); a future web hero could too. `scene`
//! has no terminal/window deps (invariant #1), so the model carries a
//! backend-agnostic [`FooterTone`]; [`footer_tone_rgb`] (here) is the ONE
//! tone→theme-role map both painters share — each only converts the resolved
//! `Rgb` to its own surface color type (ratatui `Color` / packed XRGB), so the
//! hues can't drift across surfaces. This mirrors the [`crate::board`] precedent
//! exactly (`BoardTone`/`tone_rgb`, shared `StateCounts`).
//!
//! [`build_footer`] owns the WHOLE tier/priority policy in one place — the
//! death-tier preempt (with the `▲N need you` alarm pinned through truncation),
//! the widest-that-fits full→medium→minimal ladder, the tool sort+truncate, and
//! the per-state `♩`/floor suffix — so a painter receives a resolved segment list
//! and never re-derives a tier. It is PURE: the one scene read (the tool tally)
//! is extracted to the free feeder [`footer_tool_tally`], so `build_footer`
//! itself is a function of pre-computed inputs, unit-testable with zero fixtures
//! (exactly like [`crate::board::build_board`]).

use std::collections::HashMap;

use pixtuoid_core::sprite::Rgb;
use pixtuoid_core::state::{ActivityState, DaemonState, ToolKind, MAX_FLOORS};
use pixtuoid_core::SceneState;

use crate::board::{gateway_label, StateCounts, GATEWAY_GLYPH};
use crate::theme::Theme;

// --- Shared state vocabulary (glyph + letter + word + count) ------------------
// ONE source for how an activity state reads on every surface (the footer today;
// the binary re-exports this as `StateKind` so the tooltip/dashboard read the
// SAME glyph/letter/word). Each state carries redundant channels so hue is never
// the sole carrier — survives colour removal, colour-blindness, a tofu'd glyph.

/// The four agent activity buckets as a shared vocabulary. `Waiting` owns the
/// reserved amber "needs-you" hue (via [`FooterTone::Rung`] → `label_waiting`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RungKind {
    Active,
    Waiting,
    Idle,
    Exiting,
}

impl RungKind {
    /// Canonical render order (the footer's left-to-right rung order).
    pub const ALL: [RungKind; 4] = [
        RungKind::Active,
        RungKind::Waiting,
        RungKind::Idle,
        RungKind::Exiting,
    ];

    /// A distinct geometric glyph per state — all East-Asian *ambiguous* width
    /// (1 cell in a non-CJK terminal): `●` active, `◐` waiting, `○` idle, `◌`
    /// exiting. The fill gradient IS the language: full=working, half=paused on
    /// you, empty=idle, dotted=leaving. (Every glyph is Monaspace-Neon-native —
    /// the single-face vocabulary gate in the binary's `aa_text`.)
    pub fn glyph(self) -> char {
        match self {
            RungKind::Active => '\u{25cf}',
            RungKind::Waiting => '\u{25d0}',
            RungKind::Idle => '\u{25cb}',
            RungKind::Exiting => '\u{25cc}',
        }
    }

    /// A distinct single letter — the primary colour-blind channel at the
    /// footer's narrow tier where the full word doesn't fit.
    pub fn letter(self) -> char {
        match self {
            RungKind::Active => 'A',
            RungKind::Waiting => 'W',
            RungKind::Idle => 'I',
            RungKind::Exiting => 'x',
        }
    }

    /// The full capitalized state word — the tooltip dossier's state line reads
    /// `{glyph} {word}` (the board uses its own casual `work`/`wait`/`idle`).
    pub fn word(self) -> &'static str {
        match self {
            RungKind::Active => "Active",
            RungKind::Waiting => "Waiting",
            RungKind::Idle => "Idle",
            RungKind::Exiting => "Exiting",
        }
    }

    /// The count for this state — lets a consumer iterate [`RungKind::ALL`] and
    /// pull the matching tally without re-matching (the old `state_count`).
    pub fn count(self, counts: StateCounts) -> usize {
        match self {
            RungKind::Active => counts.active,
            RungKind::Waiting => counts.waiting,
            RungKind::Idle => counts.idle,
            RungKind::Exiting => counts.exiting,
        }
    }
}

// --- The footer model --------------------------------------------------------

/// A footer segment's tone — backend-agnostic. Each painter maps it to its own
/// color (ratatui `Color` in tui, packed XRGB in floating) via
/// [`footer_tone_rgb`]. Deliberately NOT [`crate::board::BoardTone`]: the variant
/// sets are disjoint (the footer shows per-tool + gateway + warning tones the
/// board never does; the board shows Brand/Star the footer never does).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FooterTone {
    /// Labels, separators, counts, padding, the `♩`/floor/keys suffix — muted.
    Neutral,
    /// An activity-state rung — hue by the shared [`RungKind`] vocabulary.
    Rung(RungKind),
    /// A tool tally segment — hue from the TYPED [`ToolKind`], the same
    /// monitor-glow colour the sprite shows, NEVER a re-parse of the name.
    Tool(ToolKind),
    /// The gateway `⬢gw` chip — hue by daemon liveness.
    Gateway(DaemonState),
    /// Source-death / decode-drift warning (#157) — reuses the Waiting attention
    /// color (the nearest themed "needs your eyes"), no dedicated theme key.
    Warning,
}

/// Resolve a [`FooterTone`] to its theme color role — the SINGLE authority both
/// footer painters share (tui `to_color`, floating `pack_xrgb`), so a `theme.ui`
/// role change lands in ONE place and the surfaces can't drift. The model carries
/// the tone; only the output color TYPE differs per surface. Mirrors
/// [`crate::board::tone_rgb`], and reproduces the retired binary
/// `SegRole::color`/`StateKind::color` byte-for-byte (pin-tested binary-side).
pub fn footer_tone_rgb(tone: FooterTone, theme: &Theme) -> Rgb {
    match tone {
        FooterTone::Neutral => theme.ui.label_idle,
        FooterTone::Rung(RungKind::Active) => theme.ui.label_active,
        FooterTone::Rung(RungKind::Waiting) => theme.ui.label_waiting,
        FooterTone::Rung(RungKind::Idle) => theme.ui.label_idle,
        FooterTone::Rung(RungKind::Exiting) => theme.ui.label_exiting,
        FooterTone::Tool(kind) => crate::pixel_painter::tool_glow_for_kind(kind, &theme.tool_glow),
        FooterTone::Gateway(DaemonState::Idle) => theme.ui.label_idle,
        FooterTone::Gateway(DaemonState::Busy) => theme.ui.label_active,
        FooterTone::Gateway(DaemonState::Degraded | DaemonState::Down) => theme.ui.label_waiting,
        FooterTone::Warning => theme.ui.label_waiting,
    }
}

/// One tone-tagged text run of the footer. Painters concatenate/position these;
/// the model bakes in the separators, the right-flush padding, and the suffix, so
/// a painter just lays the runs left-to-right in its own coordinate space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FooterSegment {
    pub text: String,
    pub tone: FooterTone,
}

impl FooterSegment {
    fn new(text: impl Into<String>, tone: FooterTone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }
}

/// The whole footer for one frame — a flat, ordered, tone-tagged run list already
/// right-flushed to `budget` columns (the chosen tier + padding + suffix). A
/// painter renders it with zero policy: recolor each run via [`footer_tone_rgb`]
/// and lay them out. The width is baked to `budget` (the caller's column budget:
/// terminal cells for the TUI, `win_w / advance` for floating) exactly like
/// [`crate::board`] bakes its own per-frame model — so both painters right-flush
/// identically without re-implementing the fit/pad math.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FooterModel {
    pub segments: Vec<FooterSegment>,
}

impl FooterModel {
    /// The concatenated plain text — the free wording oracle (replaces the
    /// binary's `#[cfg(test)]` `build_status_summary`; snapshots + substring
    /// asserts lock the exact footer wording through this).
    pub fn text(&self) -> String {
        self.segments.iter().map(|s| s.text.as_str()).collect()
    }
}

/// The current floor breadcrumb inputs — `current`/`total_floors` drive the
/// `F{c}/{t}` badge, `total_agents` the `n/total` slash. A single-floor office
/// passes `None` (bare count, no breadcrumb) — so floating's single-floor scene
/// omits it by construction, not a special case.
#[derive(Debug, Clone, Copy)]
pub struct FooterFloor {
    pub current: usize,
    pub total_floors: usize,
    pub total_agents: usize,
}

/// One aggregate tool-tally entry: the raw display `token` (kept verbatim), the
/// TYPED [`ToolKind`] for the hue, and how many Active slots show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolTally {
    pub token: String,
    pub kind: ToolKind,
    pub count: usize,
}

/// The aggregate tool tally: group Active slots by their raw display token (the
/// first alphanumeric run of the detail, kept verbatim) but carry the TYPED
/// [`ToolKind`] for the hue — a Task slot displays "Delegating" yet tints via
/// `kind = Task`, never the name. Sorted by count desc then name, capped at 4.
/// The ONE scene read `build_footer` needs — extracted here so `build_footer`
/// stays a pure function of pre-computed inputs (the [`crate::board::scene_stats`]
/// discipline).
pub fn footer_tool_tally(scene: &SceneState) -> Vec<ToolTally> {
    let mut tool_counts: HashMap<String, (ToolKind, usize)> = HashMap::new();
    for slot in scene.agents.values() {
        if let ActivityState::Active { detail, kind, .. } = &slot.state {
            if let Some(token) = detail
                .as_deref()
                .and_then(|d| d.split(|c: char| !c.is_alphanumeric()).next())
                .filter(|t| !t.is_empty())
            {
                tool_counts.entry(token.to_string()).or_insert((*kind, 0)).1 += 1;
            }
        }
    }
    let mut tools: Vec<ToolTally> = tool_counts
        .into_iter()
        .map(|(token, (kind, count))| ToolTally { token, kind, count })
        .collect();
    tools.sort_by(|a, b| b.count.cmp(&a.count).then(a.token.cmp(&b.token)));
    tools.truncate(4);
    tools
}

/// The pre-computed per-frame inputs `build_footer` renders. Assembled once per
/// frame by each painter. `counts` is the CURRENT (projected) floor's per-state
/// breakdown (the rungs); `per_floor` + `gateway` are office-wide (the cross-floor
/// `▲F{n}` cue + the `⬢gw` chip); `tools` is [`footer_tool_tally`] (precomputed
/// for purity). `source_warning` is the pre-merged death>drift string (the binary
/// owns that merge; `None` = healthy, and floating passes `None` until it threads
/// the health channel). `keys_stats`/`keys_alert` are the painter-supplied
/// keybind tails (the TUI's `[?]help…[q]uit` / `[q]uit`; floating supplies its own).
pub struct FooterInputs<'a> {
    pub counts: StateCounts,
    pub per_floor: &'a [StateCounts; MAX_FLOORS],
    pub gateway: Option<DaemonState>,
    pub floor: Option<FooterFloor>,
    pub tools: &'a [ToolTally],
    /// "You would hear sound right now": audio live AND not effectively muted
    /// (m-state OR pause). Drives the `♩` suffix glyph.
    pub audio_audible: bool,
    /// Transient +/- readout: `Some(percent)` for ~1s after a volume nudge —
    /// renders as `♩ N%`.
    pub volume_flash: Option<u8>,
    /// Pre-merged one-line death>drift warning (#157); `None` while healthy.
    pub source_warning: Option<&'a str>,
    /// The stats-tier right keybind tail (TUI: `" [?]help [p]ause [t]heme [q]uit "`).
    pub keys_stats: &'a str,
    /// The alert-tier right keybind tail (TUI: `" [q]uit "`).
    pub keys_alert: &'a str,
}

/// Column width of a footer string. The footer's own vocabulary — the glyphs
/// `·×↑↓●◐○◌⬢▲♩⚠` and the ASCII warning/keys — is ALL single-column (ambiguous
/// EAW = 1 in a non-CJK terminal), so `chars().count()` equals the display width —
/// keeping `unicode-width` OUT of `scene` (the [`crate::board`] discipline). A
/// binary-side parity test pins `chars().count() == unicode-width` over the full
/// glyph vocabulary so a future non-single-column glyph can't silently shift the
/// flush. **Accepted residual** (the board makes the identical single-column bet
/// for its content): the ONE variable-content field is the tool-tally TOKEN
/// (`footer_tool_tally`, a raw agent-supplied alphanumeric run) — every real
/// source's tool names are ASCII, but a hypothetical wide-CJK token would count
/// short of its display width and nudge the right-flush by the excess. Not worth
/// pulling `unicode-width` into `scene` for; if a wide token ever appears in the
/// wild, that's the trigger to reconsider (not a silent bug — it over-runs the
/// suffix visibly).
fn cols(s: &str) -> usize {
    s.chars().count()
}

/// Assemble the footer for one frame — the deep builder that owns the entire
/// tier/priority policy. `budget` is the caller's column budget (terminal cells
/// for the TUI, `win_w / advance` for floating). Returns the chosen tier already
/// right-flushed to `budget`, so a painter renders the runs with zero policy.
///
/// Tiers (state rungs carry glyph+count+letter — see [`RungKind`]):
///   * **death** (preempts all) — `⚠ {warn}` with the `▲N need you` alarm PINNED
///     through body truncation (the one must-not-miss datum), + `keys_alert`.
///   * **full** — `n/total`, a rung per non-zero state incl. a first-class
///     Exiting rung, the tool tally, and the gateway chip.
///   * **medium** — compact rungs for Active/Waiting/Idle only.
///   * **minimal** — the waiting alarm LEADS, then the count.
///   * **fallback** — only the keybind tail.
pub fn build_footer(inputs: &FooterInputs<'_>, budget: u16) -> FooterModel {
    let counts = inputs.counts;
    // A dead source outranks the stats (#157): the counts go stale once a
    // transport is gone, so the warning IS the status until restart. It survives
    // every width — truncated to fit rather than tiered away — but the waiting
    // ALARM (`▲N need you`) rides along, the one must-not-miss datum even in a
    // partially-frozen office (design DEATH tier).
    if let Some(warn) = inputs.source_warning {
        let w = budget as usize;
        let quit = inputs.keys_alert;
        let avail = w.saturating_sub(cols(quit));
        let alarm = if counts.waiting > 0 {
            format!(" · \u{25b2}{} need you", counts.waiting)
        } else {
            String::new()
        };
        let prefix = " \u{26a0} ";
        let suffix = " ";
        let full = format!("{prefix}{warn}{alarm}{suffix}");
        let text = if cols(&full) <= avail {
            full
        } else {
            let chrome = cols(prefix) + cols(suffix) + cols(&alarm);
            let body_budget = avail.saturating_sub(chrome);
            if body_budget >= 1 {
                let mut body: String = warn.chars().take(body_budget.saturating_sub(1)).collect();
                body.push('\u{2026}');
                format!("{prefix}{body}{alarm}{suffix}")
            } else {
                // Too narrow even for the pinned alarm — truncate the whole line.
                let mut t: String = full.chars().take(avail.saturating_sub(1)).collect();
                t.push('\u{2026}');
                t
            }
        };
        let pad = w.saturating_sub(cols(&text) + cols(quit));
        let mut out = vec![FooterSegment::new(text, FooterTone::Warning)];
        if pad > 0 {
            out.push(FooterSegment::new(" ".repeat(pad), FooterTone::Neutral));
        }
        out.push(FooterSegment::new(quit.to_string(), FooterTone::Neutral));
        return FooterModel { segments: out };
    }

    // `n/total` — the floor's own agent count over the office total (== the sum
    // of the per-floor tallies). Single-floor offices show just the count.
    let count_str = match inputs.floor {
        Some(fi) => format!("{}/{}", counts.total, fi.total_agents),
        None => format!("{}", counts.total),
    };

    // Floor breadcrumb + the cross-floor `▲F{n}` cue: any OTHER floor holding a
    // waiting agent. Rides the right-flushed suffix so it's present at every tier
    // that keeps the suffix.
    let cross_floor = inputs.floor.and_then(|fi| {
        let cur = fi.current.saturating_sub(1);
        (0..MAX_FLOORS)
            .find(|&fl| fl != cur && inputs.per_floor[fl].waiting > 0)
            .map(|fl| fl + 1)
    });
    let floor_suffix = match inputs.floor {
        Some(fi) => {
            let cross = match cross_floor {
                Some(n) => format!(" \u{25b2}F{n}"),
                None => String::new(),
            };
            format!(
                " F{}/{}{cross} [\u{2191}\u{2193}]",
                fi.current, fi.total_floors
            )
        }
        None => String::new(),
    };
    // ♩ rides the right-flushed suffix; silent (the default) shows nothing; a
    // volume nudge appends the percent for a beat.
    let audio_glyph = match (inputs.audio_audible, inputs.volume_flash) {
        (true, Some(pct)) => format!(" \u{2669} {pct}%"),
        (true, None) => " \u{2669}".to_string(),
        (false, _) => String::new(),
    };
    let quit = format!("{audio_glyph}{floor_suffix}{}", inputs.keys_stats);

    // An empty office reads as a bare count on every tier (the board owns the
    // friendly "— office empty —").
    if counts.total == 0 {
        return finish_tier(
            vec![FooterSegment::new(
                format!(" {count_str} "),
                FooterTone::Neutral,
            )],
            &quit,
            budget,
        );
    }

    let seg_full = {
        let mut segs = vec![FooterSegment::new(
            format!(" {count_str}"),
            FooterTone::Neutral,
        )];
        for kind in RungKind::ALL {
            let c = kind.count(counts);
            if c == 0 {
                continue;
            }
            segs.push(FooterSegment::new(" · ".to_string(), FooterTone::Neutral));
            segs.push(FooterSegment::new(
                format!("{}{} {}", kind.glyph(), c, kind.letter()),
                FooterTone::Rung(kind),
            ));
        }
        if !inputs.tools.is_empty() {
            segs.push(FooterSegment::new(" · ".to_string(), FooterTone::Neutral));
            for (i, t) in inputs.tools.iter().enumerate() {
                if i > 0 {
                    segs.push(FooterSegment::new(" ".to_string(), FooterTone::Neutral));
                }
                segs.push(FooterSegment::new(
                    format!("{}\u{d7}{}", t.token, t.count),
                    FooterTone::Tool(t.kind),
                ));
            }
        }
        if let Some(g) = inputs.gateway {
            segs.push(FooterSegment::new(" · ".to_string(), FooterTone::Neutral));
            segs.push(FooterSegment::new(
                format!("{}gw {}", GATEWAY_GLYPH, gateway_label(g)),
                FooterTone::Gateway(g),
            ));
        }
        segs.push(FooterSegment::new(" ".to_string(), FooterTone::Neutral));
        segs
    };

    // Medium: compact rungs for the three resident states (exiting/tools/chip
    // drop out for width); space-separated `{glyph}{count}{letter}`.
    let seg_medium = {
        let mut rungs: Vec<FooterSegment> = Vec::new();
        for kind in [RungKind::Active, RungKind::Waiting, RungKind::Idle] {
            let c = kind.count(counts);
            if c == 0 {
                continue;
            }
            if !rungs.is_empty() {
                rungs.push(FooterSegment::new(" ".to_string(), FooterTone::Neutral));
            }
            rungs.push(FooterSegment::new(
                format!("{}{}{}", kind.glyph(), c, kind.letter()),
                FooterTone::Rung(kind),
            ));
        }
        let mut segs = vec![FooterSegment::new(
            format!(" {count_str} \u{b7} "),
            FooterTone::Neutral,
        )];
        segs.extend(rungs);
        segs.push(FooterSegment::new(" ".to_string(), FooterTone::Neutral));
        segs
    };

    // Minimal: the waiting alarm LEADS (the last stat to survive), then count.
    let seg_min = if counts.waiting > 0 {
        vec![
            FooterSegment::new(
                format!(" \u{25b2}{}", counts.waiting),
                FooterTone::Rung(RungKind::Waiting),
            ),
            FooterSegment::new(format!(" \u{b7} {count_str} "), FooterTone::Neutral),
        ]
    } else {
        vec![FooterSegment::new(
            format!(" {count_str} "),
            FooterTone::Neutral,
        )]
    };

    for tier in [seg_full, seg_medium, seg_min] {
        let stats_len: usize = tier.iter().map(|s| cols(&s.text)).sum();
        if stats_len + cols(&quit) <= budget as usize {
            return finish_tier(tier, &quit, budget);
        }
    }
    FooterModel {
        segments: vec![FooterSegment::new(quit, FooterTone::Neutral)],
    }
}

/// Right-flush a chosen stats tier: pad the gap between it and the fixed keybind
/// suffix so the tail sits at the exact `budget` edge (column-measured).
fn finish_tier(mut tier: Vec<FooterSegment>, quit: &str, budget: u16) -> FooterModel {
    let stats_len: usize = tier.iter().map(|s| cols(&s.text)).sum();
    let pad = (budget as usize).saturating_sub(stats_len + cols(quit));
    if pad > 0 {
        tier.push(FooterSegment::new(" ".repeat(pad), FooterTone::Neutral));
    }
    tier.push(FooterSegment::new(quit.to_string(), FooterTone::Neutral));
    FooterModel { segments: tier }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::state::GlobalDeskIndex;
    use pixtuoid_core::{AgentId, AgentSlot};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::SystemTime;

    // The TUI keybind tails, so the ported tests exercise the exact production
    // wording build_footer will receive from the TUI painter.
    const KEYS_STATS: &str = " [?]help [p]ause [t]heme [q]uit ";
    const KEYS_ALERT: &str = " [q]uit ";

    fn active_slot(id: &str, detail: &str, kind: ToolKind) -> AgentSlot {
        AgentSlot {
            agent_id: AgentId::from_transcript_path(id),
            source: Arc::from("cc"),
            session_id: Arc::from("s"),
            cwd: Arc::from(PathBuf::from("/p").as_path()),
            label: "l".into(),
            state: ActivityState::Active {
                tool_use_id: Some(Arc::from("t")),
                detail: Some(Arc::from(detail)),
                kind,
            },
            state_started_at: SystemTime::UNIX_EPOCH,
            created_at: SystemTime::UNIX_EPOCH,
            last_event_at: SystemTime::UNIX_EPOCH,
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

    fn waiting_slot(id: &str) -> AgentSlot {
        let mut s = active_slot(id, "x", ToolKind::Other);
        s.state = ActivityState::Waiting {
            reason: Arc::from("permission"),
        };
        s
    }

    fn inputs<'a>(
        scene: &SceneState,
        pf: &'a [StateCounts; MAX_FLOORS],
        tools: &'a [ToolTally],
        audio_audible: bool,
        volume_flash: Option<u8>,
        source_warning: Option<&'a str>,
    ) -> FooterInputs<'a> {
        FooterInputs {
            counts: crate::board::scene_stats(scene),
            per_floor: pf,
            gateway: None,
            floor: None,
            tools,
            audio_audible,
            volume_flash,
            source_warning,
            keys_stats: KEYS_STATS,
            keys_alert: KEYS_ALERT,
        }
    }

    // footer_tool_tally's token guard: a detail whose first split token is empty
    // (leading non-alphanumeric) must be SKIPPED, not counted as a tool.
    #[test]
    fn tool_tally_skips_empty_leading_token() {
        let mut scene = SceneState::uniform(16);
        // Leading '/' ⇒ first token after split-on-non-alphanumeric is "".
        let slot = active_slot("/p/lead.jsonl", "/usr/bin/thing", ToolKind::Other);
        scene.agents.insert(slot.agent_id, slot);
        let tools = footer_tool_tally(&scene);
        assert!(
            tools.is_empty(),
            "empty leading token yields no tool: {tools:?}"
        );
        let pf = crate::board::per_floor_counts(&scene);
        let line = build_footer(&inputs(&scene, &pf, &tools, false, None, None), 200).text();
        assert!(!line.contains('\u{00d7}'), "no × tool count: {line}");
        assert!(
            line.contains("\u{25cf}1 A"),
            "active rung still shows: {line}"
        );
    }

    #[test]
    fn audio_suffix_tracks_audibility_and_the_volume_flash() {
        let scene = SceneState::uniform(16);
        let pf = crate::board::per_floor_counts(&scene);
        let go = |audible, flash| {
            build_footer(&inputs(&scene, &pf, &[], audible, flash, None), 200).text()
        };
        assert!(!go(false, None).contains('\u{2669}'), "muted shows no note");
        let line = go(true, None);
        assert!(line.contains('\u{2669}'), "audible shows ♩: {line}");
        assert!(!line.contains('%'), "no percent outside the flash: {line}");
        assert!(
            go(true, Some(65)).contains("\u{2669} 65%"),
            "the flash appends the percent"
        );
        assert!(
            !go(false, Some(65)).contains('\u{2669}'),
            "muted never shows ♩"
        );
    }

    // Tier-selection + padding must measure DISPLAY COLUMNS, not bytes: the full
    // tier carries single-column multi-byte glyphs (·, ×), so the row must fill
    // the full width in columns.
    #[test]
    fn full_tier_fills_the_width_in_columns_with_multibyte_glyphs() {
        let mut scene = SceneState::uniform(16);
        let slot = active_slot("/p/mb.jsonl", "Bash ls", ToolKind::Bash);
        scene.agents.insert(slot.agent_id, slot);
        let pf = crate::board::per_floor_counts(&scene);
        let tools = footer_tool_tally(&scene);
        let width: u16 = 200;
        let model = build_footer(&inputs(&scene, &pf, &tools, false, None, None), width);
        let cols_sum: usize = model.segments.iter().map(|s| cols(&s.text)).sum();
        assert_eq!(cols_sum, width as usize, "fills full width: {model:?}");
        assert!(
            model.segments.iter().any(|s| s.text.contains('\u{00d7}')),
            "full tier with the tool breakdown: {model:?}"
        );
    }

    // DEATH tier: a source-death warning replaces the stats, but the `▲N need you`
    // alarm is the one must-not-miss datum — it must survive to a width so narrow
    // the warning body itself is truncated.
    #[test]
    fn death_tier_pins_the_waiting_alarm_through_the_narrowest_width() {
        let mut scene = SceneState::uniform(16);
        let slot = waiting_slot("/p/wait.jsonl");
        scene.agents.insert(slot.agent_id, slot);
        let pf = crate::board::per_floor_counts(&scene);
        let warn = "transport pixtuoid-hook died: connection refused after 3 retries";
        let line = build_footer(&inputs(&scene, &pf, &[], false, None, Some(warn)), 40).text();
        assert!(
            line.contains("\u{25b2}1 need you"),
            "the ▲N alarm survives body truncation: {line}"
        );
        assert!(
            line.contains('\u{2026}'),
            "the warning body IS truncated at this width (proving the alarm was pinned): {line}"
        );
    }

    #[test]
    fn empty_office_is_a_bare_count() {
        let scene = SceneState::uniform(16);
        let pf = crate::board::per_floor_counts(&scene);
        let line = build_footer(&inputs(&scene, &pf, &[], false, None, None), 200).text();
        assert!(line.contains(" 0 "), "bare zero count: {line}");
        assert!(
            !line.contains('\u{25cf}'),
            "no rungs in an empty office: {line}"
        );
    }
}
