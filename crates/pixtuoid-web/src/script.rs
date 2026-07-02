//! The hero's scripted cast — a deterministic, LOOPED event timeline fed
//! through the REAL `Reducer`, so the web office behaves exactly like the app
//! (walk-ins, desk assignment, Active debounce, waiting bubbles, walkouts)
//! and can never drift from real behavior. No tokio, no sources — just the
//! same `(Transport, AgentEvent)` stream a live CLI would produce.
//!
//! Beat structure (one `LOOP_MS` cycle):
//! - staggered `SessionStart`s walk the cast in over the first ~20s;
//! - each agent runs chained tool bursts (`ActivityStart`→`ActivityEnd` with
//!   gaps < the reducer's 1.5s Active debounce, so work reads continuous)
//!   interleaved with idle stretches (wander/coffee/meetings emerge from the
//!   engine, not the script);
//! - one agent parks on a permission `Waiting` mid-loop;
//! - one agent `SessionEnd`s and a "new hire" starts later (door traffic).
//!
//! On loop wrap the same events replay: a `SessionStart` for a live slot is
//! a reducer no-op (backfill arm), the ended agent re-enters (resurrect /
//! fresh registration), so the office stays coherent forever.

use std::path::PathBuf;

use pixtuoid_core::source::daemon::DaemonPresenceUpdate;
use pixtuoid_core::source::{claude_code, codex, cursor, opencode};
use pixtuoid_core::{AgentEvent, AgentId, ToolDetail, Transport};

/// One scripted beat: fires `at_ms` into the current loop.
pub(crate) struct Beat {
    pub at_ms: u64,
    pub transport: Transport,
    pub event: AgentEvent,
}

/// Loop length. Long enough that the cycle doesn't read as a loop (the
/// ambient layer — wander, pets, weather — is unsynchronized with it anyway).
pub(crate) const LOOP_MS: u64 = 120_000;

/// A cast member: a source CLI + a repo-ish cwd (drives the label AND the
/// Team-Palette outfit, which keys on cwd). Sources reference the modules'
/// `SOURCE_NAME` consts — a hand-typed string here silently misses the
/// registry and the label falls back to the RAW string (`claude_code·api`
/// instead of `cc·api` — a review-caught, test-invisible defect class).
const CAST: &[(&str, &str, &str)] = &[
    // (source, session key, cwd)
    (claude_code::SOURCE_NAME, "hero-cc-api", "/work/api"),
    (claude_code::SOURCE_NAME, "hero-cc-web", "/work/webapp"),
    (codex::SOURCE_NAME, "hero-cx-infra", "/work/infra"),
    (claude_code::SOURCE_NAME, "hero-cc-data", "/work/data"),
    (opencode::SOURCE_NAME, "hero-oc-cli", "/work/cli"),
    (codex::SOURCE_NAME, "hero-cx-web", "/work/webapp"),
    (cursor::SOURCE_NAME, "hero-cu-docs", "/work/docs"),
    (claude_code::SOURCE_NAME, "hero-cc-infra", "/work/infra"),
];

pub(crate) fn cast_id(i: usize) -> AgentId {
    let (source, key, _) = CAST[i];
    AgentId::from_parts(source, key)
}

fn session_start(i: usize) -> AgentEvent {
    let (source, key, cwd) = CAST[i];
    AgentEvent::SessionStart {
        agent_id: cast_id(i),
        source: source.to_string(),
        session_id: key.to_string(),
        cwd: PathBuf::from(cwd),
        parent_id: None,
    }
}

/// One tool burst's start→end span.
const BURST_MS: u64 = 900;
/// Start-to-start spacing of chained bursts inside a spell. The
/// `BURST_SPACING_MS - BURST_MS` idle gap (300ms) must stay UNDER the
/// reducer's `ACTIVE_GRACE_WINDOW` (1.5s) or the whole cast visibly flickers
/// Active↔Idle — the pairing is pinned by
/// `burst_gap_stays_under_the_reducer_debounce` below, so a core debounce
/// change fails a test instead of silently degrading the hero.
const BURST_SPACING_MS: u64 = 1200;

