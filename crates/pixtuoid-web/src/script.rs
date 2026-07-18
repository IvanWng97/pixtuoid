//! The hero's scripted cast — a deterministic, LOOPED event timeline fed
//! through the REAL `Reducer`, so the web office behaves exactly like the app
//! (walk-ins, desk assignment, Active debounce, waiting bubbles, walkouts)
//! and can never drift from real behavior. No tokio, no sources — just the
//! same `(Transport, AgentEvent)` stream a live CLI would produce.
//!
//! Beat structure (one `LOOP_MS` cycle):
//! - staggered `SessionStart`s walk the cast in over the first ~2.5s (morning rush);
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
use pixtuoid_core::source::{
    antigravity, claude_code, codewhale, codex, copilot, cursor, grok, hermes, omp, opencode,
    reasonix,
};
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
/// Every slot carries a DISTINCT CLI — ALL 11 of the registry's non-daemon
/// sources (#655; OpenClaw is the 12th, rendered separately as the lobster
/// mascot via `lobster_beats`, never a cast member): the hero's CLI-name
/// chips and the badged sprites below are meant to ECHO each other ("we
/// support these agents"), so the cast spans the full roster instead of
/// repeating one CLI across most of the slots. Slots 0-6 are the morning
/// rush, 7 the late visitor, 8-9 join the rush's tail, 10 a mid-loop joiner.
const CAST: &[(&str, &str, &str)] = &[
    // (source, session key, cwd)
    (claude_code::SOURCE_NAME, "hero-cc-api", "/work/api"),
    (codex::SOURCE_NAME, "hero-cx-web", "/work/webapp"),
    (antigravity::SOURCE_NAME, "hero-ag-infra", "/work/infra"),
    (codewhale::SOURCE_NAME, "hero-cw-data", "/work/data"),
    (opencode::SOURCE_NAME, "hero-oc-cli", "/work/cli"),
    (copilot::SOURCE_NAME, "hero-cp-docs", "/work/docs"),
    (cursor::SOURCE_NAME, "hero-cu-etl", "/work/etl"),
    (hermes::SOURCE_NAME, "hero-hm-tests", "/work/tests"),
    (grok::SOURCE_NAME, "hero-gk-ml", "/work/ml"),
    (reasonix::SOURCE_NAME, "hero-rx-research", "/work/research"),
    (omp::SOURCE_NAME, "hero-om-embedded", "/work/embedded"),
];

/// The cast size — the one authority tests derive "is this id a cast member
/// or a visitor hire" from (a hardcoded slot count silently rots when the
/// cast grows, as the 8→11 extension proved). Test-only: production code
/// iterates beats, never the cast roster.
#[cfg(test)]
pub(crate) const CAST_LEN: usize = CAST.len();

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

