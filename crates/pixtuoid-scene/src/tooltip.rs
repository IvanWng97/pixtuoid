//! What a hovered thing's tooltip says, for every painter: tone-tagged rows a
//! painter lays out and colours through [`TipTone::rgb`], as the footer's
//! [`FooterModel`](crate::footer::FooterModel) is. A painter owns only the
//! box, its placement and how a tone draws.

use std::time::SystemTime;

use pixtuoid_core::source::registry::descriptor_for;
use pixtuoid_core::sprite::Rgb;
use pixtuoid_core::state::{ActivityState, ToolKind};
use pixtuoid_core::{AgentId, SceneState};

use crate::display::{GatewayCard, HoverTarget, PetHover};
use crate::floor::FloorInputs;
use crate::footer::{FooterTone, RungKind};
use crate::hit::SceneHit;
use crate::pet::PetKind;
use crate::theme::Theme;

/// The longest tool detail or waiting reason a tooltip shows, in chars.
pub const DETAIL_CHARS: usize = 34;

/// How a tooltip run draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipTone {
    /// The tooltip's subject, drawn bold.
    Title,
    Text,
    Dim,
    /// The painter's own text colour.
    Plain,
    /// An agent's state word.
    Rung(RungKind),
    /// The tool an active agent runs.
    Tool(ToolKind),
    /// A source's `[xx]` badge, in that source's hue.
    Source(&'static str),
}

impl TipTone {
    /// This tone's theme colour, the one authority every tooltip painter
    /// reads; `None` for [`Self::Plain`].
    pub fn rgb(self, theme: &Theme) -> Option<Rgb> {
        Some(match self {
            Self::Title => theme.ui.tooltip_title,
            Self::Text => theme.ui.tooltip_text,
            Self::Dim => theme.ui.tooltip_dim,
            Self::Plain => return None,
            Self::Rung(kind) => FooterTone::Rung(kind).rgb(theme),
            Self::Tool(kind) => theme.tool_glow.for_kind(kind),
            Self::Source(tag) => theme.source_hue(tag),
        })
    }
}

/// One tone-tagged text run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TipSpan {
    pub text: String,
    pub tone: TipTone,
}

impl TipSpan {
    fn new(text: impl Into<String>, tone: TipTone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }
}

/// One row of a tooltip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TipRow {
    /// Runs laid left to right.
    Spans(Vec<TipSpan>),
    /// `left`, then `right` flush to the box's right edge.
    Heading { left: Vec<TipSpan>, right: TipSpan },
    /// A rule across the box.
    Rule,
}

/// Which side of the pointer a tooltip opens on first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipAnchor {
    /// Below, flipping above when there is no room: an agent's card.
    Below,
    /// Above, flipping below when there is no room: a one-line label.
    Above,
}

/// A tooltip: its rows, top to bottom, and where it opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tooltip {
    pub rows: Vec<TipRow>,
    pub anchor: TipAnchor,
}

impl Tooltip {
    /// A one-line label; `text` carries its own padding.
    pub fn label(text: &str) -> Self {
        Self {
            rows: vec![TipRow::Spans(vec![TipSpan::new(text, TipTone::Title)])],
            anchor: TipAnchor::Above,
        }
    }
}

/// The tooltip for what `hit` names on `world`'s frame; `None` for the star,
/// whose click is its whole story, and for an agent or gateway gone from the
/// scene.
pub fn for_hit(hit: SceneHit<'_>, world: &FloorInputs<'_>) -> Option<Tooltip> {
    match hit {
        SceneHit::Figure(HoverTarget::Agent(id)) => agent(world.scene, *id, world.now),
        SceneHit::Figure(&HoverTarget::Pet(PetHover { kind, anim, .. })) => {
            let on_cooldown = world.pets.petting.is_some_and(|p| p.is_active(world.now));
            // The hover was drawn from `pets.pet`, so the kinds agree and the
            // `default_name` arm is not a live path.
            let name = world
                .pets
                .pet
                .map_or_else(|| kind.default_name(), |p| p.name.as_str());
            Some(pet(kind, anim, on_cooldown, name))
        }
        SceneHit::Figure(HoverTarget::Mascot(key)) => {
            GatewayCard::of(world.scene, key).map(|card| Tooltip::label(&mascot_text(&card)))
        }
        SceneHit::Coffee => Some(coffee()),
        SceneHit::Furniture(label) => Some(Tooltip::label(&format!(" {label} "))),
        SceneHit::Star => None,
    }
}