fn tool(i: usize, at_ms: u64, tuid: &str, display: &str) -> [Beat; 2] {
    [
        Beat {
            at_ms,
            transport: Transport::Hook,
            event: AgentEvent::ActivityStart {
                agent_id: cast_id(i),
                tool_use_id: Some(format!("hero-{i}-{tuid}")),
                detail: Some(ToolDetail::Generic {
                    display: display.to_string(),
                }),
            },
        },
        Beat {
            at_ms: at_ms + BURST_MS,
            transport: Transport::Hook,
            event: AgentEvent::ActivityEnd {
                agent_id: cast_id(i),
                tool_use_id: Some(format!("hero-{i}-{tuid}")),
            },
        },
    ]
}

/// A work SPELL: `n` chained bursts starting at `at_ms` (each 1.2s apart →
/// continuously Active for ~1.2n seconds, then the agent settles Idle and the
/// engine's wander takes over until the next spell).
fn spell(beats: &mut Vec<Beat>, i: usize, at_ms: u64, n: u64, tools: &[&str]) {
    for k in 0..n {
        let display = tools[(k as usize) % tools.len()];
        let t = format!("s{at_ms}-{k}");
        beats.extend(tool(i, at_ms + k * BURST_SPACING_MS, &t, display));
    }
}

/// How long a visitor hire works before heading out (`SessionEnd`; the
/// reducer's exit grace then walks them to the elevator).
pub(crate) const HIRE_STAY_MS: u64 = 70_000;

/// One visitor-hired coworker's lifecycle (#434), as offsets from the hire
/// instant: walk in now, three short work spells, leave at
/// [`HIRE_STAY_MS`]. Reuses the cast's burst shape — the SAME
/// `BURST_MS`/`BURST_SPACING_MS` consts, so the 300ms idle gap stays under
/// the reducer's Active debounce and the `burst_gap_stays_under_the_reducer_
/// debounce` pin covers hires too (two copies of those literals would be the
/// latent-drift class the workspace magic-number rule names). The cwd is the
/// hire's own ("/work/yours") so Team Palette gives hires a distinct outfit
/// family.
pub(crate) fn hire_beats(id: AgentId, session: String) -> Vec<(u64, AgentEvent)> {
    let mut out: Vec<(u64, AgentEvent)> = Vec::new();
    out.push((
        0,
        AgentEvent::SessionStart {
            agent_id: id,
            source: claude_code::SOURCE_NAME.to_string(),
            session_id: session,
            cwd: PathBuf::from("/work/yours"),
            parent_id: None,
        },
    ));
    // Three short work spells across the stay, spaced so the hire also
    // idles/wanders like everyone else.
    for (k, start) in [8_000u64, 28_000, 50_000].into_iter().enumerate() {
        for b in 0..4u64 {
            let at = start + b * BURST_SPACING_MS;
            let tuid = format!("hire-{k}-{b}");
            out.push((
                at,
                AgentEvent::ActivityStart {
                    agent_id: id,
                    tool_use_id: Some(tuid.clone()),
                    detail: Some(ToolDetail::Generic {
                        display: "Edit".to_string(),
                    }),
                },
            ));
            out.push((
                at + BURST_MS,
                AgentEvent::ActivityEnd {
                    agent_id: id,
                    tool_use_id: Some(tuid),
                },
            ));
        }
    }
    out.push((
        HIRE_STAY_MS,
        AgentEvent::SessionEnd {
            agent_id: id,
            as_child: false,
        },
    ));
    out
}

/// One scripted presence beat for the OpenClaw gateway mascot. Presence is
/// deliberately NOT a [`Beat`]/`AgentEvent` (invariant #2: the one event
/// channel is `AgentId`-pure) — these ride their own lane and land through
/// the REAL `source::daemon::apply_presence` state machine, so the lobster's
/// enter/busy/degraded/leave motion is the app's, not a scripted fake.
pub(crate) struct PresenceBeat {
    pub at_ms: u64,
    pub update: DaemonPresenceUpdate,
}

