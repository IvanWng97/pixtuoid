//! ONE offline driver for the owner's contract: *transcripts in → "did we
//! parse it AND would the UI show it" out.*
//!
//! Four call sites used to implement the same pipeline by hand — the committed
//! fixtures (conformance), one fixture through the real renderer
//! (`wire_to_pixels`), stdin (`decoder_fuzz`), and a whole corpus tree
//! (`corpus_check`). They differ ONLY in where bytes come from and in
//! assert-vs-report; the pipeline between those two ends is the same:
//!
//! ```text
//!   raw line → JSON → the registry's decoder → a real Reducer → SceneState
//! ```
//!
//! [`Drive`] is that pipeline and [`Driven`] is everything the four shells ever
//! asserted or reported about it. Whether the resulting scene would PAINT is
//! the render layer's question, asked one crate up (`FloorSession::observe`);
//! this half stops at the state the painter reads.
//!
//! Three things here are load-bearing rather than incidental:
//!
//! 1. **Registration comes from the WATCHER, not the decoder.** A JSONL event
//!    for an unknown id is a documented no-op, so a transcript driven with no
//!    seed registers nothing however well it decodes — the corpus census's
//!    first run reported EVERY transcript unregistered for exactly this reason,
//!    and that was the harness's bug, not the decoder's. [`Drive::seeded`] stands in for
//!    `emit_first_sight`, keyed by [`registry::id_deriver_for`] — the SAME row
//!    the watcher reads, so the seed can't drift from production. The file SET
//!    comes from that row too ([`registry::path_filter_for`]), so a driver that
//!    walks a tree reads what the watcher would and no more.
//! 2. **Transport is load-bearing** (the reducer's hook-wins dedup keys on it),
//!    so it is not a free parameter: [`Drive::transcript`] is `Jsonl` through
//!    the row's `LineDecoder` and [`Drive::hooks`] is `Hook` through the shared
//!    `decode_hook_payload`.
//! 3. **A decoder panic is a contract violation, everywhere.** The watcher and
//!    hook listener log-and-continue on malformed input; a panic takes the
//!    whole watcher down. Every line therefore runs under `catch_unwind` — the
//!    never-panic invariant used to be checked only by the on-demand fuzz shell,
//!    and is now inherent in the pipeline all four ride.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::source::decoder::{decode_hook_payload, display_safe, LineDecoder};
use crate::source::registry;
use crate::state::{ActivityState, SceneState, ToolKind};
use crate::{AgentEvent, AgentId, Reducer, Transport};

/// A lifecycle CLASS a driven wire pushed the slot through — asserted as
/// reached-at-some-point, never as the terminal state (`mark_exiting` sets only
/// `exiting_at` and never resets `slot.state`, so a fixture whose last activity
/// its own wire never resolves legitimately ends non-Idle).
///
/// A class, not a `ToolKind`: `from_display` is case-sensitive, so a lowercase
/// wire tool name (`"bash"`) renders `Active(Other)` where `"Bash"` renders
/// `Active(Bash)` — a cosmetic per-source difference that must not be frozen
/// into a lifecycle assertion. `Delegating` is `Active` with `kind ==
/// ToolKind::Task`; `ActivityState` has no separate variant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reach {
    /// A tool call is in flight.
    Active,
    /// Blocked on a permission/input prompt.
    Waiting,
    /// A subagent dispatch is in flight.
    Delegating,
}

/// One line the pipeline could not carry, identified by position and SHAPE.
///
/// Never by content: a real transcript line carries the user's prose, code and
/// paths, and these render on a terminal (the fuzz shell prints them). `shape`
/// is the top-level `type` plus the key NAMES — enough to find the line in the
/// file it came from, and nothing a reader of the report shouldn't see.
#[derive(Clone, Debug)]
pub struct LineFailure {
    /// 1-based index among the non-blank lines driven.
    pub line: usize,
    /// `type="…" keys=[…]` — structure only.
    pub shape: String,
    /// The decoder's own error. `None` for a panic — which `Vec` the failure
    /// landed in already says which it was, so the message stays honest
    /// instead of doubling as a sentinel.
    pub message: Option<String>,
}

impl std::fmt::Display for LineFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.shape)?;
        if let Some(m) = &self.message {
            write!(f, ": {m}")?;
        }
        Ok(())
    }
}

