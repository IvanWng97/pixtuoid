//! TRUE end-to-end: real wire bytes → the production decoder → a real
//! `Reducer` → a `SceneState` → the REAL `TuiRenderer` (ratatui `TestBackend`)
//! → an actual pixel buffer, asserting a sprite was PAINTED.
//!
//! Three transport classes are covered:
//!   - JSONL transcript  → `Transport::Jsonl` (every transcript-bearing registered source)
//!   - agent hook         → `Transport::Hook`  (every hook-only registered source)
//!   - daemon presence    → the sibling channel (`apply_presence`) (openclaw)

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use pixtuoid_core::harness::{Drive, Reach, DRIVEN_DESKS};
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

/// The core crate's source-test tree, so these tests decode the SAME captured
/// wire bytes that crate pins, never a hand-rolled re-encoding. It is the tree
/// and not `fixtures/` alone because a two-session capture cannot live under the
/// conformance harness's one-AgentId rule.
fn core_fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("pixtuoid-core")
        .join("tests")
        .join("sources")
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

/// Render `scene` through the real renderer for a fixed settle window and return
/// the final frame's subpixels. The empty baseline and the occupied frame go
/// through the IDENTICAL sequence, so a later pixel-diff cancels the
/// (timezone-dependent) sky and isolates the agent's footprint.
fn settled_pixels(scene: &SceneState, cols: u16, rows: u16, now: SystemTime) -> Vec<(u8, u8, u8)> {
    // Bounded on BOTH sides: long enough for the entry walk to finish, and
    // SHORTER than LightingState's 5s EMPTY_DEBOUNCE_MS — past that the empty
    // office's floor dim would start easing while the occupied office holds its
    // lights, so the two backgrounds would stop being byte-identical and the diff
    // would no longer be purely the agent's paint. Don't raise past ~150 frames.
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

/// The empty→occupied pixel-diff floor: well above the deterministic render's
/// zero-diff noise floor, and comfortably below a settled character's real
/// footprint so sprite-art tweaks don't trip it. IMMUNE to the time-of-day sky
/// because the diff cancels the (identical) background.
const MIN_SPRITE_PIXELS: usize = 40;

/// WHICH of a source's two wires this case drives — and, for a transcript,
/// whether the driver stands in for the watcher's first-sight registration.
enum Wire {
    /// The fixture is a transcript, driven through the source's registry
    /// `LineDecoder` on `Transport::Jsonl`.
    Transcript {
        /// Seed the `SessionStart` the watcher's first-sight would emit. Needed
        /// for a transcript that carries only activity: a JSONL event for an
        /// unknown id is a documented no-op, so without it the wire drives
        /// nothing. False where the fixture's own head line registers the session.
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
    /// through — judged per-source against each decoder's documented semantics,
    /// so the wire is proven to reach the RIGHT lifecycle state, not merely a slot.
    must_reach: &'static [Reach],
}

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
        Wire::Hooks => Drive::hooks().at(t0()).lines(lines),
    };

    driven.assert_clean(case.name);
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
    for want in case.must_reach {
        assert!(
            driven.reached.contains(want),
            "{}: the wire must drive the slot through {want:?}; reached only {:?}",
            case.name,
            driven.reached
        );
    }

    // Both skies are byte-identical (the agent touches neither weather nor
    // stars), so the diff is exactly the character's footprint.
    let (cols, rows) = (120, 44);
    // The empty baseline must be shaped like the driven scene (same desk
    // capacity), or the diff would carry a layout difference as well as the agent.
    let empty = settled_pixels(&SceneState::uniform(DRIVEN_DESKS), cols, rows, t0());
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
/// classes both.
fn agent_cases() -> Vec<WireCase> {
    vec![
        WireCase {
            name: "claude_code",
            source: "claude-code",
            fixture: "fixtures/claude-code/tool-call/01000000-0000-7000-8000-0000000000cc.jsonl",
            wire: Wire::Transcript { seeded: true },
            must_reach: &[Reach::Active],
        },
        WireCase {
            name: "codex",
            source: "codex",
            fixture: "fixtures/codex/tool-run/rollout-2026-01-01T00-00-00-01000000-0000-7000-8000-000000000002.jsonl",
            wire: Wire::Transcript { seeded: true },
            must_reach: &[Reach::Active],
        },
        WireCase {
            name: "antigravity",
            source: "antigravity",
            fixture: "fixtures/antigravity/tool-run/transcript.jsonl",
            wire: Wire::Transcript { seeded: true },
            must_reach: &[Reach::Active],
        },
        WireCase {
            name: "copilot",
            source: "copilot",
            fixture: "fixtures/copilot/tool-run/events.jsonl",
            wire: Wire::Transcript { seeded: false },
            must_reach: &[Reach::Active],
        },
        WireCase {
            name: "omp",
            source: "omp",
            fixture: "fixtures/omp/tool-run/2026-07-10T08-00-00-000Z_01990000-0000-7000-8000-000000000002.jsonl",
            wire: Wire::Transcript { seeded: false },
            must_reach: &[Reach::Active],
        },
        WireCase {
            name: "grok",
            source: "grok",
            fixture: "fixtures/grok/tool-run/updates.jsonl",
            wire: Wire::Transcript { seeded: true },
            must_reach: &[Reach::Active],
        },
        WireCase {
            name: "reasonix",
            source: "reasonix",
            fixture: "fixtures/reasonix/delegation/hook-payloads.jsonl",
            wire: Wire::Hooks,
            must_reach: &[Reach::Active, Reach::Delegating],
        },
        WireCase {
            name: "codewhale",
            source: "codewhale",
            fixture: "fixtures/codewhale/tool-run/hook-payloads.jsonl",
            wire: Wire::Hooks,
            must_reach: &[Reach::Active],
        },
        WireCase {
            name: "opencode",
            source: "opencode",
            fixture: "fixtures/opencode/session-run/hook-payloads.jsonl",
            wire: Wire::Hooks,
            must_reach: &[Reach::Active, Reach::Waiting, Reach::Delegating],
        },
        WireCase {
            name: "cursor",
            source: "cursor",
            // The DELEGATING capture, not the tool-run one: `Delegating` needs a
            // Task dispatch, and a real cursor turn only delegates when asked.
            // Two sessions, so it cannot live under `fixtures/`.
            fixture: "cursor/fixtures/hook-payloads.jsonl",
            wire: Wire::Hooks,
            must_reach: &[Reach::Active, Reach::Delegating],
        },
        WireCase {
            name: "hermes",
            source: "hermes",
            fixture: "fixtures/hermes/tool-run/hook-payloads.jsonl",
            wire: Wire::Hooks,
            must_reach: &[Reach::Active],
        },
        WireCase {
            name: "kimi",
            source: "kimi",
            // The permission fixture, because a HEADLESS kimi run cannot reach
            // Waiting — it auto-approves even `rm -rf`, so `PermissionRequest`
            // only fires for a human at a real TUI. Both fixtures are captures;
            // this is the one whose turn covers both classes.
            fixture: "fixtures/kimi/permission-flow/hook-payloads.jsonl",
            wire: Wire::Hooks,
            must_reach: &[Reach::Active, Reach::Waiting],
        },
    ]
}

/// Look up one matrix case by its registered source name — never a positional
/// index into `agent_cases()`, where an insertion would silently shift every
/// downstream case onto the wrong fixture.
fn agent_case(source: &str) -> WireCase {
    agent_cases()
        .into_iter()
        .find(|c| c.source == source)
        .unwrap_or_else(|| panic!("no wire-to-pixels case for source {source:?}"))
}

/// The matrix is truth-complete: a newly registered source with no wire case
/// FAILS here instead of silently shipping untested.
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

    // A 2nd daemon must gain a presence test AND a row here.
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

#[test]
fn every_agent_source_renders_a_painted_sprite_from_real_wire() {
    for case in agent_cases() {
        assert_renders_a_sprite(&case);
    }
}

// Per-source tests too, so a single source can be run in isolation and a
// regression localizes to its own `#[test]`.

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

/// Cells the gateway presence ADDS to an otherwise identical office.
///
/// A differential, not a colour probe: the foreground wash recolours every
/// drawable, so matching the authored carapace RGB would have to predict the
/// wash — coupling a presence assertion to the lighting model and expecting a
/// value the code under test computed.
fn lobster_px(
    r: &mut TuiRenderer<TestBackend>,
    scene: &SceneState,
    pack: &pixtuoid_core::sprite::format::Pack,
    times: &[SystemTime],
) -> usize {
    let baseline = SceneState::uniform(16);
    let settle = |r: &mut TuiRenderer<TestBackend>, s: &SceneState| {
        for &t in times {
            r.render(s, pack, t).expect("render office");
        }
    };
    settle(r, &baseline);
    let base = r.buf().clone();
    settle(r, scene);
    let buf = r.buf();
    (0..buf.height())
        .flat_map(|y| (0..buf.width()).map(move |x| (x, y)))
        .filter(|&(x, y)| buf.get(x, y) != base.get(x, y))
        .count()
}

/// The daemon/presence class: `apply_presence` is the sibling-channel seam
/// `runtime/driver.rs` uses — NEVER `Reducer::apply`, which is `AgentId`-pure.
#[test]
fn openclaw_presence_envelope_renders_a_lobster() {
    let hooks =
        core_fixtures_root().join("fixtures/openclaw/gateway_lifecycle/hook-payloads.jsonl");

    // The fixture's last two envelopes are session_end → gateway_stop, which would
    // leave the daemon Down; stop before them so the asserted scene is a LIVE
    // gateway.
    let lines = read_nonblank_lines(&hooks);
    let mut scene = SceneState::uniform(16);
    let now = t0();
    let mut applied = 0;
    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line).expect("openclaw hook json");
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if ty == "session_end" || ty == "gateway_stop" {
            break;
        }
        let decoded = pixtuoid_core::source::openclaw::decode_openclaw_hook_payload(&v)
            .expect("decode_openclaw_hook_payload");
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

    let (_, _, presence) = scene
        .daemons()
        .find(|(source, _, _)| *source == pixtuoid_core::source::openclaw::SOURCE_NAME)
        .expect("openclaw presence must be populated");
    assert_ne!(
        presence.liveness,
        pixtuoid_core::state::DaemonLiveness::Down,
        "the in-session presence stream must leave the gateway UP"
    );

    // `entered_at` == `now` means the lobster is mid walk-in; settle at a LATER
    // wall-clock so it's scuttling the floor, well past the elevator.
    let pack = load_sprite_pack(None).expect("pack");
    let mut r = new_renderer(160, 80);
    let times: Vec<_> = (0..10)
        .map(|k| now + Duration::from_secs(20) + Duration::from_millis(k * 130))
        .collect();
    assert!(
        lobster_px(&mut r, &scene, &pack, &times) > 10,
        "a live OpenClaw gateway (from real presence wire bytes) must paint the lobster mascot"
    );
}
