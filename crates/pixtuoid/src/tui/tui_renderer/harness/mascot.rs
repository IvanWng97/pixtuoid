use super::*;

// ===================================================================
// Gateway lobster mascot — presence-gated wandering creature
// ===================================================================

/// A scene carrying an OpenClaw gateway presence (and nothing else, so the only
/// lobster-red pixels on the floor are the lobster's).
fn gateway_scene(
    liveness: pixtuoid_core::state::DaemonLiveness,
    entered_at: SystemTime,
    last_seen: SystemTime,
    sessions: u32,
) -> SceneState {
    gateway_scene_runs(liveness, entered_at, last_seen, sessions, &[])
}

/// As `gateway_scene`, with in-flight RUN keys (the busy bubble tell keys on
/// runs, not sessions). Busy is DERIVED: pass `DaemonLiveness::UP` + ≥1 run
/// (#460), not a stored Busy state.
fn gateway_scene_runs(
    liveness: pixtuoid_core::state::DaemonLiveness,
    entered_at: SystemTime,
    last_seen: SystemTime,
    sessions: u32,
    runs: &[&str],
) -> SceneState {
    let mut s = SceneState::uniform(16);
    s.daemons_mut().insert(
        pixtuoid_core::source::openclaw::SOURCE_NAME.to_string(),
        pixtuoid_core::state::DaemonPresence {
            liveness,
            active_sessions: sessions,
            last_seen,
            entered_at,
            in_flight_run_keys: runs.iter().map(|s| s.to_string()).collect(),
            current_pid: Some(1),
        },
    );
    s
}

/// Count the busy activity-bubble pixels (the `0xd6,0xf2,0xf8` rising stream).
fn bubble_px(buf: &RgbBuffer) -> usize {
    let bubble = pixtuoid_core::sprite::Rgb {
        r: 0xd6,
        g: 0xf2,
        b: 0xf8,
    };
    let mut n = 0;
    for y in 0..buf.height() {
        for x in 0..buf.width() {
            if buf.get(x, y) == bubble {
                n += 1;
            }
        }
    }
    n
}

