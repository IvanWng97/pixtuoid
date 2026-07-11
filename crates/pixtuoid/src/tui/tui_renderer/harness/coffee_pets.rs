use super::*;

// ===================================================================
// Coffee state
// ===================================================================

#[test]
fn coffee_state_evicted_when_agent_leaves_scene() {
    let id = AgentId::from_transcript_path("/cof/leave.jsonl");
    let scene = scene_with(vec![slot(id, 0, 0, t0())], 16);
    let mut r = build(100, 40, vec![]);
    r.inject_coffee(id, t0());
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(r.coffee_contains(id));
    // Agent gone from the scene ⇒ next render evicts its coffee state.
    let empty = SceneState::uniform(16);
    r.render(&empty, &pack(), t0() + Duration::from_millis(33))
        .unwrap();
    assert!(
        !r.coffee_contains(id),
        "coffee state must be evicted when the agent leaves (no leak)"
    );
}

#[test]
fn coffee_persists_through_floor_transition() {
    // Regression: the old render_transition_floor discarded its
    // render_to_rgb_buffer result (`let _ =`), so a coffee carrier first
    // DETECTED during a floor slide was never persisted → the cup never
    // landed. The transition path now records carriers inside the shared
    // `floor::render_floor` seam (the normal path persists via
    // DrawCtx.new_coffee_carriers).
    let p = pack();
    let step = Duration::from_millis(500);
    let cap = 16;
    // Several floor-0 wanderers (the pantry is 1 of ~10 waypoints, so one
    // agent reaches it far sooner than any single one would) + a floor-1
    // occupant so navigate_floor(1) has a destination.
    let n_f0 = 10usize;
    let mut agents: Vec<_> = (0..n_f0)
        .map(|i| {
            idle(
                &format!("/cof/f0_{i}.jsonl"),
                i,
                t0() - Duration::from_secs(120),
            )
        })
        .collect();
    agents.push(slot(
        AgentId::from_transcript_path("/cof/f1.jsonl"),
        1,
        cap,
        t0(),
    ));
    let scene = scene_with(agents, cap);
    let f0_ids: Vec<AgentId> = (0..n_f0)
        .map(|i| AgentId::from_transcript_path(&format!("/cof/f0_{i}.jsonl")))
        .collect();

    // Pass 1 (scratch): find the first frame where the NORMAL render path
    // detects ANY floor-0 wanderer walking back from the pantry, and which one.
    let mut scratch = build(100, 40, vec![]);
    let mut now = t0();
    scratch.render(&scene, &p, now).unwrap();
    let mut hit = None;
    'outer: for _ in 0..400 {
        now += step;
        scratch.render(&scene, &p, now).unwrap();
        for &id in &f0_ids {
            if scratch.coffee_contains(id) {
                hit = Some((id, now));
                break 'outer;
            }
        }
    }
    let (agent, detect_at) = hit.expect("a floor-0 wanderer should fetch coffee while wandering");

    // Pass 2 (real): advance to one step BEFORE detection (no coffee yet),
    // begin a transition, then render AT detect_at — so the carrier is first
    // detected DURING the slide (gap ≤ step < the 900ms transition window, and
    // < the wander stale-resume trigger, so the timeline matches the scratch).
    let mut r = build(100, 40, vec![]);
    let mut t = t0();
    r.render(&scene, &p, t).unwrap();
    while t + step < detect_at {
        t += step;
        r.render(&scene, &p, t).unwrap();
    }
    assert!(
        !r.coffee_contains(agent),
        "agent must not yet hold coffee before the transition"
    );
    r.navigate_floor(1, t);
    assert!(r.transition().is_some(), "navigation begins a transition");
    r.render(&scene, &p, detect_at).unwrap();
    assert!(
        r.coffee_contains(agent),
        "a coffee run completing mid-transition must persist (regression: \
         render_transition_floor dropped new_coffee_carriers)"
    );
}

