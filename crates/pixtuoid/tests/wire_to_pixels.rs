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
//! The agent assertion is a color-diversity FLOOR: an occupied frame must show
//! materially more distinct colors than the same office with no agent (the
//! recolored sprite + its shadow/label add colors absent from the empty floor).
//! The daemon assertion counts the lobster's exclusive carapace-red pixels (its
//! sprite is not recolored, so its authored RGBs render exactly).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use pixtuoid_core::source::daemon::apply_presence;
use pixtuoid_core::source::decoder::decode_hook_payload;
use pixtuoid_core::source::{registry, AgentEvent};
use pixtuoid_core::{AgentId, Reducer, SceneState, Transport};
use pixtuoid_scene::embedded_pack::load_sprite_pack;
use pixtuoid_scene::theme::NORMAL;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use pixtuoid::tui::tui_renderer::TuiRenderer;
use pixtuoid_core::Renderer;

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

/// Count distinct RGB triples in the rendered buffer — the visible-paint metric.
fn distinct_colors(buf: &pixtuoid_core::sprite::RgbBuffer) -> usize {
    let mut set = std::collections::HashSet::new();
    for px in buf.as_slice() {
        set.insert((px.r, px.g, px.b));
    }
    set.len()
}

fn new_renderer(cols: u16, rows: u16) -> TuiRenderer<TestBackend> {
    let terminal = Terminal::new(TestBackend::new(cols, rows)).expect("test backend");
    TuiRenderer::new(terminal, &NORMAL, vec![])
}

/// Render an EMPTY office to pixels and return its distinct-color count — the
/// baseline every agent wire test compares its occupied frame against.
fn empty_office_color_count(cols: u16, rows: u16, now: SystemTime) -> usize {
    let mut r = new_renderer(cols, rows);
    let pack = load_sprite_pack(None).expect("pack");
    let empty = SceneState::uniform(8);
    r.render(&empty, &pack, now).expect("render empty office");
    distinct_colors(r.buf())
}

/// Drive a scene through the renderer for a few frames so the agent's entry
/// walk advances and the sprite is firmly on the floor (not mid-doorway), then
/// return the distinct-color count of the settled frame.
fn render_settled_color_count(
    r: &mut TuiRenderer<TestBackend>,
    scene: &SceneState,
    now: SystemTime,
) -> usize {
    let pack = load_sprite_pack(None).expect("pack");
    let mut t = now;
    for _ in 0..30 {
        r.render(scene, &pack, t).expect("render occupied office");
        t += Duration::from_millis(33);
    }
    distinct_colors(r.buf())
}

// =====================================================================
// Part A — every AGENT source renders a sprite from real wire bytes
// =====================================================================

/// How a source's wire bytes reach `AgentEvent`s.
enum DecodeKind {
    /// A transcript / JSONL line decoder (from the source registry — the SAME
    /// `LineDecoder` the conformance harness runs). Folded on `Transport::Jsonl`.
    Transcript,
    /// A hook envelope through `decode_hook_payload` (the registry dispatcher).
    /// Folded on `Transport::Hook`.
    Hook,
}

/// Whether the fixture's own decoded stream registers a slot, or a synthetic
/// `SessionStart` must seed one first (JSONL events for an unknown id are a
/// no-op in the reducer — a transcript that carries only tool activity, like
/// claude-code's and antigravity's, never registers itself).
enum SeedStart {
    /// The fixture's decoded events include a `SessionStart` (copilot) OR the
    /// hook path synthesizes the slot from an unknown id (every hook source).
    /// (Codex is NOT here — its line decoder emits only activity, so it uses
    /// `CodexUuid` below.)
    None,
    /// Seed a `SessionStart` keyed `AgentId::from_parts(source, <logical-path>)`
    /// — antigravity's decoder keys exactly this way.
    FromParts,
    /// Seed a `SessionStart` keyed the claude-code way (`cc_id_from_path`, the
    /// transcript filename stem == the session UUID).
    CcStem,
    /// Seed a `SessionStart` keyed the codex way (`codex_id_from_path`, the
    /// rollout filename UUID). Codex's LINE decoder emits only activity — the
    /// watcher's first-sight (or the UserPromptSubmit hook) supplies the
    /// SessionStart in production; this seed stands in for that registration so
    /// the transcript path is exercised in isolation.
    CodexUuid,
}