/// Everything the pipeline observed. The four shells project different fields:
/// conformance snapshots `events` and asserts they coalesce, `wire_to_pixels`
/// asserts on `reached` then renders `scene`, the fuzz shell counts `panics`,
/// the corpus shell reports the lot.
#[derive(Debug)]
pub struct Driven {
    /// The folded scene — `agents.len()` is "did it register", and the scene
    /// itself is what a painter would observe.
    pub scene: SceneState,
    /// Every decoded event in wire order, seed first when there is one.
    pub events: Vec<AgentEvent>,
    /// How many leading `events` are the first-sight SEED, not the wire's own
    /// output (0 or 1). Separable because a driver that COUNTS the seed reports
    /// a verdict on bytes that produced nothing: the seed registers a slot
    /// unconditionally, so "registered" and "would paint" both come out 1/1 for
    /// a file of pure garbage. Report [`Self::wire_events`] instead.
    pub seed_events: usize,
    /// Non-blank lines driven (blank lines are skipped, not counted).
    pub lines: usize,
    /// Lines that were not valid JSON — a torn write, outside the decoder
    /// contract (the watcher skips them too).
    pub unparseable: usize,
    /// Lines whose decoder returned `Err`. ALWAYS a defect on bytes the source
    /// itself wrote: the contract is log-and-continue, never a hard error.
    pub decode_errors: Vec<LineFailure>,
    /// Lines whose decoder PANICKED — the never-panic contract violated.
    pub panics: Vec<LineFailure>,
    /// Lifecycle classes any slot passed through, in first-reached order.
    pub reached: Vec<Reach>,
}

impl Driven {
    /// Slots the fold registered — "did the wire reach the scene at all".
    pub fn registered(&self) -> usize {
        self.scene.agents.len()
    }

    /// Events the WIRE produced, excluding the first-sight seed. The honest
    /// numerator for any census: zero here means the bytes decoded to nothing,
    /// however many slots the seed put on the floor.
    pub fn wire_events(&self) -> usize {
        self.events.len() - self.seed_events
    }

    /// Assert bytes WE control drove cleanly: every line parsed, no decoder
    /// `Err` (the contract is log-and-continue, so an Err on a source's own
    /// bytes is a defect), no panic. `what` names the input in the failure.
    ///
    /// The corpus shell deliberately does NOT call this — its bytes are
    /// unbounded, so it reports the same three counts instead.
    pub fn assert_clean(&self, what: &str) {
        assert_eq!(
            self.unparseable, 0,
            "{what}: {} line(s) are not valid JSON",
            self.unparseable
        );
        assert!(
            self.decode_errors.is_empty(),
            "{what}: decode error(s): {:?}",
            self.decode_errors
        );
        assert!(
            self.panics.is_empty(),
            "{what}: the decoder PANICKED (never-panic contract): {:?}",
            self.panics
        );
    }
}

/// The fixed wall clock the fold runs at. Deterministic by construction: with
/// one instant for every event, no sweep's elapsed-time threshold can fire
/// mid-drive and reap a slot the caller is about to assert on.
const DEFAULT_NOW_EPOCH_SECS: u64 = 1_800_000_000;

/// How far past `now` the settle tick runs. Long enough for the reducer's
/// `ACTIVE_GRACE_WINDOW` pending-idle to realize (so a resolved tool really
/// reads Idle), short enough to stay inside `EXIT_GRACE_WINDOW` (so a wire
/// carrying its own `SessionEnd` still leaves the slot to observe). It also
/// clears `B1_CASCADE_GRACE`, which only stamps `exiting_at` and never resets
/// `slot.state` — so a fired cascade cannot erase a class already reached.
const SETTLE: Duration = Duration::from_secs(3);

/// PER-FLOOR desk capacity for the driven scene (`SceneState::uniform`
/// replicates it across `MAX_FLOORS`). Registration past the TOTAL is REFUSED,
/// and SILENTLY — so a wire carrying more agents than that reads as "decoded but
/// never reached the scene", which is exactly the false verdict every driver
/// here exists to avoid. Sized well above any single wire's agent count
/// (Copilot interleaves all of a session's subagents in ONE transcript), not at
/// the production headless fallback, whose 16 answers a different question.
///
/// SECOND consumer, in another crate: `wire_to_pixels` shapes its EMPTY pixel
/// baseline with this same value so both sides of its sprite diff share a desk
/// layout. Changing it for a corpus reason moves that test's measured delta —
/// check its floor before raising this.
pub const DRIVEN_DESKS: usize = 64;

