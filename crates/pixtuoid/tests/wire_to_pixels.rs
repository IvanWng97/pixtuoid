//! TRUE end-to-end: real wire bytes → the production decoder → a real
//! `Reducer` → a `SceneState` → the REAL `TuiRenderer` (ratatui `TestBackend`)
//! → an actual pixel buffer, asserting a sprite was PAINTED.
//!
//! The `tui_renderer.rs` integration test starts from a hand-built `AgentSlot`
//! — it proves the renderer paints, but skips the decode+reduce half. These
//! tests close that gap for EVERY supported source: they start from the SAME
//! real fixture bytes the conformance harness decodes (a captured transcript
//! line, a captured hook envelope, a captured daemon-presence envelope), run
//! them through the SAME production decoder (`registry::line_decoder()` for a
//! transcript source, `decode_hook_payload` for a hook source, the openclaw
//! presence decoder for the daemon), fold the resulting `AgentEvent`s through a
//! real `Reducer` (or `apply_presence` for the daemon), and render the scene to
//! pixels — proving real bytes become a visible character (or lobster), not
//! just a slot.
//!
//! Three transport classes are covered:
//!   - JSONL transcript  → `Transport::Jsonl` (claude-code, codex, antigravity, copilot)
//!   - agent hook         → `Transport::Hook`  (reasonix, codewhale, opencode, cursor)
//!   - daemon presence    → the sibling channel (`apply_presence`) (openclaw)
//!
//! The agent assertion is a pixel-diff FLOOR: an occupied frame must repaint
//! materially more subpixels than the same office with no agent, both rendered
//! through the IDENTICAL settle sequence so their (time-of-day) skies cancel —
//! what remains is the agent's own paint (recolored sprite, shadow, name label,
//! monitor glow). (A raw distinct-COLOR count was a timezone lottery: the sky palette, keyed off
//! `chrono::Local`, swamped the sprite's few colors at bright hours.) The
//! daemon assertion counts the lobster's exclusive carapace-red pixels (its
//! sprite is not recolored, so its authored RGBs render exactly).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use pixtuoid_core::harness::{Drive, Reach};
use pixtuoid_core::source::daemon::apply_presence;
use pixtuoid_core::source::registry;
use pixtuoid_core::SceneState;
use pixtuoid_scene::embedded_pack::load_sprite_pack;
use pixtuoid_scene::theme::NORMAL;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use pixtuoid::tui::tui_renderer::TuiRenderer;

/// A fixed wall-clock so motion/animation is deterministic across runs.
fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_716_286_800)
}

/// The shared core-crate fixtures tree (a sibling crate). Reused so these
/// tests decode the SAME captured wire bytes the conformance harness pins,
/// never a hand-rolled re-encoding of the format.
fn core_fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("pixtuoid-core")
        .join("tests")
        .join("sources")
        .join("fixtures")
}

fn read_nonblank_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .lines()
        .map(str::to_string)
        .filter(|l| !l.trim().is_empty())
        .collect()
}

fn new_renderer(cols: u16, rows: u16) -> TuiRenderer<TestBackend> {
    let terminal = Terminal::new(TestBackend::new(cols, rows)).expect("test backend");
    TuiRenderer::new(terminal, &NORMAL, vec![])
}

/// Render `scene` through the real renderer for a fixed settle window (so an
/// agent's entry walk finishes and the sprite is firmly at its desk, not
/// mid-doorway) and return the final frame's subpixels. The empty baseline and
/// the occupied frame go through the IDENTICAL sequence, so their (time-of-day)
/// skies are byte-for-byte equal — a later pixel-diff cancels the background and
/// isolates the agent's footprint. The sky reads the wall clock via
/// `chrono::Local` (timezone-dependent), which is precisely why the diff, not a
/// raw color count, is the metric.
fn settled_pixels(scene: &SceneState, cols: u16, rows: u16, now: SystemTime) -> Vec<(u8, u8, u8)> {
    // ~30 frames at a ~30fps step ≈ 1s — bounded on BOTH sides. Lower bound:
    // long enough for the entry walk to finish. Upper bound (load-bearing for
    // the pixel-diff cancelling the sky): SHORTER than LightingState's 5s
    // EMPTY_DEBOUNCE_MS — past that, the empty office's occupancy-driven floor
    // dim would start easing while the occupied office holds its lights, so the
    // two backgrounds would stop being byte-identical and the diff would no
    // longer be purely the agent's paint. Don't raise this past ~150 frames.
    const SETTLE_FRAMES: usize = 30;
    const FRAME_STEP: Duration = Duration::from_millis(33);
    let pack = load_sprite_pack(None).expect("pack");
    let mut r = new_renderer(cols, rows);
    let mut t = now;
    for _ in 0..SETTLE_FRAMES {
        r.render(scene, &pack, t).expect("render office");
        t += FRAME_STEP;
    }
    r.buf()
        .as_slice()
        .iter()
        .map(|px| (px.r, px.g, px.b))
        .collect()
}