struct WireCase {
    name: &'static str,
    source: &'static str,
    /// Path to the fixture file, relative to `core_fixtures_root()`.
    fixture: &'static str,
    decode: DecodeKind,
    transport: Transport,
    seed: SeedStart,
}

/// Build the synthetic `SessionStart` a transcript-only source needs (its slot
/// is otherwise never registered). The agent id is keyed exactly as the
/// source's own decoder keys, so the seed and the decoded activity coalesce to
/// one agent — exactly as production keys them.
fn seed_session_start(seed: &SeedStart, source: &str, logical: &str) -> Option<AgentEvent> {
    let agent_id = match seed {
        SeedStart::None => return None,
        SeedStart::FromParts => AgentId::from_parts(source, logical),
        SeedStart::CcStem => {
            use pixtuoid_core::source::claude_code::cc_id_from_path;
            AgentId::from_parts(source, &cc_id_from_path(Path::new(logical)))
        }
        SeedStart::CodexUuid => {
            use pixtuoid_core::source::codex::codex_id_from_path;
            AgentId::from_parts(source, &codex_id_from_path(Path::new(logical)))
        }
    };
    Some(AgentEvent::SessionStart {
        agent_id,
        source: source.to_string(),
        session_id: "wire-to-pixels-seed".to_string(),
        cwd: PathBuf::from("/home/user/demo-project"),
        parent_id: None,
    })
}