/// An agent's card: its badge, label and session id, then its state and
/// tool, what it works on, under whom, where, on which model, and its
/// session's stats. An exiting agent shows none of the live affordances: the
/// walking-out slot keeps its Active/Waiting payload, so the exiting-first
/// kind gates them, not the raw state.
pub fn agent(scene: &SceneState, id: AgentId, now: SystemTime) -> Option<Tooltip> {
    let agent = scene.agents.get(&id)?;
    let kind = if agent.exiting_at.is_some() {
        RungKind::Exiting
    } else {
        match agent.state {
            ActivityState::Active { .. } => RungKind::Active,
            ActivityState::Waiting { .. } => RungKind::Waiting,
            ActivityState::Idle => RungKind::Idle,
        }
    };
    let mut state = vec![TipSpan::new(
        format!("{} {}", kind.glyph(), kind.word()),
        TipTone::Rung(kind),
    )];
    let mut detail: Option<String> = None;
    if kind != RungKind::Exiting {
        if let ActivityState::Active {
            detail: Some(d),
            kind: tool_kind,
            ..
        } = &agent.state
            && !d.is_empty()
        {
            let (tool, rest) = d
                .split_once(char::is_whitespace)
                .map(|(t, r)| (t.trim_end_matches(':'), r.trim()))
                .unwrap_or((d.trim_end_matches(':'), ""));
            if !tool.is_empty() {
                state.push(TipSpan::new(" \u{b7} ", TipTone::Plain));
                // The hue comes from the TYPED kind, never a re-parse of the name.
                state.push(TipSpan::new(tool, TipTone::Tool(*tool_kind)));
            }
            if !rest.is_empty() {
                detail = Some(rest.chars().take(DETAIL_CHARS).collect());
            }
        } else if let ActivityState::Waiting { reason } = &agent.state {
            detail = Some(format!(
                "?{}",
                reason.chars().take(DETAIL_CHARS).collect::<String>()
            ));
        }
    }
    let dim = |text: String| TipRow::Spans(vec![TipSpan::new(text, TipTone::Dim)]);
    let mut rows = vec![
        TipRow::Heading {
            left: vec![
                TipSpan::new(
                    format!(
                        "[{:<2}]",
                        descriptor_for(agent.source.as_ref()).map_or("??", |d| d.label_prefix)
                    ),
                    TipTone::Source(
                        descriptor_for(agent.source.as_ref()).map_or("??", |d| d.label_prefix),
                    ),
                ),
                TipSpan::new(format!(" {}", agent.label), TipTone::Title),
            ],
            right: TipSpan::new(
                format!("\u{b7}{}", crate::badge::disambig_suffix(&agent.session_id)),
                TipTone::Dim,
            ),
        },
        TipRow::Rule,
        TipRow::Spans(state),
    ];
    if let Some(d) = detail {
        rows.push(TipRow::Spans(vec![TipSpan::new(
            format!("  {d}"),
            TipTone::Text,
        )]));
    }
    if let Some(parent) = agent.parent_id.and_then(|p| scene.agents.get(&p)) {
        rows.push(dim(format!("\u{21b3} under {}", parent.label)));
    }
    rows.push(dim(format!("\u{25a4} {}", short_cwd(&agent.cwd))));
    // The effort is suffixed only while FRESH — the same burn-TTL the flame
    // reads, so the text can't outlive the fire.
    if let Some(model) = agent.model.as_deref() {
        let mut row = format!("\u{2605} {model}");
        if let Some(effort) = crate::burn::fresh_effort(agent, now) {
            row.push_str(&format!(" \u{b7} {effort}"));
        }
        rows.push(dim(row));
    }
    let session_secs = now
        .duration_since(agent.created_at)
        .unwrap_or_default()
        .as_secs();
    let mut stats = format!(
        "\u{25f7} {} \u{b7} {} calls",
        crate::neon_sign::compact_hms(session_secs),
        agent.tool_call_count
    );
    // Skipped at zero so sources with no usage wire keep their card unchanged.
    if agent.tokens_used > 0 {
        stats.push_str(&format!(
            " \u{b7} \u{3a3} {} tok",
            crate::token_meter::compact_tokens(agent.tokens_used)
        ));
    }
    // Fresh agents show no active-% meter — the % is noise before ~5s of accounting.
    if kind == RungKind::Active && session_secs >= 5 {
        let pct = (agent.active_ms / 1000)
            .checked_mul(100)
            .and_then(|n| n.checked_div(session_secs))
            .map(|p| p.min(100))
            .unwrap_or(0);
        let filled = (pct as usize * 5).div_ceil(100).min(5);
        let meter: String = "\u{25ae}".repeat(filled) + &"\u{25af}".repeat(5 - filled);
        stats.push_str(&format!(" \u{b7} {meter} {pct}%"));
    }
    rows.push(dim(stats));
    Some(Tooltip {
        rows,
        anchor: TipAnchor::Below,
    })
}