/// The empty→occupied pixel-diff floor. A settled, recolored character (body +
/// shadow + name label) repaints ~135–166 subpixels of the frame (the spread is
/// the label width, which the seed's cwd sets); the empty office rendered
/// through the identical sequence differs from itself by 0 (the render is
/// deterministic). 40 sits well above that noise floor and comfortably below
/// the real footprint, tolerant of sprite-art tweaks. This is IMMUNE to the
/// time-of-day sky because the diff cancels the (identical) background — the old
/// distinct-color metric was not, and broke as a timezone lottery once the
/// day/night lighting contrast sharpened (a rich daytime/dawn sky could even net
/// NEGATIVE new colors as the sprite covered unique gradient pixels).
const MIN_SPRITE_PIXELS: usize = 40;

// =====================================================================
// Part A — every AGENT source renders a sprite from real wire bytes
// =====================================================================

/// WHICH of a source's two wires this case drives — and, for a transcript,
/// whether the driver stands in for the watcher's first-sight registration.
///
/// One field where there were three (`DecodeKind` + `Transport` + a five-variant
/// `SeedStart`): the decoder, the transport and the seed keying are not
/// independent choices, and `harness::Drive` makes each pairing a constructor.
/// The four per-source seed variants are gone entirely — the seed is keyed by
/// the source's registry row, so a new CLI needs no new variant here.
enum Wire {
    /// The fixture is a transcript, driven through the source's registry
    /// `LineDecoder` on `Transport::Jsonl`.
    Transcript {
        /// Seed the `SessionStart` the watcher's first-sight would emit. True
        /// for a transcript that carries only activity (CC, Codex, Antigravity,
        /// grok) — a JSONL event for an unknown id is a documented no-op, so
        /// without it the wire drives nothing. False where the fixture's own
        /// head line registers the session (Copilot's `session.start`, omp's
        /// session header).
        seeded: bool,
    },
    /// The fixture is a `hook-payloads.jsonl`, driven through the shared
    /// `decode_hook_payload` on `Transport::Hook`. Never seeded: a hook event
    /// for an unknown session id REGISTERS it (hooks are proof of life).
    Hooks,
}

struct WireCase {
    name: &'static str,
    source: &'static str,
    /// Path to the fixture file, relative to `core_fixtures_root()`.
    fixture: &'static str,
    wire: Wire,
    /// The state classes this fixture's committed wire must drive the slot
    /// through. Derived from a measurement pass judged per-source against each
    /// decoder's documented semantics (not guessed): the eight `&[Active]` rows
    /// carry no permission/dispatch event, so they reach only Active — seven for
    /// lack of such an event in the fixture, plus codewhale which is
    /// architecturally no-Waiting (its `ApprovalRequired` fires no hook, core
    /// CLAUDE.md). The four richer rows carry a real permission (`Waiting`) or
    /// Task dispatch (`Delegating`). Registration alone was the only prior pin —
    /// this proves the wire drives the reducer to the RIGHT lifecycle state, not
    /// merely a slot.
    must_reach: &'static [Reach],
}