/// THE shared proof: real fixture bytes → the production decoder its conformance
/// test uses → a real `Reducer` on the correct `Transport` → ≥1 registered slot
/// → the real `TuiRenderer` (settled ~30 frames) → more distinct colors than the
/// empty-office baseline (a character was painted, not merely slotted).
fn assert_renders_a_sprite(case: &WireCase) {
    let path = core_fixtures_root().join(case.fixture);
    // The decoder's logical key — the transcript path for a JSONL source (its
    // `AgentId` is a hash of it). For a hook source the per-line envelope keys
    // itself, so the value is unused; pass the path for symmetry.
    let logical = path.to_string_lossy().into_owned();

    let mut events = Vec::new();
    if let Some(start) = seed_session_start(&case.seed, case.source, &logical) {
        events.push(start);
    }

    for line in read_nonblank_lines(&path) {
        let v: serde_json::Value =
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("{}: bad json: {e}", case.name));
        let decoded = match case.decode {
            DecodeKind::Transcript => {
                let decode = registry::descriptor_for(case.source)
                    .and_then(|d| d.line_decoder())
                    .unwrap_or_else(|| {
                        panic!(
                            "{}: source {:?} has no line_decoder",
                            case.name, case.source
                        )
                    });
                decode(&logical, case.source, v)
                    .unwrap_or_else(|e| panic!("{}: decode_line: {e}", case.name))
            }
            DecodeKind::Hook => decode_hook_payload(v)
                .unwrap_or_else(|e| panic!("{}: decode_hook_payload: {e}", case.name)),
        };
        events.extend(decoded);
    }
    assert!(
        events.len() >= 2,
        "{}: fixture should decode to a registration + activity, got {events:?}",
        case.name
    );

    // Fold through a real reducer into a real SceneState, on the SAME transport
    // production tags this source's events with.
    let mut scene = SceneState::uniform(8);
    let mut reducer = Reducer::new();
    for ev in events {
        reducer.apply(&mut scene, ev, t0(), case.transport);
    }
    assert!(
        !scene.agents.is_empty(),
        "{}: the wire bytes must register at least one agent slot (got 0)",
        case.name
    );

    // Render to pixels and assert a sprite was painted (color floor). An empty
    // office can't gain colors, so any positive delta is real paint; the +4
    // floor is the measured minimum a settled, recolored character adds (a
    // just-completed single-tool sprite is the leanest at +4, an actively-busy
    // one runs +6) — well clear of an unpainted/no-slot frame's +0.
    let (cols, rows) = (120, 44);
    let baseline = empty_office_color_count(cols, rows, t0());
    let mut r = new_renderer(cols, rows);
    let occupied = render_settled_color_count(&mut r, &scene, t0());
    assert!(
        occupied >= baseline + 4,
        "{}: an occupied office (from real {} wire bytes) must paint materially more \
         distinct colors than the empty baseline (empty={baseline}, occupied={occupied})",
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
            decode: DecodeKind::Transcript,
            transport: Transport::Jsonl,
            // CC's transcript carries only tool activity → seed a SessionStart
            // keyed the cc_id_from_path (filename-stem) way.
            seed: SeedStart::CcStem,
        },
        WireCase {
            name: "codex",
            source: "codex",
            fixture: "codex/tool-run/rollout-2026-01-01T00-00-00-01000000-0000-7000-8000-000000000002.jsonl",
            decode: DecodeKind::Transcript,
            transport: Transport::Jsonl,
            // Codex's line decoder emits only activity — the watcher first-sight
            // (or the UserPromptSubmit hook) registers the slot in production;
            // seed a SessionStart keyed the codex_id_from_path (rollout-UUID) way.
            seed: SeedStart::CodexUuid,
        },
        WireCase {
            name: "antigravity",
            source: "antigravity",
            fixture: "antigravity/tool-run/transcript.jsonl",
            decode: DecodeKind::Transcript,
            transport: Transport::Jsonl,
            // Antigravity's decoder emits only activity → seed keyed from_parts.
            seed: SeedStart::FromParts,
        },
        WireCase {
            name: "copilot",
            source: "copilot",
            fixture: "copilot/tool-run/events.jsonl",
            decode: DecodeKind::Transcript,
            transport: Transport::Jsonl,
            // The events.jsonl carries a session.start → its own SessionStart.
            seed: SeedStart::None,
        },
        // ---- hook-only sources (decode_hook_payload, Hook transport) ----
        // A hook event for an unknown session id REGISTERS it (hooks are proof
        // of life), so no seed is needed — the SessionStart envelope (or the
        // first activity hook) synthesizes the slot in the reducer.
        WireCase {
            name: "reasonix",
            source: "reasonix",
            fixture: "reasonix/tool-run/hook-payloads.jsonl",
            decode: DecodeKind::Hook,
            transport: Transport::Hook,
            seed: SeedStart::None,
        },
        WireCase {
            name: "codewhale",
            source: "codewhale",
            fixture: "codewhale/tool-run/hook-payloads.jsonl",
            decode: DecodeKind::Hook,
            transport: Transport::Hook,
            seed: SeedStart::None,
        },
        WireCase {
            name: "opencode",
            source: "opencode",
            fixture: "opencode/session-run/hook-payloads.jsonl",
            decode: DecodeKind::Hook,
            transport: Transport::Hook,
            seed: SeedStart::None,
        },
        WireCase {
            name: "cursor",
            source: "cursor",
            fixture: "cursor/tool-run/hook-payloads.jsonl",
            decode: DecodeKind::Hook,
            transport: Transport::Hook,
            seed: SeedStart::None,
        },
        WireCase {
            name: "hermes",
            source: "hermes",
            fixture: "hermes/tool-run/hook-payloads.jsonl",
            decode: DecodeKind::Hook,
            transport: Transport::Hook,
            seed: SeedStart::None,
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

/// The matrix is truth-complete: every source in `REGISTERED_SOURCES` is
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
    let registered: BTreeSet<&str> = pixtuoid_core::source::REGISTERED_SOURCES
        .iter()
        .copied()
        .collect();
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
        let updates = pixtuoid_core::source::openclaw::decode_openclaw_hook_payload(&v)
            .expect("decode_openclaw_hook_payload");
        for update in updates {
            apply_presence(
                &mut scene,
                pixtuoid_core::source::openclaw::SOURCE_NAME,
                update,
                now,
            );
            applied += 1;
        }
    }
    assert!(
        applied > 0,
        "the openclaw fixture must decode to presence deltas (got 0)"
    );

    // The presence must have populated the daemon map UP (not Down) — the live
    // gateway whose lobster scuttles the floor.
    let presence = scene
        .daemons()
        .get(pixtuoid_core::source::openclaw::SOURCE_NAME)
        .expect("openclaw presence must be populated");
    assert_ne!(
        presence.state,
        pixtuoid_core::state::DaemonState::Down,
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
