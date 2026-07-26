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
    s.insert_daemon(
        pixtuoid_core::source::openclaw::SOURCE_NAME,
        harness_gateway(),
        pixtuoid_core::state::DaemonPresence {
            liveness,
            active_sessions: sessions,
            last_seen,
            entered_at,
            in_flight_runs: runs
                .iter()
                // Stamped at the scene's own clock so the run lease is fresh —
                // these fixtures assert the BUSY render, not the decay.
                .map(|r| (r.to_string(), last_seen))
                .collect(),
            current_pid: Some(1),
        },
    );
    s
}

/// The single gateway instance these harness scenes stage (OpenClaw's default
/// port) — the mascot tests are about the CREATURE, not identity; the
/// two-gateway render is pinned by `two_gateways_render_two_independent_mascots`.
fn harness_gateway() -> pixtuoid_core::state::DaemonInstanceId {
    pixtuoid_core::state::DaemonInstanceId::new("18789").expect("non-empty")
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
/// The lobster's EXCLUSIVE carapace palette (body / claw / antenna / shade) — the
/// one authored red set both `lobster_px` and `lobster_cells` read.
const LOBSTER_REDS: [pixtuoid_core::sprite::Rgb; 4] = [
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

fn lobster_px(buf: &RgbBuffer) -> usize {
    lobster_cells(buf).len()
}

/// A scene carrying one OpenClaw gateway presence per `ports` entry.
fn gateway_scene_at(ports: &[&str], entered_at: SystemTime, last_seen: SystemTime) -> SceneState {
    let mut s = SceneState::uniform(16);
    for port in ports {
        s.insert_daemon(
            pixtuoid_core::source::openclaw::SOURCE_NAME,
            pixtuoid_core::state::DaemonInstanceId::new(*port).expect("non-empty"),
            pixtuoid_core::state::DaemonPresence {
                liveness: pixtuoid_core::state::DaemonLiveness::UP,
                active_sessions: 0,
                last_seen,
                entered_at,
                in_flight_runs: Default::default(),
                current_pid: Some(1),
            },
        );
    }
    s
}

/// The lobster-red CELLS (not just a count) — so two mascots' positions can be
/// compared exactly instead of inferred from a pixel total. Same authored carapace
/// palette `lobster_px` counts, read from ONE place so the two can't drift.
fn lobster_cells(buf: &RgbBuffer) -> std::collections::BTreeSet<(u16, u16)> {
    let mut out = std::collections::BTreeSet::new();
    for y in 0..buf.height() {
        for x in 0..buf.width() {
            if LOBSTER_REDS.contains(&buf.get(x, y)) {
                out.insert((x, y));
            }
        }
    }
    out
}

#[test]
fn two_gateways_render_two_independent_mascots() {
    // THE multi-gateway payoff, at the pixels: two live gateways are two lobsters,
    // and they must not sit on top of each other. Each gateway is rendered ALONE
    // first: identical presences (same entered_at/state) differing ONLY in port
    // must occupy DIFFERENT cells, which is exactly what folding the instance id
    // into the wander seed buys — a source-only seed put both on the same cell, so
    // two gateways read as one lobster.
    let (entered, seen) = (t0() - Duration::from_secs(20), t0());
    let cells_of = |ports: &[&str]| {
        let mut r = build(160, 80, vec![]);
        r.render(&gateway_scene_at(ports, entered, seen), &pack(), t0())
            .unwrap();
        lobster_cells(r.buf())
    };
    let a = cells_of(&["18789"]);
    let b = cells_of(&["19789"]);
    assert!(
        !a.is_empty() && !b.is_empty(),
        "each gateway draws a lobster"
    );
    assert_ne!(
        a, b,
        "two gateways differing only in port must wander to DIFFERENT cells"
    );
    // Together: both are on the floor — the union of what each draws alone.
    let both = cells_of(&["18789", "19789"]);
    assert!(
        both.len() > a.len() && both.len() > b.len(),
        "two mascots must cover more floor than either alone (got {} vs {}/{})",
        both.len(),
        a.len(),
        b.len()
    );

    // …and there is NO arity assumption on this path: the roster is a map, so N
    // gateways draw N lobsters. Each added instance must cover MORE floor than the
    // set without it — a per-instance seed that collapsed two onto one cell (or a
    // painter that drew only the first/last) would flatten this out. Consecutive
    // ports on purpose: that is what a real multi-gateway host runs, and their
    // folded seeds differ by 1.
    let mut prev = 0usize;
    for n in 1..=4 {
        let ports = &["18901", "18902", "18903", "18904"][..n];
        let cells = cells_of(ports);
        assert!(
            cells.len() > prev,
            "{n} gateways must cover more floor than {}: {} vs {prev}",
            n - 1,
            cells.len()
        );
        prev = cells.len();
    }
}

#[test]
fn the_port_suffix_names_a_gateway_only_when_it_has_a_sibling() {
    // The PAINTER owns this decision (`MascotFrame.instance`), and the only way to
    // observe it is the hover text — so without this the gate could pass `Some`
    // unconditionally and every single-gateway user's tooltip would silently grow a
    // port. One gateway: the port is noise (the tooltip stays as it was before
    // multi-gateway). Two: it is the whole point of hovering.
    let (entered, seen) = (t0() - Duration::from_secs(20), t0());
    let gateway_tooltips = |ports: &[&str]| -> Vec<String> {
        let scene = gateway_scene_at(ports, entered, seen);
        let mut r = build(160, 80, vec![]);
        r.render(&scene, &pack(), t0()).unwrap();
        let cells: Vec<_> = lobster_cells(r.buf()).into_iter().collect();
        assert!(!cells.is_empty(), "the gateways must paint lobsters");
        let mut out = Vec::new();
        // Sample across the red cells rather than hovering every one: each lobster
        // is dozens of cells, and the hitbox is 14px wide, so a stride still lands
        // inside BOTH mascots without paying a full render per pixel.
        for &(x, y) in cells.iter().step_by(5) {
            r.set_mouse_pos(Some((x, y / 2)));
            r.render(&scene, &pack(), t0()).unwrap();
            let text = frame_text(r.frame_buffer());
            if text.contains("gateway") {
                out.push(text);
            }
        }
        out
    };

    let lone = gateway_tooltips(&["18789"]);
    assert!(!lone.is_empty(), "the lone gateway must be hoverable");
    assert!(
        lone.iter().all(|t| !t.contains("18789")),
        "a gateway with no sibling must NOT be named by port: {lone:?}"
    );

    let pair = gateway_tooltips(&["18789", "19789"]);
    assert!(
        pair.iter().any(|t| t.contains("18789")),
        "with a sibling, each tooltip must name WHICH gateway: {pair:?}"
    );
    assert!(
        pair.iter().any(|t| t.contains("19789")),
        "…including the second one: {pair:?}"
    );

    // "Has a sibling" is per SOURCE, not roster-wide. A second daemon SOURCE draws
    // no mascot of its own (`gateway_mascot_def` resolves only openclaw today) yet
    // still occupies a roster row, so a roster-wide count would make openclaw's LONE
    // gateway start naming its port the day a second daemon registers — the exact
    // regression the halves above pin, arriving by a different route. Unreachable
    // from the registry today, which is precisely why it needs staging by hand.
    let (entered, seen) = (t0() - Duration::from_secs(20), t0());
    let mut mixed = gateway_scene_at(&["18789"], entered, seen);
    mixed.insert_daemon(
        "daemon2",
        pixtuoid_core::state::DaemonInstanceId::new("1").expect("non-empty"),
        pixtuoid_core::state::DaemonPresence {
            liveness: pixtuoid_core::state::DaemonLiveness::UP,
            active_sessions: 0,
            last_seen: seen,
            entered_at: entered,
            in_flight_runs: Default::default(),
            current_pid: Some(1),
        },
    );
    let mut r = build(160, 80, vec![]);
    r.render(&mixed, &pack(), t0()).unwrap();
    let cells: Vec<_> = lobster_cells(r.buf()).into_iter().collect();
    assert!(!cells.is_empty(), "openclaw still paints its lobster");
    for &(x, y) in cells.iter().step_by(5) {
        r.set_mouse_pos(Some((x, y / 2)));
        r.render(&mixed, &pack(), t0()).unwrap();
        let text = frame_text(r.frame_buffer());
        if text.contains("gateway") {
            assert!(
                !text.contains("18789"),
                "a foreign daemon SOURCE must not make openclaw's lone gateway \
                 name its port: {text}"
            );
        }
    }
}

#[test]
fn one_gateway_going_down_leaves_its_sibling_on_the_floor() {
    // Instance-local liveness, at the pixels: gateway A walks out while B keeps
    // wandering. With the old source-keyed roster A's `gateway_stop` WAS B's.
    let (entered, seen) = (t0() - Duration::from_secs(20), t0());
    let mut scene = gateway_scene_at(&["18789", "19789"], entered, seen);
    let a = pixtuoid_core::state::DaemonInstanceId::new("18789").expect("non-empty");
    pixtuoid_core::source::daemon::apply_presence(
        &mut scene,
        &pixtuoid_core::source::daemon::DaemonInstanceKey::new(
            pixtuoid_core::source::openclaw::SOURCE_NAME,
            a.clone(),
        ),
        pixtuoid_core::source::daemon::DaemonPresenceUpdate::GatewayDown,
        t0(),
    );
    assert_eq!(
        scene
            .daemon(pixtuoid_core::source::openclaw::SOURCE_NAME, &a)
            .map(|p| p.display_state()),
        Some(pixtuoid_core::state::DaemonState::Down)
    );
    let b = pixtuoid_core::state::DaemonInstanceId::new("19789").expect("non-empty");
    assert_eq!(
        scene
            .daemon(pixtuoid_core::source::openclaw::SOURCE_NAME, &b)
            .map(|p| p.display_state()),
        Some(pixtuoid_core::state::DaemonState::Idle),
        "the sibling gateway is untouched"
    );
    // Well past the walk-out: A is gone from the floor, B is still drawn.
    let mut r = build(160, 80, vec![]);
    r.render(&scene, &pack(), t0() + Duration::from_secs(5))
        .unwrap();
    assert!(
        lobster_px(r.buf()) > 0,
        "the surviving gateway must still paint its lobster"
    );
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
        // Signature = the first cell in (y, x) scan order, off the SAME red set.
        if let Some(top) = lobster_cells(r.buf())
            .into_iter()
            .min_by_key(|&(x, y)| (y, x))
        {
            tops.insert(top);
        }
    }
    assert!(
        tops.len() >= 2,
        "the lobster should wander to ≥2 distinct positions, saw {}",
        tops.len()
    );
}

/// Bounding box (in PIXEL coords) of the lobster's carapace reds, or `None` if
/// the lobster isn't on screen. Reads the SAME [`LOBSTER_REDS`] set as
/// `lobster_cells`/`lobster_px`, so the three can't drift.
fn lobster_red_bbox(buf: &RgbBuffer) -> Option<(u16, u16, u16, u16)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u16::MAX, u16::MAX, 0u16, 0u16);
    let mut any = false;
    for (x, y) in lobster_cells(buf) {
        any = true;
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
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