/// THE shared proof: real fixture bytes → `harness::Drive` (the SAME pipeline
/// the conformance, corpus and fuzz shells run: the source's registry decoder,
/// its registry-keyed first-sight seed, a real `Reducer` on the correct
/// `Transport`) → ≥1 registered slot → the real `TuiRenderer` (settled ~30
/// frames) → materially more repainted subpixels than the empty office.
///
/// What this shell adds to the other three is the LAST hop: `Driven.scene` is
/// where they stop, and this one paints it.
fn assert_renders_a_sprite(case: &WireCase) {
    let path = core_fixtures_root().join(case.fixture);
    let lines = read_nonblank_lines(&path);

    let driven = match case.wire {
        Wire::Transcript { seeded } => {
            let drive = Drive::transcript_at(case.source, &path).unwrap_or_else(|| {
                panic!(
                    "{}: source {:?} has no line_decoder",
                    case.name, case.source
                )
            });
            let drive = if seeded { drive.seeded() } else { drive };
            drive.at(t0()).lines(lines)
        }
        Wire::Hooks => Drive::hooks(case.source).at(t0()).lines(lines),
    };

    // A committed fixture is bytes we control: every line must parse, decode
    // and not panic. (The other shells assert the same three; only the corpus
    // one reports instead, because its bytes are unbounded.)
    assert_eq!(
        driven.unparseable, 0,
        "{}: {} fixture line(s) are not valid JSON",
        case.name, driven.unparseable
    );
    assert!(
        driven.decode_errors.is_empty(),
        "{}: decode error(s): {:?}",
        case.name,
        driven.decode_errors
    );
    assert!(
        driven.panics.is_empty(),
        "{}: the decoder PANICKED: {:?}",
        case.name,
        driven.panics
    );
    assert!(
        driven.events.len() >= 2,
        "{}: fixture should decode to a registration + activity, got {:?}",
        case.name,
        driven.events
    );
    assert!(
        !driven.scene.agents.is_empty(),
        "{}: the wire bytes must register at least one agent slot (got 0)",
        case.name
    );
    // Lifecycle pin: the wire must drive the reducer THROUGH each documented
    // state class, not merely register a slot. A decoder that degraded to
    // emitting only a bare SessionStart would still pass the registration and
    // paint checks; this is what catches it.
    for want in case.must_reach {
        assert!(
            driven.reached.contains(want),
            "{}: the wire must drive the slot through {want:?}; reached only {:?}",
            case.name,
            driven.reached
        );
    }

    // Render the EMPTY office and the OCCUPIED office through the identical
    // settle sequence, then count the subpixels the agent changed. Both skies
    // are byte-identical (the agent touches neither weather nor stars), so the
    // diff is exactly the character's footprint — proof a sprite was PAINTED,
    // not merely slotted. Sky-INDEPENDENT by construction: the earlier
    // distinct-color count was a timezone lottery (the sky's palette, keyed off
    // `chrono::Local`, dwarfed and collided with the sprite's few colors at
    // bright hours, and a rich dawn even netted a NEGATIVE delta).
    let (cols, rows) = (120, 44);
    let empty = settled_pixels(&SceneState::uniform(8), cols, rows, t0());
    let occupied = settled_pixels(&driven.scene, cols, rows, t0());
    let changed = empty.iter().zip(&occupied).filter(|(a, b)| a != b).count();
    assert!(
        changed >= MIN_SPRITE_PIXELS,
        "{}: an occupied office (from real {} wire bytes) must repaint materially more \
         pixels than the empty office at the same instant (changed={changed}, \
         floor={MIN_SPRITE_PIXELS})",
        case.name,
        case.source
    );
}