/// The lobster's loop (#434): the OpenClaw mascot scuttles in from the
/// elevator mid-loop, shuttles through two busy runs, and walks out before
/// the wrap — so every loop replays a clean enter animation (GatewayUp after
/// Down re-anchors `entered_at`). The wide poster's instant (100s) lands in
/// the idle amble between run 2 ending (96s) and the walk-out (112s).
pub(crate) fn lobster_beats() -> Vec<PresenceBeat> {
    use DaemonPresenceUpdate::*;
    [
        (25_000, GatewayUp { pid: None }),
        (
            40_000,
            RunStarted {
                run_key: "hero-run-1".into(),
            },
        ),
        (
            62_000,
            RunEnded {
                run_key: "hero-run-1".into(),
            },
        ),
        (
            78_000,
            RunStarted {
                run_key: "hero-run-2".into(),
            },
        ),
        (
            96_000,
            RunEnded {
                run_key: "hero-run-2".into(),
            },
        ),
        (112_000, GatewayDown),
    ]
    .into_iter()
    .map(|(at_ms, update)| PresenceBeat { at_ms, update })
    .collect()
}

/// Build one loop of the hero timeline, sorted by `at_ms`.
pub(crate) fn hero_script() -> Vec<Beat> {
    let mut b: Vec<Beat> = Vec::new();

    // Walk-ins: staggered so the door + corridor animate one by one.
    for (i, delay) in [0u64, 2_500, 5_500, 9_000, 12_000, 15_500, 19_000]
        .iter()
        .enumerate()
    {
        b.push(Beat {
            at_ms: *delay,
            transport: Transport::Jsonl,
            event: session_start(i),
        });
    }

    // Work spells per agent — offsets chosen so at any instant roughly half
    // the office types while the rest wander/idle (meetings + coffee emerge).
    spell(
        &mut b,
        0,
        6_000,
        8,
        &["Bash: cargo test", "Edit main.rs", "Read lib.rs"],
    );
    spell(&mut b, 1, 10_000, 6, &["Edit App.tsx", "Bash: pnpm build"]);
    spell(
        &mut b,
        2,
        14_000,
        10,
        &["Bash: terraform plan", "Read modules.tf"],
    );
    spell(&mut b, 3, 20_000, 6, &["Bash: dbt run", "Edit models.sql"]);
    spell(&mut b, 4, 24_000, 8, &["Edit cmd.rs", "Bash: cargo clippy"]);
    spell(&mut b, 5, 30_000, 6, &["Read index.ts", "Edit routes.ts"]);
    spell(
        &mut b,
        0,
        42_000,
        10,
        &["Write api.rs", "Bash: cargo check"],
    );
    spell(
        &mut b,
        2,
        55_000,
        8,
        &["Bash: kubectl apply", "Read deploy.yml"],
    );
    spell(
        &mut b,
        1,
        62_000,
        8,
        &["Edit styles.css", "Bash: pnpm test"],
    );
    spell(&mut b, 6, 40_000, 6, &["Edit README.md", "Read guide.md"]);
    spell(
        &mut b,
        4,
        70_000,
        8,
        &["Bash: cargo build", "Edit parse.rs"],
    );
    spell(&mut b, 3, 80_000, 6, &["Read schema.sql", "Edit etl.py"]);
    spell(&mut b, 5, 90_000, 8, &["Bash: vitest run", "Edit hooks.ts"]);
    spell(
        &mut b,
        0,
        100_000,
        6,
        &["Edit tests.rs", "Bash: cargo test"],
    );

    // A permission park: agent 6 hits a gate mid-loop, resolved ~12s later by
    // the gated tool's completion (the reducer's gated_before_waiting path).
    b.push(Beat {
        at_ms: 58_000,
        transport: Transport::Hook,
        event: AgentEvent::ActivityStart {
            agent_id: cast_id(6),
            tool_use_id: Some("hero-6-gated".to_string()),
            detail: Some(ToolDetail::Generic {
                display: "Bash: rm -rf dist".to_string(),
            }),
        },
    });
    b.push(Beat {
        at_ms: 58_400,
        transport: Transport::Hook,
        event: AgentEvent::Waiting {
            agent_id: cast_id(6),
            reason: "permission".to_string(),
        },
    });
    b.push(Beat {
        at_ms: 70_500,
        transport: Transport::Hook,
        event: AgentEvent::ActivityEnd {
            agent_id: cast_id(6),
            tool_use_id: Some("hero-6-gated".to_string()),
        },
    });

    // Door traffic: agent 5 wraps up and leaves; a late hire (7) walks in.
    b.push(Beat {
        at_ms: 104_000,
        transport: Transport::Hook,
        event: AgentEvent::SessionEnd {
            agent_id: cast_id(5),
            as_child: false,
        },
    });
    b.push(Beat {
        at_ms: 108_000,
        transport: Transport::Jsonl,
        event: session_start(7),
    });
    spell(&mut b, 7, 110_000, 6, &["Read main.rs", "Bash: just test"]);
    // ...and 7 leaves near the wrap so the loop restart re-seats a stable cast
    // (5 re-enters on the next loop's walk-in replay; 7's start replays too but
    // lands AFTER its end below — the pair nets out to a periodic visitor).
    b.push(Beat {
        at_ms: LOOP_MS - 2_000,
        transport: Transport::Hook,
        event: AgentEvent::SessionEnd {
            agent_id: cast_id(7),
            as_child: false,
        },
    });

    b.sort_by_key(|beat| beat.at_ms);
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixtuoid_core::state::reducer::Reducer;
    use pixtuoid_core::state::SceneState;
    use std::time::{Duration, SystemTime};

    fn run_script_through_reducer(loops: u32) -> SceneState {
        let mut scene = SceneState::uniform(16);
        let mut reducer = Reducer::new();
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_millis(1_000_000);
        let script = hero_script();
        for l in 0..loops {
            for beat in &script {
                let now = t0 + Duration::from_millis(u64::from(l) * LOOP_MS + beat.at_ms);
                reducer.apply(&mut scene, beat.event.clone(), now, beat.transport);
                reducer.tick(&mut scene, now);
            }
        }
        scene
    }

    #[test]
    fn script_is_sorted_and_fits_one_loop() {
        let s = hero_script();
        assert!(s.windows(2).all(|w| w[0].at_ms <= w[1].at_ms));
        assert!(s.last().unwrap().at_ms < LOOP_MS);
    }

    #[test]
    fn burst_gap_stays_under_the_reducer_debounce() {
        // The cross-crate pairing this script's whole "continuously Active"
        // illusion rests on: the idle gap between chained bursts must sit
        // inside the reducer's Active→Idle debounce, or every spell flickers.
        assert!(
            std::time::Duration::from_millis(BURST_SPACING_MS - BURST_MS)
                < pixtuoid_core::state::reducer::ACTIVE_GRACE_WINDOW,
            "burst gap ({}ms) must stay under ACTIVE_GRACE_WINDOW ({:?})",
            BURST_SPACING_MS - BURST_MS,
            pixtuoid_core::state::reducer::ACTIVE_GRACE_WINDOW
        );
    }

    #[test]
    fn one_loop_populates_a_working_office() {
        let scene = run_script_through_reducer(1);
        // 7 walk-ins + the late hire − the two walkouts still present as slots
        // (exiting slots GC ~4.5s after their end; the loop's last end is 2s
        // before wrap, so at wrap the cast is 6 seated + up to 2 exiting).
        assert!(
            scene.agents.len() >= 6,
            "expected a populated office, got {}",
            scene.agents.len()
        );
        // Desk assignment happened through the real allocator.
        let desks: std::collections::HashSet<_> =
            scene.agents.values().map(|a| a.desk_index.0).collect();
        assert_eq!(
            desks.len(),
            scene.agents.len(),
            "each agent has its own desk"
        );
        // Every cast source resolved a REGISTERED label prefix — a hand-typed
        // source string that misses the registry falls back to the raw string
        // (e.g. `claude_code·api`), which no real app session ever shows.
        for a in scene.agents.values() {
            let prefix = a.label.split('·').next().unwrap();
            assert!(
                ["cc", "cx", "oc", "cu"].contains(&prefix),
                "label {:?} must carry a registered source prefix",
                a.label
            );
        }
    }

    #[test]
    fn looping_stays_stable_across_wraps() {
        // 3 loops: replayed SessionStarts must not duplicate agents or leak
        // desks; the office converges to the steady cast.
        let scene = run_script_through_reducer(3);
        assert!(
            (6..=8).contains(&scene.agents.len()),
            "cast must stay bounded across loops, got {}",
            scene.agents.len()
        );
    }
}