/// A short form of a cwd path: the TAIL (most informative — project dir) with a
/// leading `…`. Char-sliced, never a byte slice, so a multibyte path can't panic.
fn short_cwd(cwd: &std::path::Path) -> String {
    const MAX: usize = 30;
    let s = cwd.to_string_lossy();
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= MAX {
        s.into_owned()
    } else {
        format!(
            "\u{2026}{}",
            chars[chars.len() - (MAX - 1)..].iter().collect::<String>()
        )
    }
}

/// The coffee machine's label.
pub fn coffee() -> Tooltip {
    Tooltip::label(" \u{2615} Buy Ivan a coffee ")
}

/// A pet's label: its reaction while a petting plays, else what it is doing,
/// else its name.
pub fn pet(kind: PetKind, anim: &str, on_cooldown: bool, name: &str) -> Tooltip {
    let idle = format!(" {name} ");
    let text: &str = if on_cooldown {
        match kind {
            PetKind::Cat => " purr... ",
            PetKind::Dog => " woof! ",
        }
    } else if anim == kind.sleep_anim() {
        " Shhh... sleeping "
    } else if anim == kind.sit_anim() {
        " Pet me! "
    } else {
        &idle
    };
    Tooltip::label(text)
}

/// The gateway mascot's label. The verb keys on `busy` — see
/// [`GatewayCard::busy`] for why the run state, not the session count — and
/// `degraded` outranks busy/idle.
pub fn mascot_text(card: &GatewayCard) -> String {
    let &GatewayCard {
        name,
        ref instance,
        busy,
        degraded,
        active_sessions,
    } = card;
    // `OpenClaw:19789` — `instance` is set only when there IS a sibling to tell
    // apart, so the single-gateway label stays byte-unchanged.
    let name = match instance {
        Some(i) => format!("{name}:{i}"),
        None => name.to_string(),
    };
    let verb = if degraded {
        "model error"
    } else if busy {
        "working"
    } else {
        "idle"
    };
    if active_sessions > 1 {
        format!(" {name} gateway \u{b7} {verb} \u{b7} {active_sessions} sessions ")
    } else {
        format!(" {name} gateway \u{b7} {verb} ")
    }
}