/// A token-usage reading (#632): the desk paper tower's wire. HONESTY RULE —
/// only cast members whose REAL CLI carries a per-turn usage wire (cc, cx)
/// get these; the hero must never show a tower the product can't produce for
/// that CLI. Jsonl transport, matching production (usage is JSONL-only).
/// Because the scene PERSISTS across loop replays, each reading re-applies
/// per loop — the towers deliberately GROW the longer a visitor watches
/// (cc crosses T2 on loop 2 and tops out T3 after ~8 loops ≈ 16 min; every
/// reading clears the 25K sheet minimum, so a sheet drops onto the pile as
/// each spell wraps). Pinned by `usage_beats_grow_only_wire_bearing_towers`.
fn usage(i: usize, at_ms: u64, fresh_tokens: u64) -> Beat {
    Beat {
        at_ms,
        transport: Transport::Jsonl,
        event: AgentEvent::Usage {
            agent_id: cast_id(i),
            fresh_tokens,
        },
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

    // Walk-ins — the MORNING RUSH (spec §1, audit top12 #3): the cast is
    // through the door within ~2.5s of reveal, so scroll-0 never reads as an
    // empty looping video. Loop-wrap replay is unchanged: a SessionStart for a
    // live slot is a no-op.
    for (i, delay) in [0u64, 350, 750, 1_150, 1_600, 2_050, 2_500]
        .iter()
        .enumerate()
    {
        b.push(Beat {
            at_ms: *delay,
            transport: Transport::Jsonl,
            event: session_start(i),
        });
    }
    // The rush's tail (#655): gk and rx trail the first seven in, keeping the
    // door busy through ~3.5s without delaying the ≥4-monitors-by-3s pin.
    b.push(Beat {
        at_ms: 2_900,
        transport: Transport::Jsonl,
        event: session_start(8),
    });
    b.push(Beat {
        at_ms: 3_300,
        transport: Transport::Jsonl,
        event: session_start(9),
    });
    // om joins mid-loop — one more arrival beat spread into the loop's quiet
    // middle (its replayed start is a no-op once seated, like the rush's).
    b.push(Beat {
        at_ms: 30_000,
        transport: Transport::Jsonl,
        event: session_start(10),
    });

    // Opening spells: each agent starts working shortly after walking in —
    // ≥4 monitors on by ~3s (pinned by morning_rush_populates_within_three_
    // seconds). Offsets still interleave so wander/coffee/meetings emerge.
    spell(
        &mut b,
        0,
        1_000,
        8,
        &["Bash: cargo test", "Edit main.rs", "Read lib.rs"],
    );
    spell(&mut b, 1, 1_600, 6, &["Edit App.tsx", "Bash: pnpm build"]);
    spell(
        &mut b,
        2,
        2_200,
        10,
        &["Bash: terraform plan", "Read modules.tf"],
    );
    spell(&mut b, 3, 2_800, 6, &["Bash: dbt run", "Edit models.sql"]);
    spell(&mut b, 4, 3_400, 8, &["Edit cmd.rs", "Bash: cargo clippy"]);
    spell(&mut b, 5, 4_000, 6, &["Read index.ts", "Edit routes.ts"]);
    spell(&mut b, 8, 4_600, 8, &["Bash: pytest -q", "Edit agent.py"]);
    spell(&mut b, 9, 5_200, 6, &["Read planner.md", "Edit reason.rs"]);
    // Fill spells: the openers now END early (~11s), so re-cover the 15–40s
    // stretch the old 6–30s starts used to occupy.
    spell(
        &mut b,
        1,
        16_000,
        6,
        &["Bash: pnpm test", "Edit styles.css"],
    );
    spell(&mut b, 3, 24_000, 6, &["Read schema.sql", "Edit etl.py"]);
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
    spell(&mut b, 8, 38_000, 8, &["Edit train.py", "Bash: pytest -q"]);
    spell(&mut b, 9, 48_000, 6, &["Edit reason.rs", "Read notes.md"]);
    spell(
        &mut b,
        10,
        32_000,
        8,
        &["Bash: make flash", "Edit firmware.c"],
    );
    spell(&mut b, 10, 76_000, 6, &["Read boot.c", "Edit firmware.c"]);
    spell(&mut b, 8, 86_000, 6, &["Bash: pytest -q", "Edit eval.py"]);
    spell(&mut b, 9, 96_000, 6, &["Read paper.md", "Edit reason.rs"]);
    spell(
        &mut b,
        0,
        100_000,
        6,
        &["Edit tests.rs", "Bash: cargo test"],
    );

    // Token-meter readings (#632) — cc (0) and cx (1) only, the two cast
    // CLIs whose real wires carry per-turn usage. cc opens with a big
    // restore-sized reading so its first ream + falling sheet land within
    // the first second of the reveal; the rest settle at each spell's wrap.
    // Per-loop totals: cc +1.9M (T1 instantly, T2 on loop 2, T3 ~loop 8),
    // cx +0.4M (T1 late in loop 1, T2 ~loop 5).
    b.push(usage(0, 500, 1_200_000));
    b.push(usage(0, 11_000, 180_000));
    b.push(usage(0, 54_000, 320_000));
    b.push(usage(0, 108_000, 200_000));
    b.push(usage(1, 12_000, 150_000));
    b.push(usage(1, 25_000, 90_000));
    b.push(usage(1, 70_000, 160_000));

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
        // 9 rush walk-ins + the mid-loop joiner + the late hire − the two
        // walkouts still present as slots (exiting slots GC ~4.5s after their
        // end; the loop's last end is 2s before wrap, so at wrap the cast is
        // 9 seated + up to 2 exiting).
        assert!(
            scene.agents.len() >= 9,
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
                ["cc", "cx", "ag", "cw", "oc", "cp", "cu", "hm", "gk", "rx", "om"]
                    .contains(&prefix),
                "label {:?} must carry a registered source prefix",
                a.label
            );
        }
    }

    #[test]
    fn usage_beats_grow_only_wire_bearing_towers() {
        // #632 honesty rule: towers appear ONLY on cast members whose real
        // CLI carries a per-turn usage wire (cc, cx) — a tower on any other
        // cast desk would demo something the product never does for that CLI.
        let scene = run_script_through_reducer(1);
        let tokens_of = |i: usize| scene.agents.get(&cast_id(i)).map_or(0, |a| a.tokens_used);
        // cc opens with the restore-sized reading → T1 from the first second,
        // finishing loop 1 just under T2; cx crosses T1 late in loop 1.
        assert!(
            pixtuoid_scene::token_meter::token_tier(tokens_of(0)) >= 1,
            "cc must carry a tower after one loop, got {} tokens",
            tokens_of(0)
        );
        assert!(
            pixtuoid_scene::token_meter::token_tier(tokens_of(1)) >= 1,
            "cx must carry a tower after one loop, got {} tokens",
            tokens_of(1)
        );
        for (i, (source, _, _)) in CAST.iter().enumerate().skip(2) {
            assert_eq!(
                tokens_of(i),
                0,
                "cast {i} ({source}) has no per-turn usage wire — its desk must stay bare"
            );
        }
        // The persistent scene makes towers GROW across loop replays — the
        // leave-the-tab-open reward: cc reaches T2 by loop 2.
        let scene3 = run_script_through_reducer(3);
        let cc3 = scene3.agents.get(&cast_id(0)).map_or(0, |a| a.tokens_used);
        assert!(
            pixtuoid_scene::token_meter::token_tier(cc3) >= 2,
            "cc must have grown to T2 by loop 3, got {cc3}"
        );
    }

    #[test]
    fn looping_stays_stable_across_wraps() {
        // 3 loops: replayed SessionStarts must not duplicate agents or leak
        // desks; the office converges to the steady cast (9 seated + up to 2
        // exiting-slot stragglers at the wrap boundary).
        let scene = run_script_through_reducer(3);
        assert!(
            (9..=11).contains(&scene.agents.len()),
            "cast must stay bounded across loops, got {}",
            scene.agents.len()
        );
    }

    #[test]
    fn morning_rush_populates_within_three_seconds() {
        // Spec §1 (audit top12 #3): within ~3s of reveal the office must read
        // as a working morning — most of the cast through the door and ≥4
        // monitors on (the morning rush spec).
        let mut scene = SceneState::uniform(16);
        let mut reducer = Reducer::new();
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_millis(1_000_000);
        const RUSH_MS: u64 = 3_000;
        for beat in hero_script().iter().filter(|b| b.at_ms <= RUSH_MS) {
            let now = t0 + Duration::from_millis(beat.at_ms);
            reducer.apply(&mut scene, beat.event.clone(), now, beat.transport);
        }
        reducer.tick(&mut scene, t0 + Duration::from_millis(RUSH_MS));
        assert!(
            scene.agents.len() >= 6,
            "morning rush: expected >=6 walk-ins by 3s, got {}",
            scene.agents.len()
        );
        let active = scene
            .agents
            .values()
            .filter(|a| matches!(a.state, pixtuoid_core::state::ActivityState::Active { .. }))
            .count();
        // WHY the exact-4 hairline is expected, not a regression: the current
        // schedule lands `active` at exactly the >=4 floor with zero headroom
        // — a spell-offset tweak that shifts one burst's start a beat later
        // can drop this to 3 without the morning-rush SPEC actually regressing.
        // This asserts the floor, not a margin; if it flips red, re-check the
        // rendered office before assuming a real regression.
        assert!(
            active >= 4,
            "morning rush: expected >=4 monitors on by 3s, got {active}"
        );
    }
}