/// The seed session's working directory. Non-empty on purpose: an empty cwd
/// registers the reducer's unknown-cwd ghost (a bare ordinal label, and a
/// ~3-min reap in production), which is not what any driver wants to observe.
const SEED_CWD: &str = "/pixtuoid/harness";

/// The decode-and-fold pipeline for one source's bytes.
///
/// Construct with [`Drive::transcript`] (JSONL lines through the source's
/// registry `LineDecoder`) or [`Drive::hooks`] (hook envelopes through the
/// shared dispatcher), then feed lines to [`Drive::lines`].
pub struct Drive {
    transport: Transport,
    decode: Decode,
    seeded: bool,
    now: SystemTime,
}

/// The transport's decoder plus whatever it needs. Private so the
/// decoder↔transport pairing stays a constructor's job, never a caller's.
enum Decode {
    /// A transcript's line decoder, with the source it decodes for and the
    /// logical path it keys on.
    Transcript {
        decode: LineDecoder,
        source: String,
        logical: String,
    },
    /// The shared hook dispatcher. It carries NO source: `decode_hook_payload`
    /// routes on the envelope's own `_pixtuoid_source` stamp, so a source
    /// passed in here would be inert at best and a lie at worst — the payload
    /// would win.
    Hooks,
}

impl Drive {
    /// Drive the transcript AT `path` — the production form: the logical key
    /// is `normalize_path_key` of it, which is exactly the string `walk_jsonl`
    /// hands the decoder AND runs the id deriver over.
    ///
    /// Use this whenever the transcript is a real file. Normalizing at this ONE
    /// point is what keeps the seed and the decoded lines on one `AgentId` for a
    /// source whose deriver normalizes (Antigravity's path key): passing a raw
    /// Windows path to [`Drive::transcript`] would key the seed lowercased and
    /// forward-slashed while the decoder kept the raw string — every decoded
    /// event landing on a phantom id, on Windows only.
    pub fn transcript_at(source: &str, path: &Path) -> Option<Self> {
        Self::transcript(
            source,
            &crate::id::normalize_path_key(&path.to_string_lossy()),
        )
    }

    /// Drive a transcript's lines through `source`'s own `LineDecoder` on
    /// `Transport::Jsonl`, keyed on the LOGICAL string `logical` — for a caller
    /// whose key is not a filesystem path (a fixture-relative key chosen so
    /// snapshots stay machine-independent). For a real file use
    /// [`Drive::transcript_at`], which applies production's normalization.
    ///
    /// `None` when the source is hook-only, a daemon, or unregistered: those
    /// have no transcript, and a caller that silently fell back to some other
    /// decoder would report a green never-panic run having exercised the wrong
    /// one.
    pub fn transcript(source: &str, logical: &str) -> Option<Self> {
        let decode = registry::descriptor_for(source)?.line_decoder()?;
        Some(Self {
            transport: Transport::Jsonl,
            decode: Decode::Transcript {
                decode,
                source: source.to_string(),
                logical: logical.to_string(),
            },
            seeded: false,
            now: SystemTime::UNIX_EPOCH + Duration::from_secs(DEFAULT_NOW_EPOCH_SECS),
        })
    }

    /// Drive hook envelopes through the shared `decode_hook_payload` on
    /// `Transport::Hook`. Takes NO source: the dispatcher routes by the
    /// envelope's own `_pixtuoid_source` stamp, and a daemon's payloads
    /// short-circuit to zero `AgentEvent`s (presence rides a sibling channel).
    ///
    /// There is no seed here by construction — a hook for an unknown id
    /// REGISTERS it (hooks are proof of life), so hook bytes never need one.
    pub fn hooks() -> Self {
        Self {
            transport: Transport::Hook,
            decode: Decode::Hooks,
            seeded: false,
            now: SystemTime::UNIX_EPOCH + Duration::from_secs(DEFAULT_NOW_EPOCH_SECS),
        }
    }