/// Count the lobster's exclusive carapace reds in the buffer (the lobster sprite is
/// not recolored, so its authored RGBs render exactly). An empty agents scene
/// means no recolored shirts can collide.
fn lobster_px(buf: &RgbBuffer) -> usize {
    let reds = [
        pixtuoid_core::sprite::Rgb {
            r: 0xd2,
            g: 0x40,
            b: 0x2f,
        }, // body
        pixtuoid_core::sprite::Rgb {
            r: 0xe8,
            g: 0x55,
            b: 0x40,
        }, // claw
        pixtuoid_core::sprite::Rgb {
            r: 0xc8,
            g: 0x38,
            b: 0x28,
        }, // antenna
        pixtuoid_core::sprite::Rgb {
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

#[test]
fn no_gateway_mascot_without_presence() {
    // The ~99% who don't run a gateway see a normal office — no lobster.
    let scene = SceneState::uniform(16);
    let mut r = build(160, 80, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    assert_eq!(lobster_px(r.buf()), 0, "no presence ⇒ no lobster pixels");
}

#[test]
fn gateway_mascot_present_when_up() {
    // entered_at well in the past ⇒ steady wander (past the walk-in).
    let scene = gateway_scene(
        pixtuoid_core::state::DaemonLiveness::UP,
        t0() - Duration::from_secs(20),
        t0(),
        0,
    );
    let mut r = build(160, 80, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(
        lobster_px(r.buf()) > 10,
        "a live gateway ⇒ the lobster scuttles the floor"
    );
}

#[test]
fn gateway_mascot_busy_bubbles_track_runs_not_sessions() {
    // Busy (an in-flight RUN) renders the activity-bubble stream; a persistent
    // session with NO run must NOT bubble (the run-vs-session intensity fix).
    let entered = t0() - Duration::from_secs(20);

    // Idle with a live session (sessions=1, NO runs) ⇒ no bubbles.
    let idle = gateway_scene(pixtuoid_core::state::DaemonLiveness::UP, entered, t0(), 1);
    // Busy with two in-flight runs ⇒ bubbles (Busy derives from the runs, UP).
    let busy = gateway_scene_runs(
        pixtuoid_core::state::DaemonLiveness::UP,
        entered,
        t0(),
        1,
        &["r1", "r2"],
    );

    let mut r = build(160, 80, vec![]);
    // Bubbles animate by `now`; scan a few frames so we don't land on an
    // all-off-screen phase, and assert the idle scene NEVER bubbles.
    let mut busy_max = 0;
    let mut idle_max = 0;
    for k in 0..8u64 {
        let now = t0() + Duration::from_millis(k * 130);
        r.render(&busy, &pack(), now).unwrap();
        busy_max = busy_max.max(bubble_px(r.buf()));
        r.render(&idle, &pack(), now).unwrap();
        idle_max = idle_max.max(bubble_px(r.buf()));
    }
    assert!(busy_max > 0, "an in-flight run ⇒ activity bubbles render");
    assert_eq!(idle_max, 0, "a persistent idle session must NOT bubble");
}

#[test]
fn gateway_mascot_walks_out_then_is_gone() {
    let mut r = build(160, 80, vec![]);
    // Just after going Down ⇒ still walking out, visible.
    let leaving = gateway_scene(
        pixtuoid_core::state::DaemonLiveness::Down,
        t0() - Duration::from_secs(20),
        t0() - Duration::from_millis(400),
        0,
    );
    r.render(&leaving, &pack(), t0()).unwrap();
    assert!(
        lobster_px(r.buf()) > 0,
        "mid walk-out, the lobster is still visible"
    );

    // Well past the walk-out window ⇒ gone (back to a normal office).
    let gone = gateway_scene(
        pixtuoid_core::state::DaemonLiveness::Down,
        t0() - Duration::from_secs(30),
        t0() - Duration::from_secs(10),
        0,
    );
    r.render(&gone, &pack(), t0()).unwrap();
    assert_eq!(
        lobster_px(r.buf()),
        0,
        "after the walk-out, the lobster has left"
    );
}

#[test]
fn gateway_mascot_wanders_over_time() {
    // Same scene rendered across a wander cycle ⇒ the lobster changes position
    // (proves the motion is live, not a fixed sticker).
    let scene = gateway_scene(
        pixtuoid_core::state::DaemonLiveness::UP,
        t0() - Duration::from_secs(20),
        t0(),
        0,
    );
    let mut r = build(160, 80, vec![]);
    let mut tops = std::collections::HashSet::new();
    for k in 0..8u64 {
        let now = t0() + Duration::from_secs(k * 3);
        r.render(&scene, &pack(), now).unwrap();
        // Signature: the topmost-leftmost lobster pixel.
        let buf = r.buf();
        'scan: for y in 0..buf.height() {
            for x in 0..buf.width() {
                let reds = [
                    pixtuoid_core::sprite::Rgb {
                        r: 0xd2,
                        g: 0x40,
                        b: 0x2f,
                    },
                    pixtuoid_core::sprite::Rgb {
                        r: 0xe8,
                        g: 0x55,
                        b: 0x40,
                    },
                ];
                if reds.contains(&buf.get(x, y)) {
                    tops.insert((x, y));
                    break 'scan;
                }
            }
        }
    }
    assert!(
        tops.len() >= 2,
        "the lobster should wander to ≥2 distinct positions, saw {}",
        tops.len()
    );
}

/// Bounding box (in PIXEL coords) of the lobster's carapace reds, or `None` if
/// the lobster isn't on screen. Reuses the `lobster_px` red set.
fn lobster_red_bbox(buf: &RgbBuffer) -> Option<(u16, u16, u16, u16)> {
    let reds = [
        pixtuoid_core::sprite::Rgb {
            r: 0xd2,
            g: 0x40,
            b: 0x2f,
        },
        pixtuoid_core::sprite::Rgb {
            r: 0xe8,
            g: 0x55,
            b: 0x40,
        },
        pixtuoid_core::sprite::Rgb {
            r: 0xc8,
            g: 0x38,
            b: 0x28,
        },
        pixtuoid_core::sprite::Rgb {
            r: 0x9e,
            g: 0x2a,
            b: 0x20,
        },
    ];
    let (mut x0, mut y0, mut x1, mut y1) = (u16::MAX, u16::MAX, 0u16, 0u16);
    let mut any = false;
    for y in 0..buf.height() {
        for x in 0..buf.width() {
            if reds.contains(&buf.get(x, y)) {
                any = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    any.then_some((x0, y0, x1, y1))
}

#[test]
fn gateway_mascot_tooltip_on_hover() {
    // entered_at well in the past ⇒ steady wander (a stable lobster to aim at).
    let scene = gateway_scene(
        pixtuoid_core::state::DaemonLiveness::UP,
        t0() - Duration::from_secs(20),
        t0(),
        0,
    );
    // vec![] = no pet, so the pet hover arm is skipped and the mascot arm runs.
    let mut r = build(160, 80, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();

    // Before any hover, no gateway tooltip text is painted.
    assert!(
        !frame_text(r.frame_buffer()).contains("gateway"),
        "no hover ⇒ no mascot tooltip"
    );

    // Center of the lobster's red bbox (pixel coords) → terminal cell.
    let (x0, y0, x1, y1) = lobster_red_bbox(r.buf()).expect("lobster on screen");
    let cx = (x0 + x1) / 2;
    let cy_px = (y0 + y1) / 2;
    // The 14px-wide hitbox tolerates the approximate center; half-block ⇒ /2.
    r.set_mouse_pos(Some((cx, cy_px / 2)));
    r.render(&scene, &pack(), t0()).unwrap();

    // The mascot tooltip reads " <name> gateway · idle " — the literal "gateway"
    // is exclusive to the mascot arm (pet/coffee/furniture tooltips never say it),
    // so this distinguishes the mascot branch from the other fallthroughs.
    assert!(
        frame_text(r.frame_buffer()).contains("gateway"),
        "hovering the lobster shows the gateway mascot tooltip"
    );
}