/// The wire→pixels matrix: every supported AGENT source, transcript and hook
/// classes both. (The openclaw DAEMON is the 3rd class — its own test below.)
fn agent_cases() -> Vec<WireCase> {
    vec![
        // ---- transcript / JSONL sources (line decoder, Jsonl transport) ----
        WireCase {
            name: "claude_code",
            source: "claude-code",
            fixture: "claude-code/tool-call/01000000-0000-7000-8000-0000000000cc.jsonl",
            // CC's transcript carries only tool activity.
            wire: Wire::Transcript { seeded: true },
            must_reach: &[Reach::Active],
        },
        WireCase {
            name: "codex",
            source: "codex",
            fixture: "codex/tool-run/rollout-2026-01-01T00-00-00-01000000-0000-7000-8000-000000000002.jsonl",
            // Codex's line decoder emits only activity — the watcher's
            // first-sight (or the UserPromptSubmit hook) registers it live.
            wire: Wire::Transcript { seeded: true },
            must_reach: &[Reach::Active],
        },
        WireCase {
            name: "antigravity",
            source: "antigravity",
            fixture: "antigravity/tool-run/transcript.jsonl",
            // Antigravity's decoder emits only activity.
            wire: Wire::Transcript { seeded: true },
            must_reach: &[Reach::Active],
        },
        WireCase {
            name: "copilot",
            source: "copilot",
            fixture: "copilot/tool-run/events.jsonl",
            // The events.jsonl carries a session.start → its own SessionStart.
            wire: Wire::Transcript { seeded: false },
            must_reach: &[Reach::Active],
        },
        WireCase {
            name: "omp",
            source: "omp",
            fixture: "omp/tool-run/2026-07-10T08-00-00-000Z_01990000-0000-7000-8000-000000000002.jsonl",
            // The session header line carries its own SessionStart.
            wire: Wire::Transcript { seeded: false },
            must_reach: &[Reach::Active],
        },
        WireCase {
            name: "grok",
            source: "grok",
            fixture: "grok/tool-run/updates.jsonl",
            // updates.jsonl carries only activity for the root (the watcher's
            // first-sight or any hook registers it in production).
            wire: Wire::Transcript { seeded: true },
            must_reach: &[Reach::Active],
        },
        // ---- hook-only sources (decode_hook_payload, Hook transport) ----
        // A hook event for an unknown session id REGISTERS it (hooks are proof
        // of life), so no seed is needed — the SessionStart envelope (or the
        // first activity hook) synthesizes the slot in the reducer.
        WireCase {
            name: "reasonix",
            source: "reasonix",
            fixture: "reasonix/tool-run/hook-payloads.jsonl",
            wire: Wire::Hooks,
            must_reach: &[Reach::Active, Reach::Delegating],
        },
        WireCase {
            name: "codewhale",
            source: "codewhale",
            fixture: "codewhale/tool-run/hook-payloads.jsonl",
            wire: Wire::Hooks,
            must_reach: &[Reach::Active],
        },
        WireCase {
            name: "opencode",
            source: "opencode",
            fixture: "opencode/session-run/hook-payloads.jsonl",
            wire: Wire::Hooks,
            must_reach: &[Reach::Active, Reach::Waiting, Reach::Delegating],
        },
        WireCase {
            name: "cursor",
            source: "cursor",
            fixture: "cursor/tool-run/hook-payloads.jsonl",
            wire: Wire::Hooks,
            must_reach: &[Reach::Active, Reach::Delegating],
        },
        WireCase {
            name: "hermes",
            source: "hermes",
            fixture: "hermes/tool-run/hook-payloads.jsonl",
            wire: Wire::Hooks,
            must_reach: &[Reach::Active],
        },
        WireCase {
            name: "kimi",
            source: "kimi",
            fixture: "kimi/tool-run/hook-payloads.jsonl",
            wire: Wire::Hooks,
            must_reach: &[Reach::Active, Reach::Waiting],
        },
    ]
}

/// Look up one matrix case by its registered source name. The per-source tests
/// key on this STABLE id — never a positional index into `agent_cases()`, where
/// an insertion/reorder would silently shift every downstream case onto the
/// wrong fixture.
fn agent_case(source: &str) -> WireCase {
    agent_cases()
        .into_iter()
        .find(|c| c.source == source)
        .unwrap_or_else(|| panic!("no wire-to-pixels case for source {source:?}"))
}