    /// Prepend the `SessionStart` the watcher's `emit_first_sight` would emit
    /// for this transcript, keyed by the source's registry row.
    ///
    /// Not a free choice: it mirrors what the source's OWN wire does. A
    /// transcript carrying no `SessionStart` of its own (CC, Codex,
    /// Antigravity, grok) is registered in production by the watcher, so an
    /// offline driver must seed it; one whose head line IS a `SessionStart`
    /// (Copilot, omp) registers itself and a seed would only duplicate it. A
    /// no-op on a hook drive, which never needs one (hooks are proof of life).
    #[must_use]
    pub fn seeded(mut self) -> Self {
        self.seeded = true;
        self
    }

    /// Fold at `now` instead of the default fixed instant. Only a caller that
    /// RENDERS the resulting scene needs this — the render must read the same
    /// clock the fold wrote, or every sprite is mid-entry-walk.
    #[must_use]
    pub fn at(mut self, now: SystemTime) -> Self {
        self.now = now;
        self
    }

    /// The `AgentId` a first-sight seed keys on: the source's registry
    /// derivation over the transcript path — byte-identical to what the
    /// watcher would have registered.
    fn seed_id(source: &str, logical: &str) -> AgentId {
        AgentId::from_parts(
            source,
            &registry::id_deriver_for(source)(Path::new(logical)),
        )
    }

    /// Drive `lines` end to end. Blank lines are skipped; every other line is
    /// parsed, decoded under `catch_unwind`, and the whole decoded stream is
    /// folded through a real `Reducer`.
    pub fn lines<I, S>(&self, lines: I) -> Driven
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut d = Driven {
            scene: SceneState::uniform(DRIVEN_DESKS),
            events: Vec::new(),
            seed_events: 0,
            lines: 0,
            unparseable: 0,
            decode_errors: Vec::new(),
            panics: Vec::new(),
            reached: Vec::new(),
        };

        if let (
            true,
            Decode::Transcript {
                source, logical, ..
            },
        ) = (self.seeded, &self.decode)
        {
            d.seed_events = 1;
            d.events.push(AgentEvent::SessionStart {
                agent_id: Self::seed_id(source, logical),
                source: source.clone(),
                session_id: format!("{source}-harness-seed"),
                cwd: PathBuf::from(SEED_CWD),
                parent_id: None,
            });
        }

        for line in lines {
            let line = line.as_ref();
            if line.trim().is_empty() {
                continue;
            }
            d.lines += 1;
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                d.unparseable += 1;
                continue;
            };
            // `v` moves into the decoder: cloning every line to keep a copy for
            // the failure shape would clone whole transcript bodies on the
            // corpus path. A panic is rare enough to re-parse for.
            match catch_unwind(AssertUnwindSafe(|| self.decode_one(v))) {
                Ok(Ok(evs)) => d.events.extend(evs),
                Ok(Err(e)) => d.decode_errors.push(LineFailure {
                    line: d.lines,
                    shape: shape_of(line),
                    message: Some(e.to_string()),
                }),
                Err(_) => d.panics.push(LineFailure {
                    line: d.lines,
                    shape: shape_of(line),
                    message: None,
                }),
            }
        }

        self.fold(&mut d);
        d
    }

    fn decode_one(&self, v: Value) -> anyhow::Result<Vec<AgentEvent>> {
        match &self.decode {
            Decode::Transcript {
                decode,
                source,
                logical,
            } => decode(logical, source, v),
            Decode::Hooks => decode_hook_payload(v),
        }
    }

    /// Fold the decoded stream through a real `Reducer`, recording every
    /// lifecycle class any slot passes through. Each event is observed BEFORE
    /// and AFTER a settle tick, so a transient class a later event resolves
    /// still counts as reached.
    fn fold(&self, d: &mut Driven) {
        let mut reducer = Reducer::new();
        for ev in &d.events {
            reducer.apply(&mut d.scene, ev.clone(), self.now, self.transport);
            note_reached(&d.scene, &mut d.reached);
            reducer.tick(&mut d.scene, self.now + SETTLE);
            note_reached(&d.scene, &mut d.reached);
        }
    }
}