#[test]
fn injected_coffee_changes_desk_render() {
    // Compare two renders that differ ONLY by coffee state (same scene,
    // same final timestamp) so the diff is attributable to the coffee cup +
    // steam, not elapsed-time animation.
    let id = AgentId::from_transcript_path("/cof/steam.jsonl");
    let scene = scene_with(
        vec![idle("/cof/steam.jsonl", 0, t0() - Duration::from_secs(30))],
        16,
    );
    let t1 = t0() + Duration::from_millis(33);

    let mut base = build(100, 40, vec![]);
    base.render(&scene, &pack(), t0()).unwrap();
    base.render(&scene, &pack(), t1).unwrap();
    let baseline = base.buf().clone();
    let desk = base.cached_layout().expect("layout").home_desks[0];

    let mut r = build(100, 40, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    r.inject_coffee(id, t0()); // fresh fetch ⇒ within steam window
    r.render(&scene, &pack(), t1).unwrap();

    let d = region_diff(
        &baseline,
        r.buf(),
        desk.x.saturating_sub(2),
        desk.y.saturating_sub(6),
        18,
        14,
    );
    assert!(
        d > 0,
        "coffee state should alter the desk render (cup + steam)"
    );
}

// ===================================================================
// Pets
// ===================================================================

#[test]
fn no_pet_when_pets_disabled() {
    let scene = scene_with(vec![active("/pet/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(100, 40, vec![]); // no pets
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(r.cached_pet_pos().is_none(), "no pet when none enabled");
}

#[test]
fn pet_present_when_enabled() {
    let scene = scene_with(vec![active("/pet/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(100, 40, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    assert!(r.cached_pet_pos().is_some(), "a cat should be placed");
}

#[test]
fn pet_position_varies_over_its_cycle() {
    let scene = scene_with(vec![active("/pet/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(100, 40, vec![PetKind::Cat]);
    let mut seen = std::collections::HashSet::new();
    for i in 0..5 {
        let now = t0() + Duration::from_secs(i * 10);
        r.render(&scene, &pack(), now).unwrap();
        if let Some(PetFrame { pos, anim, .. }) = r.cached_pet_pos() {
            seen.insert((pos.x, pos.y, anim));
        }
    }
    assert!(
        seen.len() >= 2,
        "pet should move/animate across its 40s cycle, saw {} distinct states",
        seen.len()
    );
}

#[test]
fn petting_freezes_pet_position() {
    let scene = scene_with(vec![active("/pet/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(100, 40, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    let PetFrame { pos, kind, .. } = r.cached_pet_pos().expect("pet placed");
    r.set_active_pet(Some(PetState {
        petted_at: t0(),
        pet_pos: pos,
        kind,
        floor_idx: 0,
    }));
    r.render(&scene, &pack(), t0() + Duration::from_millis(500))
        .unwrap();
    let PetFrame { pos: pos2, .. } = r.cached_pet_pos().expect("pet still placed");
    assert_eq!(pos, pos2, "a petted pet holds its position");
}

#[test]
fn pet_walk_is_frame_stable() {
    // Same `now` rendered by two independent renderers must yield the same pet
    // position — proves A* on (static mask + empty overlay) is deterministic
    // (no per-frame flash).
    let scene = scene_with(vec![active("/pstab/0.jsonl", 0, "Edit", t0())], 16);
    let now = t0() + Duration::from_millis(5_000); // mid walk-phase of cycle 0
    let mut r1 = build(160, 80, vec![PetKind::Cat]);
    let mut r2 = build(160, 80, vec![PetKind::Cat]);
    r1.render(&scene, &pack(), now).unwrap();
    r2.render(&scene, &pack(), now).unwrap();
    assert_eq!(
        r1.cached_pet_pos().map(|f| (f.pos.x, f.pos.y)),
        r2.cached_pet_pos().map(|f| (f.pos.x, f.pos.y)),
        "identical `now` must give identical pet position (no flash)"
    );
}

#[test]
fn pet_walk_never_clips_through_furniture() {
    // Across 4 cycles (many prev/dest pairs) × the whole 35% walk phase, every
    // walking frame must land on a walkable cell — i.e. routed around furniture.
    let scene = scene_with(vec![active("/pwalk/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(160, 80, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout after prime").clone();
    for cycle in 0u64..4 {
        for step in 0..35u64 {
            let now = t0() + Duration::from_millis(cycle * 40_000 + step * 400);
            r.render(&scene, &pack(), now).unwrap();
            if let Some(PetFrame { pos, anim, .. }) = r.cached_pet_pos() {
                if anim == PetKind::Cat.walk_anim() {
                    // Coarse-cell walkable = the predicate A* itself guarantees
                    // (same grid every agent sprite rides). Per-pixel is_walkable
                    // is stricter than the router delivers (pad band / diagonal
                    // corner-graze) and would hold the pet to a higher bar than
                    // the agents.
                    assert!(
                        pixtuoid_scene::pathfind::point_in_walkable_cell(&layout.walkable, pos),
                        "walking pet at ({},{}) is in a blocked routing cell (cycle={cycle} step={step})",
                        pos.x,
                        pos.y
                    );
                }
            }
        }
    }
}

#[test]
fn pet_rest_pos_is_walkable() {
    let scene = scene_with(vec![active("/prest/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(160, 80, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout after prime").clone();
    for cycle in 0u64..4 {
        for step in 0..10u64 {
            let now = t0() + Duration::from_millis(cycle * 40_000 + 14_200 + step * 2_600);
            r.render(&scene, &pack(), now).unwrap();
            if let Some(PetFrame { pos, anim, .. }) = r.cached_pet_pos() {
                if anim != PetKind::Cat.walk_anim() {
                    // Rest pose is a snapped cell center, so it should satisfy the
                    // stronger per-pixel check — assert that directly.
                    assert!(
                        layout.walkable.is_walkable(pos.x, pos.y),
                        "resting pet at ({},{}) is on a blocked cell (cycle={cycle} step={step})",
                        pos.x,
                        pos.y
                    );
                }
            }
        }
    }
}

#[test]
fn pet_leg_boundary_no_pop() {
    // The snapped rest anchor == the next leg's snapped walk-start anchor, so
    // the pet must not teleport across the 40s leg boundary.
    let scene = scene_with(vec![active("/pbnd/0.jsonl", 0, "Edit", t0())], 16);
    let mut r = build(160, 80, vec![PetKind::Cat]);
    r.render(&scene, &pack(), t0() + Duration::from_millis(39_600))
        .unwrap();
    let before = r.cached_pet_pos().map(|f| (f.pos.x, f.pos.y));
    r.render(&scene, &pack(), t0() + Duration::from_millis(40_040))
        .unwrap();
    let after = r.cached_pet_pos().map(|f| (f.pos.x, f.pos.y));
    if let (Some((x0, y0)), Some((x1, y1))) = (before, after) {
        let gap = (x0 as i32 - x1 as i32).unsigned_abs() + (y0 as i32 - y1 as i32).unsigned_abs();
        assert!(
            gap <= 16,
            "pet leg boundary teleports (gap={gap}px, ({x0},{y0})→({x1},{y1}))"
        );
    }
}

// ===================================================================
// Pet tooltip: cooldown (purr/woof per kind) + sleeping arms (CG5)
// ===================================================================

#[test]
fn pet_tooltip_shows_cooldown_reaction_for_cat_and_dog() {
    for (kind, word) in [(PetKind::Cat, "purr"), (PetKind::Dog, "woof")] {
        let scene = scene_with(vec![active("/ck/0.jsonl", 0, "Edit", t0())], 16);
        let mut r = build(140, 48, vec![kind]);
        r.render(&scene, &pack(), t0()).unwrap();
        let PetFrame { pos, .. } = r.cached_pet_pos().expect("pet placed");
        // Activate the petting cooldown so the tooltip shows purr/woof.
        r.set_active_pet(Some(PetState {
            petted_at: t0(),
            pet_pos: pos,
            kind,
            floor_idx: 0,
        }));
        r.set_mouse_pos(Some((pos.x, pos.y / 2)));
        r.render(&scene, &pack(), t0() + Duration::from_millis(200))
            .unwrap();
        let text = frame_text(r.frame_buffer());
        assert!(
            text.contains(word),
            "{kind:?} on cooldown should show '{word}'; got:\n{text}"
        );
    }
}

#[test]
fn pet_tooltip_shows_sleeping_when_all_idle() {
    // With every agent idle the cat sleeps (sleeps_near_idle); hovering it shows
    // the sleeping line. Use a long-idle scene so the pet settles to sleep.
    let scene = scene_with(
        vec![idle("/slp/0.jsonl", 0, t0() - Duration::from_secs(300))],
        16,
    );
    let mut r = build(160, 64, vec![PetKind::Cat]);
    // Scan the pet cycle for a sleeping frame, then hover it.
    let mut hit = None;
    for i in 0..40u64 {
        let now = t0() + Duration::from_secs(i);
        r.render(&scene, &pack(), now).unwrap();
        if let Some(PetFrame { pos, anim, .. }) = r.cached_pet_pos() {
            if anim == PetKind::Cat.sleep_anim() {
                hit = Some((pos, now));
                break;
            }
        }
    }
    let (pos, now) = hit.expect("a long-idle cat must enter its sleep anim within the window");
    r.set_mouse_pos(Some((pos.x, pos.y / 2)));
    r.render(&scene, &pack(), now).unwrap();
    let text = frame_text(r.frame_buffer());
    assert!(
        text.contains("sleeping"),
        "hovering a sleeping cat shows the sleeping line; got:\n{text}"
    );
}

// Hover a furniture item near the TOP edge so paint_simple_tooltip flips the
// box BELOW the cursor (the my < scene_rect.y + tip_h branch).
#[test]
fn furniture_tooltip_flips_below_near_top_edge() {
    let scene = scene_with(vec![idle("/flip/0.jsonl", 0, t0())], 16);
    let mut r = build(140, 48, vec![]);
    r.render(&scene, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout");
    // Find a furniture hit at the smallest cell-y (closest to the top edge).
    let mut top_hit = None;
    'scan: for my in 0..6u16 {
        for mx in 0..140u16 {
            if crate::tui::hit_test::hit_test_furniture(layout, mx, my).is_some() {
                top_hit = Some((mx, my));
                break 'scan;
            }
        }
    }
    let (mx, my) = top_hit.expect("some furniture must hover-test near the top edge");
    r.set_mouse_pos(Some((mx, my)));
    r.render(&scene, &pack(), t0())
        .expect("top-edge furniture hover must flip the tooltip below without panic");
}

// Hover an AGENT near the BOTTOM edge so paint_hover_tooltip flips the panel UP
// (the ty overflow branch). Pin it to force the centered/hover panel path.
#[test]
fn agent_tooltip_flips_up_near_bottom_edge() {
    // Seat the agent at the BOTTOM-most home desk so its hover hit-cells sit
    // near the scene's bottom edge — the dossier then can't fit below the
    // cursor and must flip up. (The pinned-anchor variant died with
    // click-to-pin; hover anchors at the real hit cell now.)
    let probe = SceneState::uniform(16);
    let mut r = build(120, 44, vec![]);
    r.render(&probe, &pack(), t0()).unwrap();
    let layout = r.cached_layout().expect("layout").clone();
    let bottom_idx = layout
        .home_desks
        .iter()
        .enumerate()
        .max_by_key(|(_, d)| d.y)
        .map(|(i, _)| i)
        .expect("a home desk");
    let scene = scene_with(
        vec![idle(
            "/flup/0.jsonl",
            bottom_idx,
            t0() - Duration::from_secs(120),
        )],
        16,
    );
    r.render(&scene, &pack(), t0()).unwrap();
    let id = AgentId::from_transcript_path("/flup/0.jsonl");
    super::hover_agent(&mut r, &scene, id, 120, 44);
    r.render(&scene, &pack(), t0())
        .expect("bottom-edge hover must not panic");
    // Reaching here (no panic, tooltip flipped within bounds) is the assertion.
}