/// The matrix is truth-complete: every registered source is
/// exercised by this suite — the agent matrix covers every AGENT source, and
/// each DAEMON source has its own presence test (the openclaw lobster below).
/// A newly registered source with no wire case FAILS here instead of silently
/// shipping untested (the same "registration is not coverage" pin
/// `supported_sources_manifest.rs` gives `site/src/sources.json`).
#[test]
fn wire_matrix_covers_every_registered_source() {
    use std::collections::BTreeSet;

    let agents: BTreeSet<&str> = agent_cases().iter().map(|c| c.source).collect();
    assert_eq!(
        agents.len(),
        agent_cases().len(),
        "duplicate agent_cases rows for one source"
    );
    for source in &agents {
        let d = registry::descriptor_for(source)
            .unwrap_or_else(|| panic!("matrix source {source:?} is not in the registry"));
        assert!(
            !d.is_daemon(),
            "{source}: a daemon belongs in the presence-test list, not the agent matrix"
        );
    }

    // The daemon class is covered by its own presence→lobster test below; a
    // 2nd daemon must gain a presence test AND a row here.
    let daemons_covered = [pixtuoid_core::source::openclaw::SOURCE_NAME];
    for source in daemons_covered {
        let d = registry::descriptor_for(source)
            .unwrap_or_else(|| panic!("presence-test source {source:?} is not in the registry"));
        assert!(d.is_daemon(), "{source}: presence tests are for daemons");
    }

    let covered: BTreeSet<&str> = agents.iter().copied().chain(daemons_covered).collect();
    let registered: BTreeSet<&str> =
        pixtuoid_core::source::registry::registered_source_names().collect();
    assert_eq!(
        covered,
        registered,
        "the wire→pixels suite must cover EVERY registered source.\n  \
         registered but untested (add a WireCase / presence test): {:?}\n  \
         tested but not registered (stale row): {:?}",
        registered.difference(&covered).collect::<Vec<_>>(),
        covered.difference(&registered).collect::<Vec<_>>(),
    );
}

/// Drive the whole matrix in one test so a per-source failure names the source
/// (the `case.name` is woven into every assertion message). Each case is the
/// full wire→pixels chain for its source.
#[test]
fn every_agent_source_renders_a_painted_sprite_from_real_wire() {
    for case in agent_cases() {
        assert_renders_a_sprite(&case);
    }
}

// Per-source tests too, so a single source can be run in isolation
// (`-E 'test(claude_code)'`) and a regression localizes to its own `#[test]`.

#[test]
fn claude_code_transcript_line_renders_a_painted_sprite() {
    assert_renders_a_sprite(&agent_case("claude-code"));
}

#[test]
fn codex_transcript_line_renders_a_painted_sprite() {
    assert_renders_a_sprite(&agent_case("codex"));
}

#[test]
fn antigravity_transcript_line_renders_a_painted_sprite() {
    assert_renders_a_sprite(&agent_case("antigravity"));
}

#[test]
fn copilot_transcript_line_renders_a_painted_sprite() {
    assert_renders_a_sprite(&agent_case("copilot"));
}

#[test]
fn omp_transcript_line_renders_a_painted_sprite() {
    assert_renders_a_sprite(&agent_case("omp"));
}

#[test]
fn grok_transcript_line_renders_a_painted_sprite() {
    assert_renders_a_sprite(&agent_case("grok"));
}

#[test]
fn reasonix_hook_envelope_renders_a_painted_sprite() {
    assert_renders_a_sprite(&agent_case("reasonix"));
}

#[test]
fn codewhale_hook_envelope_renders_a_painted_sprite() {
    assert_renders_a_sprite(&agent_case("codewhale"));
}

#[test]
fn opencode_hook_envelope_renders_a_painted_sprite() {
    assert_renders_a_sprite(&agent_case("opencode"));
}

#[test]
fn cursor_hook_envelope_renders_a_painted_sprite() {
    assert_renders_a_sprite(&agent_case("cursor"));
}

#[test]
fn hermes_hook_envelope_renders_a_painted_sprite() {
    assert_renders_a_sprite(&agent_case("hermes"));
}

#[test]
fn kimi_hook_envelope_renders_a_painted_sprite() {
    assert_renders_a_sprite(&agent_case("kimi"));
}

// =====================================================================
// Part B — OpenClaw DAEMON presence → a painted lobster (3rd class)
// =====================================================================