/// Union the lifecycle classes every live slot is in into `reached`.
///
/// Every slot, not the first: a `HashMap`'s iteration order is arbitrary, so
/// reading one slot makes a multi-agent wire (a parent and its subagent) report
/// a different class run to run. The trade is explicit — this is a DETERMINISM
/// fix that is also strictly WEAKER: on a fixture registering a parent AND a
/// child, "one slot reached both classes" and "two slots reached one each" are
/// now indistinguishable. A stronger form is `HashMap<AgentId, Vec<Reach>>` with
/// "some ONE slot reached all of these"; not worth the shape until a caller
/// needs to tell those apart.
fn note_reached(scene: &SceneState, reached: &mut Vec<Reach>) {
    for slot in scene.agents.values() {
        let r = match &slot.state {
            ActivityState::Active {
                kind: ToolKind::Task,
                ..
            } => Reach::Delegating,
            ActivityState::Active { .. } => Reach::Active,
            ActivityState::Waiting { .. } => Reach::Waiting,
            ActivityState::Idle => continue,
        };
        if !reached.contains(&r) {
            reached.push(r);
        }
    }
}

/// A line's STRUCTURE — its top-level `type` and key names, never its values.
/// The one content-safe formatter for failure reporting (the corpus and fuzz
/// shells print these straight to a terminal).
fn shape_of(line: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return "<unparseable>".to_string();
    };
    // Key NAMES are wire-controlled too, so they route through the crate's ONE
    // terminal-egress policy (Cc + the Cf bidi set, then capped) like every
    // drift breadcrumb — a second, laxer sanitizer here would be the documented
    // chokepoint-BYPASS class.
    let ty = display_safe(v.get("type").and_then(Value::as_str).unwrap_or(""));
    let keys = v.as_object().map_or_else(
        || "<non-object>".to_string(),
        |o| display_safe(&o.keys().cloned().collect::<Vec<_>>().join(",")),
    );
    format!("type={ty:?} keys=[{keys}]")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CC transcript line carrying only tool activity — the shape that made
    /// `seeded()` load-bearing (nothing in it registers a session).
    fn cc_tool_line() -> String {
        serde_json::json!({
            "type": "assistant",
            "message": {"content": [
                {"type": "tool_use", "id": "tu-1", "name": "Bash", "input": {"command": "ls"}}
            ]}
        })
        .to_string()
    }

    const CC_TRANSCRIPT: &str =
        "/h/.claude/projects/-h-p/01000000-0000-7000-8000-0000000000cc.jsonl";

    #[test]
    fn an_unseeded_activity_only_transcript_decodes_but_registers_nothing() {
        // The documented no-op `seeded()` exists for: the events are
        // real, and every one of them lands against an unknown id.
        let d = Drive::transcript("claude-code", CC_TRANSCRIPT)
            .unwrap()
            .lines([cc_tool_line()]);
        assert!(!d.events.is_empty(), "the line must decode");
        assert_eq!(
            d.registered(),
            0,
            "a JSONL event for an unknown id is a no-op"
        );
        assert!(d.reached.is_empty());
    }

    #[test]
    fn the_first_sight_seed_registers_and_the_wire_drives_the_slot_active() {
        let d = Drive::transcript("claude-code", CC_TRANSCRIPT)
            .unwrap()
            .seeded()
            .lines([cc_tool_line()]);
        assert_eq!(d.registered(), 1);
        assert_eq!(d.reached, vec![Reach::Active]);
    }

    /// The seed must key EXACTLY as the watcher's first-sight would, or it
    /// registers one agent and the decoded activity lands on another — two
    /// slots for one session, which is the bug this seeds against.
    #[test]
    fn the_seed_coalesces_with_the_decoders_own_agent_id() {
        let d = Drive::transcript("claude-code", CC_TRANSCRIPT)
            .unwrap()
            .seeded()
            .lines([cc_tool_line()]);
        let ids: std::collections::BTreeSet<_> =
            d.events.iter().map(AgentEvent::agent_id).collect();
        assert_eq!(ids.len(), 1, "seed + decoded activity must be ONE agent");
        assert_eq!(
            ids.into_iter().next().unwrap(),
            AgentId::from_parts("claude-code", "01000000-0000-7000-8000-0000000000cc"),
            "the seed must key on the row's derivation (the CC filename stem)"
        );
    }

    /// `transcript_at` keys on the SAME normalized string production hands the
    /// decoder — the one place a raw OS path may be folded.
    #[test]
    fn transcript_at_keys_on_the_normalized_path_like_the_walker_does() {
        let path = Path::new(CC_TRANSCRIPT);
        let by_path = Drive::transcript_at("claude-code", path)
            .unwrap()
            .seeded()
            .lines([cc_tool_line()]);
        let by_key = Drive::transcript(
            "claude-code",
            &crate::id::normalize_path_key(&path.to_string_lossy()),
        )
        .unwrap()
        .seeded()
        .lines([cc_tool_line()]);
        assert_eq!(
            by_path.events[0].agent_id(),
            by_key.events[0].agent_id(),
            "transcript_at must fold the path exactly as walk_jsonl does"
        );
    }

    #[test]
    fn blank_lines_are_skipped_and_non_json_counts_as_unparseable() {
        let d = Drive::transcript("claude-code", CC_TRANSCRIPT)
            .unwrap()
            .lines(["", "   ", "not json at all", "{}"]);
        assert_eq!(d.lines, 2, "only the non-blank lines are driven");
        assert_eq!(d.unparseable, 1);
        assert!(d.decode_errors.is_empty());
    }

    #[test]
    fn a_hook_envelope_registers_itself_with_no_seed() {
        // Hooks are proof of life: an unknown id is REGISTERED, not ignored.
        let payload = serde_json::json!({
            "_pixtuoid_source": "claude-code",
            "hook_event_name": "SessionStart",
            "session_id": "ses-h",
            "cwd": "/repo"
        })
        .to_string();
        let d = Drive::hooks().lines([payload]);
        assert_eq!(d.registered(), 1);
    }

    /// A hook-only source has no transcript to drive — the constructor refuses
    /// rather than falling back to another source's decoder (the misroute that
    /// once reported a false-green fuzz run).
    #[test]
    fn transcript_is_none_for_hook_only_daemon_and_unknown_sources() {
        assert!(Drive::transcript("cursor", "/x.jsonl").is_none());
        assert!(Drive::transcript("openclaw", "/x.jsonl").is_none());
        assert!(Drive::transcript("not-a-source", "/x.jsonl").is_none());
    }

    /// A `Drive` over a STAND-IN decoder. The two failure lanes below are about
    /// the harness's capture, not about which real decoder happens to reject
    /// which bytes today (a corpus run measures 0 decode errors, so no real
    /// decoder can serve as the fixture for either lane).
    fn drive_with(decode: LineDecoder) -> Drive {
        Drive {
            transport: Transport::Jsonl,
            decode: Decode::Transcript {
                decode,
                source: "claude-code".to_string(),
                logical: CC_TRANSCRIPT.to_string(),
            },
            seeded: false,
            now: SystemTime::UNIX_EPOCH + Duration::from_secs(DEFAULT_NOW_EPOCH_SECS),
        }
    }

    /// A line carrying prose under a key — the shape a real transcript has, and
    /// what the failure report must NOT echo back to a terminal.
    const PROSE_LINE: &str = r#"{"type":"assistant","secret":"do not print me"}"#;

    #[test]
    fn a_decoder_error_is_recorded_with_the_lines_shape_not_its_content() {
        fn refuse(_p: &str, _s: &str, _v: Value) -> anyhow::Result<Vec<AgentEvent>> {
            Err(anyhow::anyhow!("unsupported event"))
        }
        let d = drive_with(refuse).lines([PROSE_LINE]);

        assert_eq!(d.decode_errors.len(), 1);
        assert_eq!(d.decode_errors[0].line, 1);
        assert_eq!(
            d.decode_errors[0].message.as_deref(),
            Some("unsupported event")
        );
        let shape = &d.decode_errors[0].shape;
        assert!(
            !shape.contains("do not print me"),
            "the shape must carry key NAMES only, got {shape}"
        );
        assert!(shape.contains("type=\"assistant\""), "got {shape}");
        assert!(shape.contains("secret"), "got {shape}");
        assert!(d.panics.is_empty(), "a decode error is not a panic");
    }

    /// THE anti-tautology pin, at the lib level rather than only in the census
    /// that consumes it: a seeded drive registers a slot unconditionally, so
    /// `registered()` alone reports success for bytes that decoded to NOTHING.
    /// `wire_events()` is what a report must count instead.
    #[test]
    fn wire_events_excludes_the_seed_so_garbage_scores_zero() {
        let garbage = ["not json at all", r#"{"totally":"unrelated"}"#];
        let d = Drive::transcript("claude-code", CC_TRANSCRIPT)
            .unwrap()
            .seeded()
            .lines(garbage);
        assert_eq!(
            d.registered(),
            1,
            "the seed registers a slot even for garbage — that is WHY the census \
             cannot count it"
        );
        assert_eq!(d.wire_events(), 0, "no wire event came out of garbage");
        assert_eq!(d.events.len(), 1, "the one event is the seed itself");

        // A real line: the seed is still excluded, the activity is not.
        let d = Drive::transcript("claude-code", CC_TRANSCRIPT)
            .unwrap()
            .seeded()
            .lines([cc_tool_line()]);
        assert_eq!(d.seed_events, 1);
        assert_eq!(d.wire_events(), d.events.len() - 1);

        // Unseeded, every event is the wire's.
        let d = Drive::transcript("claude-code", CC_TRANSCRIPT)
            .unwrap()
            .lines([cc_tool_line()]);
        assert_eq!(d.seed_events, 0);
        assert_eq!(d.wire_events(), d.events.len());
    }

    /// Both on-demand tools PRINT failures to a terminal, so `Display` is the
    /// last content-safety boundary: position + shape, never the line's values.
    #[test]
    fn line_failure_display_carries_position_and_shape_not_content() {
        fn refuse(_p: &str, _s: &str, _v: Value) -> anyhow::Result<Vec<AgentEvent>> {
            Err(anyhow::anyhow!("unsupported event"))
        }
        let d = drive_with(refuse).lines([PROSE_LINE]);
        let rendered = d.decode_errors[0].to_string();
        assert!(rendered.starts_with("line 1: "), "got {rendered}");
        assert!(rendered.contains("type=\"assistant\""), "got {rendered}");
        assert!(rendered.ends_with(": unsupported event"), "got {rendered}");
        assert!(
            !rendered.contains("do not print me"),
            "Display must not echo the line's values, got {rendered}"
        );

        // A panic carries no message, so Display stops at the shape.
        fn boom(_p: &str, _s: &str, _v: Value) -> anyhow::Result<Vec<AgentEvent>> {
            panic!("decoder blew up")
        }
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let d = drive_with(boom).lines([PROSE_LINE]);
        std::panic::set_hook(prev);
        let rendered = d.panics[0].to_string();
        assert!(!rendered.contains("blew up"), "got {rendered}");
        assert!(
            rendered.ends_with(']'),
            "shape is the whole line, got {rendered}"
        );
    }

    /// NEGATIVE CONTROL for the never-panic capture: a decoder that panics must
    /// be RECORDED, not propagated. Without this the `panics` field could stay
    /// permanently empty and every shell would report a green contract.
    #[test]
    fn a_panicking_decoder_is_caught_and_recorded_by_shape() {
        fn boom(_p: &str, _s: &str, _v: Value) -> anyhow::Result<Vec<AgentEvent>> {
            panic!("decoder blew up")
        }
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let d = drive_with(boom).lines([PROSE_LINE]);
        std::panic::set_hook(prev);

        assert_eq!(d.panics.len(), 1, "the panic must be captured, not unwound");
        assert_eq!(d.panics[0].line, 1);
        assert!(
            d.panics[0].shape.contains("type=\"assistant\""),
            "got {}",
            d.panics[0].shape
        );
        assert!(
            !d.panics[0].shape.contains("do not print me"),
            "got {}",
            d.panics[0].shape
        );
        assert!(d.decode_errors.is_empty(), "a panic is not a decode error");
    }

    /// A class reached only transiently — resolved by a later line — still
    /// counts, because the fold observes before AND after each settle tick.
    #[test]
    fn a_transient_class_resolved_by_a_later_line_still_counts_as_reached() {
        let start = serde_json::json!({
            "type": "assistant",
            "message": {"content": [
                {"type": "tool_use", "id": "tu-9", "name": "Bash", "input": {"command": "ls"}}
            ]}
        })
        .to_string();
        let end = serde_json::json!({
            "type": "user",
            "message": {"content": [
                {"type": "tool_result", "tool_use_id": "tu-9", "content": "ok"}
            ]}
        })
        .to_string();
        let d = Drive::transcript("claude-code", CC_TRANSCRIPT)
            .unwrap()
            .seeded()
            .lines([start, end]);
        assert!(
            d.reached.contains(&Reach::Active),
            "the resolved tool call must still count as reached, got {:?}",
            d.reached
        );
    }
}