/// The lobster's exclusive carapace reds (the mascot sprite is NOT recolored,
/// so its authored RGBs render exactly — mirrors `harness.rs::lobster_px`). An
/// empty-agents scene means no recolored agent shirt can collide with these.
fn lobster_px(buf: &pixtuoid_core::sprite::RgbBuffer) -> usize {
    use pixtuoid_core::sprite::Rgb;
    let reds = [
        Rgb {
            r: 0xd2,
            g: 0x40,
            b: 0x2f,
        }, // body
        Rgb {
            r: 0xe8,
            g: 0x55,
            b: 0x40,
        }, // claw
        Rgb {
            r: 0xc8,
            g: 0x38,
            b: 0x28,
        }, // antenna
        Rgb {
            r: 0x9e,
            g: 0x2a,
            b: 0x20,
        }, // shade
    ];
    let mut n = 0;
    for y in 0..buf.height() {
        for x in 0..buf.width() {
            if reds.contains(&buf.get(x, y)) {
                n += 1;
            }
        }
    }
    n
}

/// TEST — OpenClaw (DAEMON / presence source): the captured gateway-lifecycle
/// hook envelopes → the production presence decoder
/// (`openclaw::decode_openclaw_hook_payload`) → `apply_presence` (the SAME
/// sibling-channel seam `runtime/driver.rs` uses — NEVER `Reducer::apply`,
/// which is `AgentId`-pure) → `SceneState.daemons[openclaw]` populated UP →
/// `TuiRenderer` pixels. Asserts the LOBSTER is painted, proving the
/// daemon-presence wire→pixels chain (distinct from the agent JSONL + agent
/// hook classes above).
#[test]
fn openclaw_presence_envelope_renders_a_lobster() {
    let hooks = core_fixtures_root().join("openclaw/gateway_lifecycle/hook-payloads.jsonl");

    // The fixture's last two envelopes are session_end → gateway_stop, which
    // would leave the daemon Down (and the lobster walking out / gone). We want
    // the daemon UP, so apply only through the in-session run (gateway_start ..
    // agent_end) — exactly the live "gateway is serving" state. Each remaining
    // envelope is decoded via the production presence decoder.
    let lines = read_nonblank_lines(&hooks);
    let mut scene = SceneState::uniform(16);
    let now = t0();
    let mut applied = 0;
    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line).expect("openclaw hook json");
        // Stop at session teardown so the asserted scene is a LIVE gateway.
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if ty == "session_end" || ty == "gateway_stop" {
            break;
        }
        let decoded = pixtuoid_core::source::openclaw::decode_openclaw_hook_payload(&v)
            .expect("decode_openclaw_hook_payload");
        // The captured wire carries its own gateway identity — the key is DECODED
        // here, never assumed, so a fixture with two ports would render two mascots.
        let key = pixtuoid_core::source::daemon::DaemonInstanceKey::new(
            pixtuoid_core::source::openclaw::SOURCE_NAME,
            decoded.instance.clone(),
        );
        for update in decoded.updates {
            apply_presence(&mut scene, &key, update, now);
            applied += 1;
        }
    }
    assert!(
        applied > 0,
        "the openclaw fixture must decode to presence deltas (got 0)"
    );

    // The presence must have populated the daemon map UP (not Down) — the live
    // gateway whose lobster scuttles the floor.
    let (_, _, presence) = scene
        .daemons()
        .find(|(source, _, _)| *source == pixtuoid_core::source::openclaw::SOURCE_NAME)
        .expect("openclaw presence must be populated");
    assert_ne!(
        presence.liveness,
        pixtuoid_core::state::DaemonLiveness::Down,
        "the in-session presence stream must leave the gateway UP"
    );

    // Render the scene. `entered_at` == `now` means the lobster is mid walk-in;
    // settle a few frames at a LATER wall-clock so it's scuttling the floor
    // (well past the elevator), then assert the carapace reds are painted.
    let pack = load_sprite_pack(None).expect("pack");
    let mut r = new_renderer(160, 80);
    let mut t = now + Duration::from_secs(20);
    for _ in 0..10 {
        r.render(&scene, &pack, t).expect("render gateway office");
        t += Duration::from_millis(130);
    }
    assert!(
        lobster_px(r.buf()) > 10,
        "a live OpenClaw gateway (from real presence wire bytes) must paint the lobster mascot"
    );
}
