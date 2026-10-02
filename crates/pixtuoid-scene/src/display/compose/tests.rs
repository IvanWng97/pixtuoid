use super::*;
use crate::anim::Motion;
use crate::cutaway::paint::tests::{
    by_day, lowest_painted_row, painted_alone, painted_over_two_fills,
    same_fingerprint_same_pixels, stray_pixel,
};
use crate::pack::test_default_pack;
use pixtuoid_core::sprite::RgbBuffer;

/// The wall board of an empty office, which no clock moves: for frames
/// whose board a test does not read.
pub(crate) fn quiet_board() -> &'static crate::board::BoardModel {
    static BOARD: std::sync::LazyLock<crate::board::BoardModel> = std::sync::LazyLock::new(|| {
        crate::board::build_board(
            crate::board::StateCounts::default(),
            0,
            None,
            None,
            crate::anim::Motion::Full,
            std::time::UNIX_EPOCH,
        )
    });
    &BOARD
}

/// `floor` at `now`, under the [`quiet_board`].
pub(crate) fn showing(
    floor: crate::floor::FloorMeta,
    now: std::time::SystemTime,
) -> Showing<'static> {
    Showing {
        floor,
        now,
        board: quiet_board(),
    }
}

/// A piece's base row — the ordering key — through the SAME `piece_span`
/// the draw list builds with. Width does not affect the base row, so the
/// call sites stay focused on depth.
fn sort_row(pivot: crate::layout::Pivot, pos: crate::layout::Point, h: u16, below: u16) -> u16 {
    piece_span(pivot, pos, 1, h, below).depth
}

/// The desk sorts on its face's south edge and a back-turned sitter on their
/// seat's sort row; that row lands south of the face, so the "head
/// over the surface" reading needs no special case.
#[test]
fn a_seated_occupant_sorts_in_front_of_the_desk_it_sits_at() {
    let pack = test_default_pack();
    let desk = crate::layout::Point { x: 0, y: 10 };
    let art = desk_art(&pack, crate::layout::Facing::North).expect("desk art");
    let desk_z = desk_span(&pack, art, desk, RenderScale::ONE)
        .expect("desk")
        .depth;
    let seated_z = seated_back_span(&pack, desk).depth;
    assert!(
        seated_z > desk_z,
        "a seated occupant must paint over its desk (desk {desk_z}, seated {seated_z})"
    );
}

/// The depth sort reserves no face under a density variant, which draws its
/// own front, so someone on the first row south of the art stands in FRONT
/// of the desk, not behind a face that is never drawn.
#[test]
#[cfg(feature = "density-art")]
fn someone_just_south_of_a_variant_desk_sorts_in_front_of_it() {
    let pack = test_default_pack();
    let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
    let desk = crate::layout::Point { x: 0, y: 20 };
    let art = desk_art(&pack, crate::layout::Facing::South).expect("desk art");
    let desk_box = desk_span(&pack, art, desk, scale).expect("desk");
    let (_, art_h) = art_size(&pack, art).expect("desk");
    // The first row south of the ART, measured from its placement.
    let feet = crate::pack::desk_art_top(&pack, desk.y, art_h) + art_h;
    let (w, h) = base_size(&pack, "standing");
    let person = piece_span(
        crate::layout::Pivot::TopLeft,
        crate::layout::Point {
            x: desk.x,
            y: feet + 1 - h,
        },
        w,
        h,
        0,
    );
    assert_eq!(
        depth_sort(vec![(person, "person"), (desk_box, "desk")]),
        ["desk", "person"],
        "the person just south of the art must paint over the desk"
    );
}

/// A back-turned sitter's depth box, built the way `push_characters` builds
/// it, at the sort row the sim seats an occupant at (their seat's walk anchor).
fn seated_back_span(pack: &Pack, desk: crate::layout::Point) -> Span {
    use crate::layout::Facing;
    let (w, h) = base_size(pack, "seated_back");
    let chair = chair_span(pack, Facing::North, desk).map(|(s, _)| s);
    occupant_span(
        piece_span(crate::layout::Pivot::TopLeft, near_seat(desk), w, h, 0),
        crate::layout::desk_walk_anchor_facing(desk, Facing::North).y,
        chair,
    )
}

/// The other half: someone on the FAR side is occluded BY the desk, which is
/// what gives the office depth rather than a flat plan.
#[test]
fn a_character_north_of_the_desk_sorts_behind_it() {
    let pack = test_default_pack();
    let desk = crate::layout::Point { x: 0, y: 20 };
    let (_, body_h) = base_size(&pack, "standing");

    let plain = desk_art(&pack, crate::layout::Facing::South).expect("desk art");
    let desk_z = desk_span(&pack, plain, desk, RenderScale::ONE)
        .expect("desk")
        .depth;
    // Standing at the desk's north approach, feet on the row just north of
    // its anchor.
    let behind_z = sort_row(
        crate::layout::Pivot::TopLeft,
        crate::layout::Point {
            x: desk.x,
            y: desk.y - body_h,
        },
        body_h,
        0,
    );
    assert!(behind_z < desk_z, "desk {desk_z}, walker {behind_z}");
}

/// A centre-anchored prop standing in the aisle SOUTH of a desk must paint
/// in front of that desk and behind its occupant. Keyed on its own middle
/// row, a tall plant between the two would paint over the occupant while
/// standing behind them.
#[test]
fn an_aisle_prop_sorts_between_the_desk_and_its_occupant() {
    let pack = test_default_pack();
    let desk = crate::layout::Point { x: 0, y: 10 };
    let art = desk_art(&pack, crate::layout::Facing::North).expect("desk art");
    let desk_box = desk_span(&pack, art, desk, RenderScale::ONE).expect("desk");
    let seated = seated_back_span(&pack, desk);
    let (plant_w, plant_h) = base_size(&pack, "plant");
    // A plant whose BASE sits just south of the desk's front face.
    let plant_base = desk_box.depth + 1;
    let plant_centre = crate::layout::Point {
        x: plant_w / 2,
        y: plant_base + plant_h / 2 - plant_h + 1,
    };
    let plant = piece_span(
        crate::layout::Pivot::Center,
        plant_centre,
        plant_w,
        plant_h,
        0,
    );
    assert!(
        desk_box.depth < plant.depth && plant.depth < seated.depth,
        "the fixture must separate all three by depth, or a tie-break decides: \
         desk {desk_box:?}, plant {plant:?}, seated {seated:?}"
    );
    // Pushed in REVERSE, so no tie-break by push order can produce the answer.
    let drawn = crate::display::depth_sort(vec![
        (seated, "seated"),
        (plant, "plant"),
        (desk_box, "desk"),
    ]);
    assert_eq!(drawn, vec!["desk", "plant", "seated"]);
}

/// A back-turned desk's own art is taller only ABOVE the desk: it sorts on
/// the same base row as the plain desk, so swapping the art moves no depth.
#[test]
fn a_back_turned_desk_grows_upward_and_keeps_its_base_row() {
    let pack = test_default_pack();
    let desk = crate::layout::Point { x: 20, y: 30 };
    let north = desk_art(&pack, crate::layout::Facing::North).expect("desk art");
    let south = desk_art(&pack, crate::layout::Facing::South).expect("desk art");
    assert_ne!(north, south, "the bundled pack ships the raised art");
    let plain = desk_span(&pack, south, desk, RenderScale::ONE).expect("desk");
    let raised = desk_span(&pack, north, desk, RenderScale::ONE).expect("desk_north");
    assert_eq!(raised.depth, plain.depth);
    let ((_, plain_h), (_, north_h)) = (base_size(&pack, south), base_size(&pack, north));
    assert!(
        north_h > plain_h,
        "the back-turned desk's monitor stands above the base desk"
    );
    assert_eq!(plain.y0 - raised.y0, north_h - plain_h);
}

/// A pack that draws only its own front sofa gets it flipped for the back
/// view, never the default pack's back-view art in another style.
#[test]
fn a_pack_without_the_back_view_sofa_draws_its_own_front_one_flipped() {
    let mut own = pixtuoid_core::sprite::format::load_pack_from_strings(
        "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\
         [animations.meeting_sofa]\nframes=[\"one.sprite\"]\nframe_ms=100\n",
        &[("one.sprite", "@frame 0\nA")],
    )
    .expect("pack builds");
    own.merge_from(&test_default_pack());
    let mut order = Vec::new();
    push_sofa(
        &mut order,
        &own,
        crate::layout::Point { x: 10, y: 10 },
        true,
        Tie::FixtureOver,
    );
    assert!(
        matches!(
            order.as_slice(),
            [(
                _,
                PieceKind::Prop {
                    art: Art {
                        sprite: "meeting_sofa",
                        flip: Flip::Vertical,
                        ..
                    },
                    ..
                }
            )]
        ),
        "{order:?}"
    );
}

/// A pack without the facing's own art draws the piece it derives from, the
/// classic painter's rule, rather than no desk.
#[test]
fn a_pack_without_the_back_turned_art_draws_the_plain_desk() {
    let pack = pixtuoid_core::sprite::format::load_pack_from_strings(
        "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\
         [animations.desk]\nframes=[\"one.sprite\"]\nframe_ms=100\n",
        &[("one.sprite", "@frame 0\nA")],
    )
    .expect("pack builds");
    assert_eq!(desk_art(&pack, crate::layout::Facing::North), Some("desk"));
}

/// A back-turned sitter and their chair are ONE piece, so nothing can sort
/// between them: the occupied desk pushes no chair of its own, and the
/// sitter's box covers the chair's at either phase of the breathing bob.
#[test]
fn a_back_turned_sitter_carries_their_own_chair() {
    let pack = test_default_pack();
    let layout = SceneLayout::compute_with_seed(160, 96, None, 0).expect("lays out");
    let north: Vec<crate::layout::Point> = layout
        .home_desks
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            layout.desk_facing(pixtuoid_core::state::FloorLocalDeskIndex(*i))
                == crate::layout::Facing::North
        })
        .map(|(_, d)| *d)
        .collect();
    assert!(!north.is_empty(), "the office has back-turned desks");

    let chairs = queued(&layout, &pack, RenderScale::ONE, &north[..1], |k| {
        matches!(k, FixtureKind::DeskChair(_))
    });
    assert_eq!(
        chairs.len(),
        north.len() - 1,
        "the occupied desk's chair rides its sitter"
    );

    let desk = north[0];
    let (chair, _) = chair_span(&pack, crate::layout::Facing::North, desk).expect("a north chair");
    for anim in ["typing_back", "seated_back"] {
        let (w, h) = base_size(&pack, anim);
        for bob in [0, 1] {
            let seat = near_seat(desk);
            let body = piece_span(
                crate::layout::Pivot::TopLeft,
                crate::layout::Point {
                    x: seat.x,
                    y: seat.y + bob,
                },
                w,
                h,
                0,
            );
            let depth =
                crate::layout::desk_walk_anchor_facing(desk, crate::layout::Facing::North).y;
            let piece = occupant_span(body, depth, Some(chair));
            assert!(
                piece.depth >= chair.depth
                    && piece.depth >= depth
                    && piece.x0 <= chair.x0.min(body.x0)
                    && piece.x1 >= chair.x1.max(body.x1)
                    && piece.y0 <= chair.y0.min(body.y0)
                    && piece.y1 >= chair.y1.max(body.y1),
                "{anim}, bob {bob}: {piece:?} must cover {body:?} and {chair:?}"
            );
        }
    }
    assert!(
        chair_span(&pack, crate::layout::Facing::South, desk).is_none(),
        "a viewer-facing occupant sits in front of their own chair"
    );

    // A chair wider than its sitter is still inside their piece: a passer-by
    // overlapping only the chair's columns sorts against the sitter too.
    let piece = occupant_span(
        Span::new(10, 10, 4, 6, 0),
        16,
        Some(Span::new(8, 13, 8, 4, 0)),
    );
    assert_eq!((piece.x0, piece.x1), (8, 15));
}

/// A desk's shadow centres on the row just under the one it sorts on: the
/// row it meets the ground on. Pinned exactly, as the ordering tests compare
/// depths by inequality, which a one-row shift passes.
#[test]
fn a_desks_shadow_centres_on_the_row_under_where_it_sorts() {
    let pack = test_default_pack();
    let desk = crate::layout::Point { x: 20, y: 30 };
    for facing in [crate::layout::Facing::North, crate::layout::Facing::South] {
        let art = desk_art(&pack, facing).expect("desk art");
        for s in [1, pack.max_density_variant().get()] {
            let scale = RenderScale::new(s).expect("nonzero");
            let span = desk_span(&pack, art, desk, scale).expect("desk");
            let kind = PieceKind::Desk {
                at: desk,
                art,
                screen: Screen::Off,
            };
            let shadow = ground_shadow(span, &kind, &pack).expect("a desk casts a shadow");
            let ((_, top), (_, past)) = shadow.bounds();
            assert_eq!((top + past) / 2, span.depth + 1, "{art} at scale {s}");
        }
    }
}

/// A standing chair sorts on the classic painter's own chair key.
#[test]
fn a_chair_sorts_on_the_classic_chair_key() {
    let pack = test_default_pack();
    let desk = crate::layout::Point { x: 20, y: 30 };
    let (span, _) = chair_span(&pack, crate::layout::Facing::North, desk)
        .expect("a back-turned desk stands a chair");
    assert_eq!(
        span.depth,
        crate::layout::desk_chair_sort_row(desk, crate::layout::Facing::North)
    );
}

/// A walker sorts on the sim's sort row for them, every step.
#[test]
fn a_walker_sorts_on_the_sims_sort_row() {
    let (layout, pack, frames, _) = sit_down(crate::layout::Facing::South, 0);
    let mut walked = 0;
    for frame in &frames {
        let Some(c) = frame.characters.first().filter(|c| c.seat_desk.is_none()) else {
            continue;
        };
        let mut order = Vec::new();
        push_characters(
            frame,
            Office {
                layout: &layout,
                pack: &pack,
                theme: &crate::theme::NORMAL,
                scale: RenderScale::ONE,
            },
            std::time::UNIX_EPOCH,
            &mut order,
        );
        let (span, _) = order.first().expect("the walker is drawn");
        assert_eq!(span.depth, c.sort_row);
        walked += 1;
    }
    assert!(walked > 1, "the fixture never walked, so this pins nothing");
}

/// A chair that rises above its sitter's head still lies inside their
/// piece, which sorts on the later of the two depths.
#[test]
fn an_occupant_span_bounds_a_chair_taller_than_its_sitter() {
    let body = Span::new(10, 20, 8, 12, 0);
    let chair = Span::new(9, 15, 10, 20, 0).with_depth(40);
    let piece = occupant_span(body, 31, Some(chair));
    assert_eq!(
        (piece.x0, piece.x1, piece.y0, piece.y1, piece.depth),
        (9, 18, 15, 34, 40)
    );
}

/// A lit screen's scanline lights its glass on the column the model names
/// and nowhere else, at every density the desk art is drawn at: the glass's
/// columns split evenly among the classic's glass columns.
#[test]
fn a_lit_screens_scanline_is_on_the_models_column() {
    let pack = test_default_pack();
    let art = desk_art(&pack, crate::layout::Facing::North).expect("desk art");
    let glow = pixtuoid_core::sprite::Rgb {
        r: 40,
        g: 180,
        b: 220,
    };
    let line = crate::effects::look::scanline_color(glow);
    let cols = crate::layout::SCREEN_GLASS_COLS;
    let n = cols.end() - cols.start() + 1;
    for s in [1, pack.max_density_variant().get()] {
        let scale = RenderScale::new(s).expect("nonzero");
        let desk = crate::pack::densest_frame(&pack, art, 0, scale).expect("the art");
        let w = usize::from(desk.frame.width());
        let glass: Vec<u16> = drawn_in(
            &desk,
            &[crate::pack::SCREEN_GLASS_KEY, crate::pack::SCREEN_TEXT_KEY],
        )
        .iter()
        .enumerate()
        .filter(|&(_, &g)| g)
        .map(|(i, _)| (i % w) as u16)
        .collect();
        let x0 = *glass.iter().min().expect("the art draws glass");
        let band = (glass.iter().max().expect("glass") - x0 + 1) / n;
        for scan in 0..n {
            let lit = Screen::Lit { glow, scan }
                .on(desk.recolorable)
                .expect("a lit screen");
            let lined: std::collections::BTreeSet<u16> = (0..lit.height())
                .flat_map(|y| (0..lit.width()).map(move |x| (x, y)))
                .filter(|&(x, y)| lit.get(x, y).copied().flatten() == Some(line))
                .map(|(x, _)| x)
                .collect();
            assert_eq!(
                lined,
                (x0 + scan * band..x0 + (scan + 1) * band).collect(),
                "at scale {s}, scan {scan} lit other columns"
            );
        }
    }
}

/// The bundled office with one editing agent homed at its first desk facing
/// `facing`, observed through the real sim every tick of their walk there:
/// the frames up to the first where they sit, then `seated_ticks` more, and
/// that desk.
pub(crate) fn sit_down(
    facing: crate::layout::Facing,
    seated_ticks: usize,
) -> (SceneLayout, Pack, Vec<SimFrame>, crate::layout::Point) {
    sit_down_in(test_default_pack(), facing, seated_ticks)
}

/// [`sit_down`] with `pack` drawing the office.
fn sit_down_in(
    pack: Pack,
    facing: crate::layout::Facing,
    seated_ticks: usize,
) -> (SceneLayout, Pack, Vec<SimFrame>, crate::layout::Point) {
    let id = pixtuoid_core::AgentId::from_transcript_path("/cutaway/sit.jsonl");
    sit_down_as(pack, facing, seated_ticks, id)
}

/// [`sit_down_in`] with `id` doing the walking.
fn sit_down_as(
    pack: Pack,
    facing: crate::layout::Facing,
    seated_ticks: usize,
    id: pixtuoid_core::AgentId,
) -> (SceneLayout, Pack, Vec<SimFrame>, crate::layout::Point) {
    use crate::floor::{FloorMeta, FloorSession};
    use pixtuoid_core::state::{ActivityState, FloorLocalDeskIndex, ToolKind};
    use std::time::{Duration, SystemTime};
    const LOGICAL: (u16, u16) = (160, 96);
    let meta = FloorMeta::ground();
    let layout = SceneLayout::compute_with_seed(LOGICAL.0, LOGICAL.1, None, meta.floor_seed)
        .expect("lays out");
    let home = (0..layout.home_desks.len())
        .find(|&i| layout.desk_facing(FloorLocalDeskIndex(i)) == facing)
        .expect("the office has a desk facing that way");
    let now0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut scene = pixtuoid_core::SceneState::uniform(16);
    scene.agents.insert(
        id,
        pixtuoid_core::AgentSlot {
            agent_id: id,
            source: std::sync::Arc::from("claude-code"),
            session_id: std::sync::Arc::from("s"),
            cwd: std::sync::Arc::from(std::path::Path::new("/w/x")),
            label: "x".into(),
            state: ActivityState::Active {
                tool_use_id: None,
                detail: None,
                kind: ToolKind::Edit,
            },
            state_started_at: now0,
            created_at: now0,
            last_event_at: now0,
            exiting_at: None,
            pending_idle_at: None,
            desk_index: pixtuoid_core::GlobalDeskIndex(home),
            floor_idx: 0,
            tool_call_count: 0,
            active_ms: 0,
            unknown_cwd: false,
            parent_id: None,
            pid: None,
            model: None,
            effort: None,
            tokens_used: 0,
            last_usage: None,
        },
    );
    let mut session = FloorSession::new();
    let mut frames = Vec::new();
    let mut seated_at = None;
    for n in 1..=1200u64 {
        let frame = session
            .step(
                crate::floor::FloorInputs {
                    scene: &scene,
                    pack: &pack,
                    now: now0 + Duration::from_millis(100 * n),
                    floor: meta,
                    pets: crate::floor::PetInputs::default(),
                },
                crate::layout::Size {
                    w: LOGICAL.0,
                    h: LOGICAL.1,
                },
            )
            .expect("lays out")
            .frame;
        if seated_at.is_none()
            && frame
                .seated_agents
                .get(&FloorLocalDeskIndex(home))
                .copied()
                .unwrap_or(false)
        {
            seated_at = Some(frames.len());
        }
        frames.push(frame);
        if seated_at.is_some_and(|at| frames.len() > at + seated_ticks) {
            let desk = layout.home_desks[home];
            return (layout, pack, frames, desk);
        }
    }
    panic!("the agent never sat at their desk");
}

/// A back-turned sitter's badge clears the raised monitor behind their head:
/// its whole plate lands above the desk art's top.
#[test]
fn a_back_turned_sitters_badge_clears_their_raised_monitor() {
    let (layout, pack, frames, desk) = sit_down(crate::layout::Facing::North, 0);
    let seated = frames.last().expect("a seated frame");
    let office = Office {
        layout: &layout,
        pack: &pack,
        theme: crate::theme::theme_by_name("normal").expect("theme"),
        scale: RenderScale::new(4).expect("nonzero"),
    };
    let list = frame_list(
        seated,
        office,
        showing(
            crate::floor::FloorMeta::ground(),
            std::time::SystemTime::UNIX_EPOCH,
        ),
    );
    let plate = list
        .pieces()
        .iter()
        .find(|p| matches!(p.kind, PieceKind::Badge { .. }))
        .expect("the sitter has a badge")
        .span;
    let art = desk_art(&pack, crate::layout::Facing::North).expect("desk art");
    let top = desk_span(&pack, art, desk, RenderScale::ONE)
        .expect("desk")
        .y0;
    assert!(plate.y1 < top, "{plate:?} reaches the monitor top at {top}");
}

/// Who carries a chair is ONE decision: a sitter skipped for art the pack
/// lacks carries nothing, so their desk still stands its own chair.
#[test]
fn a_sitter_the_pack_cannot_draw_leaves_their_chair_standing() {
    let (layout, pack, frames, desk) = sit_down(crate::layout::Facing::North, 0);
    let seated = frames.last().expect("a seated frame");
    let mut order = Vec::new();
    assert_eq!(
        push_characters(
            seated,
            Office {
                layout: &layout,
                pack: &pack,
                theme: &crate::theme::NORMAL,
                scale: RenderScale::ONE
            },
            std::time::UNIX_EPOCH,
            &mut order
        ),
        vec![desk]
    );

    let chair_only = pixtuoid_core::sprite::format::load_pack_from_strings(
        &format!(
            "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\"A\"=\"#010203\"\n\
             [animations.{}]\nframes=[\"one.sprite\"]\nframe_ms=100\n",
            crate::pack::DESK_CHAIR_SPRITE
        ),
        &[("one.sprite", "@frame 0\nA")],
    )
    .expect("pack builds");
    let mut order = Vec::new();
    let carried = push_characters(
        seated,
        Office {
            layout: &layout,
            pack: &chair_only,
            theme: &crate::theme::NORMAL,
            scale: RenderScale::ONE,
        },
        std::time::UNIX_EPOCH,
        &mut order,
    );
    assert!(carried.is_empty() && order.is_empty(), "no character art");
    order.extend(queued(
        &layout,
        &chair_only,
        RenderScale::ONE,
        &carried,
        |k| matches!(k, FixtureKind::DeskChair(_)),
    ));
    assert!(
        order
            .iter()
            .any(|(_, k)| matches!(k, PieceKind::Chair { at } if Some(*at)
                == crate::layout::desk_chair_top_left(desk, crate::layout::Facing::North))),
        "the undrawn sitter's desk lost its chair"
    );
}

/// Whether desk `desk`'s chair draws over `frame`'s one person — riding their
/// piece once they sit, or as its own piece before — or `None` where the two
/// do not overlap, so their order shows nothing.
fn chair_over_person(
    frame: &SimFrame,
    layout: &SceneLayout,
    pack: &Pack,
    desk: crate::layout::Point,
) -> Option<bool> {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let order = collect_pieces(
        frame,
        Office {
            layout,
            pack,
            theme,
            scale: RenderScale::ONE,
        },
        &Moment::resolve(
            crate::sky::Sky::clock(std::time::UNIX_EPOCH),
            theme,
            0.0,
            Motion::Full.clock(std::time::UNIX_EPOCH),
        ),
    );
    let (person, person_span) = order
        .iter()
        .enumerate()
        .find_map(|(i, (s, k))| matches!(k, PieceKind::Character { .. }).then_some((i, *s)))?;
    if let (_, PieceKind::Character { chair: Some(_), .. }) = &order[person] {
        return Some(true);
    }
    let (at, chair_span) =
        crate::layout::desk_chair_top_left(desk, crate::layout::Facing::North)
            .zip(chair_span(pack, crate::layout::Facing::North, desk).map(|(s, _)| s))?;
    let chair = order
        .iter()
        .position(|(_, k)| matches!(k, PieceKind::Chair { at: a } if *a == at))?;
    let overlap = person_span.x0 <= chair_span.x1
        && chair_span.x0 <= person_span.x1
        && person_span.y0 <= chair_span.y1
        && chair_span.y0 <= person_span.y1;
    if !overlap {
        return None;
    }
    let drawn = crate::display::depth_sort(
        order
            .iter()
            .enumerate()
            .map(|(i, (s, _))| (*s, i))
            .collect(),
    );
    let pos = |i: usize| drawn.iter().position(|&j| j == i);
    Some(pos(chair)? > pos(person)?)
}

/// The chair draws over its occupant through the settle arc — every frame
/// the sim keys them at their seat — as in the classic painter: keyed on its
/// own box, it would sort behind them until they sat and jump in front the
/// frame they did.
#[test]
fn a_chair_keeps_its_order_to_its_sitter_through_the_settle() {
    use crate::layout::Facing;
    let (layout, pack, frames, desk) = sit_down(Facing::North, 0);
    let seat_row = crate::layout::desk_chair_sort_row(desk, Facing::North);
    let orders: Vec<(usize, bool)> = frames
        .iter()
        .enumerate()
        .filter(|(_, f)| f.characters.first().is_some_and(|c| c.sort_row == seat_row))
        .filter_map(|(n, f)| chair_over_person(f, &layout, &pack, desk).map(|o| (n, o)))
        .collect();
    assert!(
        orders.len() > 1,
        "the sitter was never keyed at their seat before sitting"
    );
    assert!(
        orders.iter().all(|&(_, over)| over),
        "the chair flipped under its sitter at frames {:?}",
        orders
            .iter()
            .filter(|(_, o)| !o)
            .map(|(n, _)| n)
            .collect::<Vec<_>>()
    );
}

/// A person sorts on the sim's own key, so a viewer-facing sitter's depth
/// holds while their breath moves their drawn box.
#[test]
fn a_sitters_depth_holds_through_their_breath() {
    let (layout, pack, frames, desk) = sit_down(crate::layout::Facing::South, 60);
    let (mut depths, mut tops) = (
        std::collections::BTreeSet::new(),
        std::collections::BTreeSet::new(),
    );
    for frame in frames.iter().filter(|f| {
        f.characters
            .first()
            .is_some_and(|c| c.seat_desk == Some(desk))
    }) {
        let mut order = Vec::new();
        push_characters(
            frame,
            Office {
                layout: &layout,
                pack: &pack,
                theme: &crate::theme::NORMAL,
                scale: RenderScale::ONE,
            },
            std::time::UNIX_EPOCH,
            &mut order,
        );
        let (span, _) = order.first().expect("the sitter is drawn");
        depths.insert(span.depth);
        tops.insert(span.y0);
    }
    assert!(
        tops.len() > 1,
        "the sitter never breathed, so this pins nothing: {tops:?}"
    );
    assert_eq!(
        depths.len(),
        1,
        "their depth moved with their breath: {depths:?}"
    );
}

/// The badge follows the CUTAWAY's body, not the classic one:
/// `overlay::build_overlay` hangs off the classic-drawn sprite, which for a
/// seated agent is not where the cutaway draws them.
#[test]
fn a_label_anchor_sits_above_the_head_and_centred_on_the_sprite() {
    let at = crate::layout::Point { x: 10, y: 20 };
    let anchor = label_anchor(at, 8, None);
    assert_eq!(anchor.x, at.x + 4, "centred on the sprite");
    assert_eq!(at.y - anchor.y, LABEL_GAP, "clear of the head");
}

/// The floor indicator's plate stays in the cell the classic writes it
/// across, its text whole, at every density the pack draws: a row lower
/// and it covers the top of the elevator door, over everything.
#[test]
#[cfg(feature = "density-art")]
fn the_floor_indicator_stays_in_its_cell() {
    use crate::cutaway::text::LINE_H;
    let pack = crate::pack::test_default_pack();
    let door = SceneLayout::compute_with_seed(160, 96, None, 0)
        .expect("lays out")
        .door;
    let rows = crate::layout::floor_indicator_rows(door.y);
    let densities = pack.density_variants();
    assert!(!densities.is_empty(), "the pack draws a density");
    for d in densities {
        let pen = Pen::new(RenderScale::new(d.get()).expect("nonzero"), d.get())
            .expect("d divides itself");
        for floor in [1, 12, 99] {
            let plate = indicator_plate(door, floor, pen);
            let span = topmost_span(plate, pen);
            assert!(
                rows.contains(&span.y0) && rows.contains(&span.y1),
                "{d:?} floor {floor}: rows {}..={} outside {rows:?}",
                span.y0,
                span.y1
            );
            assert!(plate.h.0 >= LINE_H, "{d:?}: the text fits");
        }
    }
}

/// At the pack's 4x art the board writes inside the neon sign's dark
/// interior, as the classic's terminal board does, however full its lines.
#[test]
#[cfg(feature = "density-art")]
fn the_board_writes_inside_the_signs_interior() {
    use crate::layout::{
        NEON_PANEL_INNER_H, NEON_PANEL_INNER_W, NEON_PANEL_INNER_X, NEON_PANEL_INNER_Y,
    };
    let pack = test_default_pack();
    let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
    let counts = crate::board::StateCounts {
        waiting: 12,
        active: 34,
        idle: 56,
        exiting: 0,
        total: 102,
    };
    let gateway = Some(pixtuoid_core::state::DaemonState::Degraded);
    for ms in (0..16_000).step_by(100) {
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms);
        let board = crate::board::build_board(
            counts,
            99 * 3_600,
            Some((12, 12)),
            gateway,
            crate::anim::Motion::Full,
            now,
        );
        let span = board_span(&board, Pen::for_pack(scale, &pack));
        assert!(
            span.x0 >= NEON_PANEL_INNER_X
                && span.x1 < NEON_PANEL_INNER_X + NEON_PANEL_INNER_W
                && span.y0 >= NEON_PANEL_INNER_Y
                && span.y1 < NEON_PANEL_INNER_Y + NEON_PANEL_INNER_H,
            "{span:?} at +{ms}ms"
        );
    }
}

/// A ceiling ABOVE the head lifts the badge clear of it; one below the head
/// changes nothing.
#[test]
fn a_label_anchor_clears_a_ceiling_above_the_head() {
    let at = crate::layout::Point { x: 10, y: 20 };
    let free = label_anchor(at, 8, None);
    let raised = label_anchor(at, 8, Some(at.y - 4));
    assert_eq!(
        raised.y,
        at.y - 4 - LABEL_GAP,
        "the badge clears the monitor top by the same gap it clears a head by"
    );
    assert_eq!(raised.x, free.x);
    assert_eq!(label_anchor(at, 8, Some(at.y + 4)), free);
}

/// A solid casts its shadow under its south edge; a prop's upper band does
/// not.
#[test]
fn what_meets_the_ground_casts_a_shadow() {
    let pack = test_default_pack();
    let span = Span::new(10, 10, 8, 12, 0);
    let chair = PieceKind::Chair {
        at: crate::layout::Point { x: 10, y: 10 },
    };
    assert_eq!(
        ground_shadow(span, &chair, &pack),
        Some(crate::ground::Contact::under(10, 8, span.y1 + 1)),
        "centred under it, on the ground row under its south edge"
    );
    let (_, h) = art_size(&pack, MEETING_SOFA_NORTH).expect("sofa art");
    let band = |rows| PieceKind::PropBand {
        at: crate::layout::Point { x: 10, y: 10 },
        sprite: MEETING_SOFA_NORTH,
        rows,
    };
    assert!(
        ground_shadow(span, &band((0, NORTH_SOFA_SEAT_ROWS)), &pack).is_none(),
        "the seat band"
    );
    assert!(
        ground_shadow(span, &band((NORTH_SOFA_SEAT_ROWS, h)), &pack).is_some(),
        "the foot band"
    );
}

/// A figure standing casts its own shadow; sitting, the chair they carry
/// casts one under itself, so a chair's shadow stays put as they sit down.
#[test]
fn a_sitters_chair_casts_the_shadow_they_do_not() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
    let (mut standing, mut sitting) = (false, false);
    for frame in &frames {
        for (span, kind) in collect_pieces(
            frame,
            Office {
                layout: &layout,
                pack: &pack,
                theme,
                scale: RenderScale::ONE,
            },
            &Moment::resolve(
                crate::sky::Sky::clock(std::time::UNIX_EPOCH),
                theme,
                0.0,
                Motion::Full.clock(std::time::UNIX_EPOCH),
            ),
        ) {
            let PieceKind::Character {
                ref figure,
                body,
                chair,
                ..
            } = kind
            else {
                continue;
            };
            let cast = ground_shadow(span, &kind, &pack).expect("every figure grounds");
            if figure.shadow {
                standing = true;
                assert_eq!(
                    cast,
                    crate::ground::Contact::under(body.x0, body.x1 - body.x0 + 1, body.y1 + 1)
                );
            } else {
                sitting = true;
                let at = chair.expect("a desk sitter carries their chair");
                let (w, h) = art_size(&pack, crate::pack::DESK_CHAIR_SPRITE).expect("chair art");
                let empty = piece_span(crate::layout::Pivot::TopLeft, at, w, h, 0);
                assert_eq!(
                    Some(cast),
                    ground_shadow(empty, &PieceKind::Chair { at }, &pack),
                    "the chair casts the shadow it cast empty"
                );
            }
        }
    }
    assert!(standing && sitting, "the walk in and the sit");
}

/// A pack with no back-view art flips its front view, over the sitters.
#[test]
fn a_flipped_sofa_sorts_in_front_of_its_sitters() {
    let pack = pixtuoid_core::sprite::format::load_pack_from_strings(
        "[pack]\nname=\"t\"\nversion=\"1\"\n[palette]\n\".\"=\"transparent\"\n\
         \"F\"=\"#202020\"\n[animations.meeting_sofa]\nframes=[\"s.sprite\"]\nframe_ms=100\n",
        &[("s.sprite", "@frame 0\nF F F\nF F F\n")],
    )
    .expect("a pack of one sofa loads");
    let at = crate::layout::Point { x: 40, y: 30 };
    let mut order = Vec::new();
    push_sofa(&mut order, &pack, at, true, Tie::FixtureOver);
    let [
        (
            span,
            PieceKind::Prop {
                art:
                    Art {
                        flip: Flip::Vertical,
                        ..
                    },
                ..
            },
        ),
    ] = order.as_slice()
    else {
        panic!("a flipped sofa is one mirrored prop: {order:?}");
    };
    let sitter = Span::new(at.x, at.y, 1, 1, 0)
        .with_depth(crate::sim::seat::sofa_sitter_sort_row(at))
        .with_layer(Layer::Figure);
    assert_eq!(
        crate::display::depth_sort(vec![(*span, "sofa"), (sitter, "sitter")]),
        ["sitter", "sofa"]
    );
}

/// A frame of an office nobody is in, its room lights full and its sign
/// calm.
pub(crate) fn empty_frame(layout: &SceneLayout) -> SimFrame {
    SimFrame {
        agents: Vec::new(),
        poses: std::collections::HashMap::new(),
        seated_agents: std::collections::HashMap::new(),
        characters: Vec::new(),
        indoor_scale: 1.0,
        neon: crate::floor::NeonLevels::CALM,
        chitchat_bubbles: Vec::new(),
        new_coffee_carriers: Vec::new(),
        occupied_waypoints: Default::default(),
        pet: None,
        mascots: Vec::new(),
        desks: vec![Default::default(); layout.home_desks.len()],
        door_frame: 0,
    }
}

/// The pieces the fixtures `keep` picks queue ([`push_fixture`]) in an
/// empty office.
pub(crate) fn queued(
    layout: &SceneLayout,
    pack: &Pack,
    scale: RenderScale,
    carried: &[crate::layout::Point],
    keep: impl Fn(FixtureKind) -> bool,
) -> Vec<(Span, PieceKind)> {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let frame = empty_frame(layout);
    let now = std::time::UNIX_EPOCH;
    let moment = Moment::resolve(
        crate::sky::Sky::clock(now),
        theme,
        0.0,
        Motion::Full.clock(now),
    );
    let build = Build {
        frame: &frame,
        office: Office {
            layout,
            pack,
            theme,
            scale,
        },
        moment: &moment,
    };
    let mut order = Vec::new();
    for f in layout.fixtures().filter(|f| keep(f.kind)) {
        push_fixture(f, build, carried, &mut order);
    }
    order
}

fn near_seat(desk: crate::layout::Point) -> crate::layout::Point {
    crate::sim::seated_top_left(
        desk,
        crate::layout::CHARACTER_SPRITE_W,
        crate::layout::Facing::North,
    )
}

pub(crate) fn base_size(pack: &Pack, name: &str) -> (u16, u16) {
    let f = pack
        .animation(name)
        .and_then(|a| a.frames().first())
        .expect("the bundled pack has this piece");
    (f.width(), f.height())
}

/// The glass `push_windows` queues for `layout` at `moment` under
/// `weather`, at the densest scale.
fn glass_views(layout: &SceneLayout, moment: &Moment, weather: &GlassWeather) -> Vec<WindowView> {
    let pack = test_default_pack();
    let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
    let mut order = Vec::new();
    push_windows(
        Office {
            layout,
            pack: &pack,
            theme: &crate::theme::NORMAL,
            scale,
        },
        moment,
        weather,
        &mut order,
    );
    order
        .into_iter()
        .map(|(_, kind)| match kind {
            PieceKind::Glass { view } => view,
            other => panic!("a window is glass: {other:?}"),
        })
        .collect()
}

pub(crate) fn moment_at(w: crate::sky::Weather, now: std::time::SystemTime) -> Moment {
    Moment::resolve(
        crate::sky::Sky::at_with(now, w),
        &crate::theme::NORMAL,
        0.0,
        Motion::Full.clock(now),
    )
}

/// Against the same sky under a clear model, a weather changes only glass
/// pixels — never a frame cell, which stays the backdrop's — and every one
/// that veils or falls changes some.
#[test]
fn the_weather_shows_on_the_glass_and_only_there() {
    use crate::sky::Weather;
    for (w, h) in [(160, 96), (240, 135)] {
        let layout = SceneLayout::compute_with_seed(w, h, None, 0).expect("lays out");
        for hour in [12, 0] {
            let now = crate::localclock::at_hour(hour);
            let clear = GlassWeather::of(&moment_at(Weather::Clear, now));
            for weather in Weather::ALL {
                let moment = moment_at(weather, now);
                let shown = glass_views(&layout, &moment, &GlassWeather::of(&moment));
                let crisp = glass_views(&layout, &moment, &clear);
                let mut changed = 0;
                for (a, b) in shown.iter().zip(&crisp) {
                    for (pa, pb) in a.px.iter().zip(&b.px) {
                        assert_eq!(pa.is_some(), pb.is_some(), "{weather:?} on a frame");
                        changed += usize::from(pa != pb);
                    }
                }
                let shows = weather != Weather::Clear;
                assert_eq!(changed > 0, shows, "{weather:?} at {w}x{h} {hour}h");
            }
        }
    }
}

/// The canvas repaints a window only when its fingerprint moves: over one
/// sky, the glass weather's tick moves it where something falls and
/// nowhere else, and one key always gives one fingerprint.
#[test]
fn a_window_s_fingerprint_moves_with_the_weather_tick_where_it_falls() {
    use crate::sky::Weather;
    let layout = SceneLayout::compute_with_seed(160, 96, None, 0).expect("lays out");
    let at = |w, ms| {
        moment_at(
            w,
            crate::localclock::at_hour(12) + std::time::Duration::from_millis(ms),
        )
    };
    for w in Weather::ALL {
        let moment = at(w, 5_000);
        let prints = |ms| -> Vec<u64> {
            glass_views(&layout, &moment, &GlassWeather::of(&at(w, ms)))
                .into_iter()
                .map(|view| fingerprint(&PieceKind::Glass { view }))
                .collect()
        };
        assert_eq!(prints(5_000), prints(5_000), "{w:?}: one key");
        let falls = matches!(
            w,
            Weather::Rain | Weather::Storm | Weather::Snow | Weather::Windy
        );
        assert_eq!(prints(5_000) != prints(5_600), falls, "{w:?}");
    }
}

/// A REAL office's draw list, checked against every pairwise "must be
/// behind" fact its own geometry states — what a sort key cannot give you.
#[test]
fn a_real_offices_draw_list_satisfies_every_ordering_constraint() {
    let pack = test_default_pack();
    for (w, h) in [(160u16, 96u16), (240, 144), (100, 60)] {
        let layout = SceneLayout::compute_with_seed(w, h, None, 0).expect("lays out");
        let mut order = queued(&layout, &pack, RenderScale::ONE, &[], |_| true);
        wall_segments(&layout, &mut order);
        assert!(order.len() > 10, "{w}x{h} produced a trivial list");

        let spans: Vec<Span> = order.iter().map(|(s, _)| *s).collect();
        let tagged: Vec<(Span, usize)> = spans.iter().copied().zip(0..).collect();
        let produced = crate::display::depth_sort(tagged);
        assert_eq!(produced.len(), spans.len(), "{w}x{h} dropped a piece");
        assert_eq!(
            crate::display::check_order(&spans, &produced),
            None,
            "{w}x{h}: the draw list violates a constraint its geometry states"
        );
    }
}

/// Pins [`Span`]'s bounds contract for every piece kind and prop builder. A
/// pixel counts as WRITTEN where two paints over different fills agree, so
/// no colour is assumed to be one the paint never uses.
#[test]
fn every_piece_paints_only_inside_its_span() {
    use crate::floor::{FloorMeta, FloorSession};
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let (mut kinds, mut props) = (
        std::collections::BTreeSet::new(),
        std::collections::BTreeSet::new(),
    );
    let mut check = |pack: &Pack, frame: &SimFrame, layout: &SceneLayout, only_people: bool| {
        for s in [1, 3, pack.max_density_variant().get()] {
            let scale = RenderScale::new(s).expect("nonzero");
            let office = Office {
                layout,
                pack,
                theme,
                scale,
            };
            let moment = Moment::resolve(
                crate::sky::Sky::clock(std::time::UNIX_EPOCH),
                theme,
                0.0,
                Motion::Full.clock(std::time::UNIX_EPOCH),
            );
            for (span, kind) in collect_pieces(frame, office, &moment)
                .into_iter()
                .chain(signs(office, 0, quiet_board()))
            {
                if only_people
                    && !matches!(
                        kind,
                        PieceKind::Character { .. }
                            | PieceKind::Effect(_)
                            | PieceKind::Badge { .. }
                    )
                {
                    continue;
                }
                kinds.insert(kind_name(&kind));
                if let PieceKind::Prop { art, .. } | PieceKind::Animated { art, .. } = kind {
                    props.insert(art.sprite);
                }
                assert_eq!(
                    stray_pixel(&kind, span, layout, pack, theme, scale),
                    None,
                    "{kind:?} at scale {s} wrote a logical pixel outside {span:?}"
                );
                // At the densities the cutaway draws at.
                if s % pack.max_density_variant().get() == 0
                    && ground_shadow(span, &kind, pack).is_some()
                {
                    assert_eq!(
                        lowest_painted_row(&kind, layout, pack, theme, scale),
                        Some(span.y1),
                        "{kind:?} at scale {s} is grounded on a row it doesn't reach: {span:?}"
                    );
                }
            }
        }
    };
    // Every step of a walk to each desk facing and the sit, for the mover...
    for facing in [crate::layout::Facing::North, crate::layout::Facing::South] {
        let (layout, pack, frames, _) = sit_down(facing, 2);
        for frame in &frames {
            check(&pack, frame, &layout, true);
        }
        // ...the office around them once, a lit screen and a carried chair
        // included...
        check(
            &pack,
            frames.last().expect("a seated frame"),
            &layout,
            false,
        );
    }
    // ...the mover in every style the pack draws, so a box that forgot the
    // rows a style's hair rises by shows...
    let styles: std::collections::BTreeSet<_> = test_default_pack()
        .hairstyles()
        .map(|s| s.name().to_owned())
        .collect();
    let mut worn = std::collections::BTreeSet::new();
    for i in 0..1000 {
        let pack = test_default_pack();
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let dense = crate::pack::densest_frame(&pack, "walking", 0, scale).expect("the walk's art");
        let id = pixtuoid_core::AgentId::from_transcript_path(&format!("/style/{i}.jsonl"));
        let style = crate::character::dress_for(&pack, id, dense.frame, dense.head, dense.density)
            .and_then(|d| d.style);
        let Some(style) = style.filter(|s| !worn.contains(s)) else {
            continue;
        };
        worn.insert(style);
        let (layout, pack, frames, _) = sit_down_as(pack, crate::layout::Facing::South, 0, id);
        for frame in &frames {
            check(&pack, frame, &layout, true);
        }
        if worn == styles {
            break;
        }
    }
    assert_eq!(worn, styles, "the walks wore every style");
    // ...every effect a figure carries, on each step of a walk and the sit...
    let (layout, pack, frames, _) = sit_down(crate::layout::Facing::South, 2);
    for mut frame in frames {
        for c in &mut frame.characters {
            c.effects = every_effect(c.top_left);
        }
        check(&pack, &frame, &layout, true);
    }
    // ...every desk prop, each tower tier with a sheet mid-fall, both ways
    // a desk faces...
    for facing in [crate::layout::Facing::North, crate::layout::Facing::South] {
        let (layout, pack, frames, _) = sit_down(facing, 2);
        let mut frame = frames.last().expect("a seated frame").clone();
        for (i, d) in frame.desks.iter_mut().enumerate() {
            d.cup = Some(crate::sim::Cup::Steaming);
            d.token_tier = (i % usize::from(crate::token_meter::MAX_TIER + 1)) as u8;
            d.sheet_fall = Some(1);
        }
        check(&pack, &frame, &layout, false);
    }
    // ...a walk whose frames differ in size, so a span sized from the wrong
    // frame shows...
    const LONG_STRIDE: &str = "\
@frame 0
. n H H H H n .
n H H H H H H n
H H S S S S H H
H S e S S e S H
. S S S m S S .
. n S S S S n .
. B B B B B B .
B B B B B B B B
S B B B B B B S
. P P P P P P .
. P P P P P P .
. P . . . . P P
. P . . . . . P
";
    let uneven = crate::pack::test_pack_with(&[("walking_1.sprite", LONG_STRIDE)]);
    let (layout, uneven, frames, _) = sit_down_in(uneven, crate::layout::Facing::South, 0);
    for frame in &frames {
        check(&uneven, frame, &layout, true);
    }
    // ...and offices whose sizes gate in the pieces 160x96 lacks, empty.
    let pack = test_default_pack();
    for (w, h) in [(240u16, 144u16), (100, 60)] {
        let stepped = FloorSession::new()
            .step(
                crate::floor::FloorInputs {
                    scene: &pixtuoid_core::SceneState::uniform(16),
                    pack: &pack,
                    now: std::time::SystemTime::UNIX_EPOCH,
                    floor: FloorMeta::ground(),
                    pets: crate::floor::PetInputs::default(),
                },
                crate::layout::Size { w, h },
            )
            .expect("lays out");
        check(&pack, &stepped.frame, &stepped.layout, false);
    }
    assert_eq!(
        kinds.into_iter().collect::<Vec<_>>(),
        [
            "animated",
            "badge",
            "board",
            "chair",
            "character",
            "clock",
            "desk",
            "desk prop",
            "door",
            "effect",
            "glass",
            "hung decor",
            "indicator",
            "neon",
            "prop",
            "prop band",
            "table",
            "wall"
        ],
        "a piece kind went untested"
    );
    // Every fixture drawn from art must have been reached, the pantry
    // counter at both its sizes.
    for sprite in crate::layout::PANTRY_COUNTER_ANIMS.into_iter().chain([
        "meeting_sofa",
        "plant",
        "filing_cabinet",
        "meeting_chair",
        "coat_rack",
        "side_table",
        "floor_lamp",
        "kitchen_island",
        "pantry_bin",
        "fish_tank",
        "water_cooler",
        "vending_machine",
        "printer",
    ]) {
        assert!(
            props.contains(sprite),
            "no {sprite} prop was painted: {props:?}"
        );
    }
}

/// The pet and the gateway mascots stand as figures: each paints only
/// inside its span at every density, sorts on its feet's row as the
/// classic sorts it, faces as the sim turns it, grounds its shadow, and has
/// what rides on it straight after it; a degraded gateway's art is greyed.
#[test]
fn creatures_stand_as_figures_with_their_riders_after_them() {
    use crate::layout::{Pivot, sort_row_at};
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let pack = test_default_pack();
    let layout = SceneLayout::compute_with_seed(160, 96, None, 0).expect("lays out");
    let mut frame = empty_frame(&layout);
    let (cat, lobster) = (Point { x: 40, y: 70 }, Point { x: 110, y: 70 });
    frame.pet = Some(crate::sim::PetPlacement {
        kind: crate::pet::PetKind::Cat,
        pos: cat,
        flip: true,
        anim_name: "cat_walk",
        frame_idx: 1,
        effects: crate::effects::pet_hearts(cat, 300).collect(),
    });
    frame.mascots = vec![crate::sim::MascotPlacement {
        pos: lobster,
        size: crate::layout::Size { w: 14, h: 12 },
        anim_name: "lobster_walk",
        frame_idx: 0,
        name: "OpenClaw",
        instance: None,
        state: pixtuoid_core::state::DaemonState::Busy,
        effects: crate::effects::mascot_bubbles(
            lobster,
            12,
            2,
            Motion::Full.beat(std::time::UNIX_EPOCH),
        )
        .collect(),
        active_sessions: 1,
    }];
    // a second gateway, degraded, nearer the viewer so it sorts last
    let sick = Point { x: 110, y: 84 };
    frame.mascots.push(crate::sim::MascotPlacement {
        pos: sick,
        state: pixtuoid_core::state::DaemonState::Degraded,
        effects: Vec::new(),
        ..frame.mascots[0].clone()
    });
    let riders = [
        frame.pet.as_ref().map_or(0, |p| p.effects.len()),
        frame.mascots[0].effects.len(),
        0,
    ];
    for s in [1, pack.max_density_variant().get()] {
        let scale = RenderScale::new(s).expect("nonzero");
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale,
        };
        let list = list_at(&frame, office, 12);
        let pieces = list.pieces();
        let creatures: Vec<usize> = pieces
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(p.kind, PieceKind::Creature { .. }))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(creatures.len(), 3, "at scale {s}, the pet and the mascots");
        // The pet faces west; a mascot never turns.
        let facing = [Flip::Horizontal, Flip::None, Flip::None];
        let sickly = [false, false, true];
        for (((&i, ridden), flip), sick) in creatures.iter().zip(riders).zip(facing).zip(sickly) {
            let p = &pieces[i];
            let PieceKind::Creature { at, art, degraded } = p.kind else {
                unreachable!("filtered to creatures");
            };
            assert_eq!(art.flip, flip, "at scale {s} {} faces wrong", art.sprite);
            assert_eq!(
                degraded, sick,
                "at scale {s} the gateway at {at:?} misreads its state"
            );
            assert!(
                p.shadow.is_some(),
                "at scale {s} {} casts no shadow",
                art.sprite
            );
            let h = crate::pack::densest_frame(&pack, art.sprite, art.frame, RenderScale::ONE)
                .expect("the art")
                .logical
                .1;
            assert_eq!(p.span.depth, sort_row_at(Pivot::Center, at, h));
            assert_eq!(
                stray_pixel(&p.kind, p.span, &layout, &pack, theme, scale),
                None,
                "at scale {s} {:?} painted outside {:?}",
                p.kind,
                p.span
            );
            let after = pieces[i + 1..]
                .iter()
                .take_while(|q| matches!(q.kind, PieceKind::Effect(_)))
                .count();
            assert_eq!(after, ridden, "at scale {s} {} lost its riders", art.sprite);
        }
        let lobster_kind = |degraded| PieceKind::Creature {
            at: lobster,
            art: Art::still("lobster_rest"),
            degraded,
        };
        assert_ne!(
            painted_alone(&lobster_kind(true), &layout, &pack, theme, scale),
            painted_alone(&lobster_kind(false), &layout, &pack, theme, scale),
            "at scale {s} a degraded gateway looks as a healthy one"
        );
    }
}

/// The carpet takes the weather's tint: the list a rainy hour builds lays
/// the theme's carpet drawn toward the rain's tint, which no clear hour's
/// ground matches.
#[test]
fn the_ground_takes_the_weathers_tint() {
    use crate::sky::{Sky, Weather};
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let pack = test_default_pack();
    let layout = SceneLayout::compute_with_seed(160, 96, None, 0).expect("lays out");
    let frame = empty_frame(&layout);
    let now = crate::localclock::at_hour(12);
    let ground_in = |w: Weather| {
        let sky = Sky::at_with(now, w);
        let tint = crate::atmosphere::SkyTones::resolve(&sky, theme).ground_tint;
        let list = build_list(
            &frame,
            Office {
                layout: &layout,
                pack: &pack,
                theme,
                scale: RenderScale::ONE,
            },
            &Moment::resolve(sky, theme, 0.0, Motion::Full.clock(now)),
            crate::floor::FloorMeta::ground(),
            quiet_board(),
        );
        (list.ground(), tint)
    };
    let (rain, (tint, share)) = ground_in(Weather::Rain);
    assert_eq!(rain.base, theme.surface.carpet_base.mix(tint, share));
    assert_ne!(rain, ground_in(Weather::Clear).0);
}

/// A figure's dust paints straight before them and their other riders
/// straight after, so nothing sorts between a person and what rides on
/// them.
#[test]
fn riders_paint_beside_their_figure() {
    use crate::effects::EffectKind as K;
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let (layout, pack, frames, _) = sit_down(crate::layout::Facing::South, 2);
    let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
    let office = Office {
        layout: &layout,
        pack: &pack,
        theme,
        scale,
    };
    for mut frame in frames {
        for c in &mut frame.characters {
            c.effects = every_effect(c.top_left);
        }
        let list = list_at(&frame, office, 12);
        let kinds: Vec<Option<K>> = list
            .pieces()
            .iter()
            .map(|p| match p.kind {
                PieceKind::Effect(r) => Some(r.effect.kind),
                _ => None,
            })
            .collect();
        let figure = list
            .pieces()
            .iter()
            .position(|p| matches!(p.kind, PieceKind::Character { .. }))
            .expect("the figure");
        assert_eq!(
            kinds[figure - 1..=figure + 3],
            [
                Some(K::WalkingDust),
                None,
                Some(K::FlameCrown),
                Some(K::SleepZ),
                Some(K::WaitingMark)
            ]
        );
    }
}

/// What the incremental canvas relies on: two pieces sharing a span and a
/// fingerprint paint the same pixels, so one may stand in for the other.
/// Walked over every tick of a walk to a desk and a sit, where the figure's
/// and the desk's fingerprints both change (asserted below). Every piece is
/// compared at scale 1; at the densest scale only figures, the kind that
/// changes tick to tick (the glass is walked in
/// `a_window_s_fingerprint_moves_with_the_moment_it_shows`).
#[test]
fn one_span_and_fingerprint_always_paint_the_same_pixels() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let now = std::time::SystemTime::UNIX_EPOCH;
    // Keyed by scale too: a fingerprint holds within one scale (`Piece`).
    let mut painted: std::collections::HashMap<(u16, Span, u64), u64> =
        std::collections::HashMap::new();
    let (mut repeats, mut figure_changes, mut desk_changes) = (0, 0, 0);
    for facing in [crate::layout::Facing::North, crate::layout::Facing::South] {
        let (layout, pack, frames, desk) = sit_down(facing, 2);
        for s in [1, pack.max_density_variant().get()] {
            let scale = RenderScale::new(s).expect("nonzero");
            let mut last: Option<(u64, u64)> = None;
            for frame in &frames {
                let list = build_list(
                    frame,
                    Office {
                        layout: &layout,
                        pack: &pack,
                        theme,
                        scale,
                    },
                    &Moment::resolve(
                        crate::sky::Sky::clock(now),
                        theme,
                        0.0,
                        Motion::Full.clock(now),
                    ),
                    crate::floor::FloorMeta::ground(),
                    quiet_board(),
                );
                repeats += same_fingerprint_same_pixels(&mut painted, &list, &layout, |p| {
                    s == 1 || matches!(p.kind, PieceKind::Character { .. })
                });
                let fp_of = |want: fn(&PieceKind, crate::layout::Point) -> bool| {
                    list.pieces()
                        .iter()
                        .find(|p| want(&p.kind, desk))
                        .map(|p| p.fingerprint)
                };
                let figure = fp_of(|k, _| matches!(k, PieceKind::Character { .. }));
                let desk_fp = fp_of(|k, d| matches!(k, PieceKind::Desk { at, .. } if *at == d));
                if let (Some((f0, d0)), Some(f1), Some(d1)) = (last, figure, desk_fp) {
                    figure_changes += usize::from(f0 != f1);
                    desk_changes += usize::from(d0 != d1);
                }
                last = figure.zip(desk_fp);
            }
        }
    }
    assert!(repeats > 0, "no piece recurred, so nothing was compared");
    assert!(figure_changes > 0, "the walker never changed fingerprint");
    assert!(
        desk_changes > 0,
        "the home desk's screen never changed fingerprint"
    );
}

/// The glass half of the property: a window keeps its span while the sky
/// and the city's lights move under it, so only the fingerprint can tell
/// two moments twelve hours apart, at the densest scale where the view is art pixels.
#[test]
fn a_window_s_fingerprint_moves_with_the_moment_it_shows() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let (layout, pack, frames, _) = sit_down(crate::layout::Facing::South, 2);
    let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
    let mut painted = std::collections::HashMap::new();
    let glass = |now: std::time::SystemTime, painted: &mut _| {
        let list = build_list(
            &frames[0],
            Office {
                layout: &layout,
                pack: &pack,
                theme,
                scale,
            },
            &Moment::resolve(
                crate::sky::Sky::clock(now),
                theme,
                0.0,
                Motion::Full.clock(now),
            ),
            crate::floor::FloorMeta::ground(),
            quiet_board(),
        );
        let is_glass = |p: &Piece| matches!(p.kind, PieceKind::Glass { .. });
        same_fingerprint_same_pixels(painted, &list, &layout, is_glass);
        list.pieces()
            .iter()
            .filter(|p| is_glass(p))
            .map(|p| (p.span, p.fingerprint))
            .collect::<Vec<_>>()
    };
    let noon = std::time::UNIX_EPOCH + std::time::Duration::from_secs(12 * 3600);
    let (a, b) = (
        glass(std::time::UNIX_EPOCH, &mut painted),
        glass(noon, &mut painted),
    );
    assert!(!a.is_empty(), "the office has windows");
    assert_eq!(a.len(), b.len(), "the same windows at every hour");
    for ((span_a, fp_a), (span_b, fp_b)) in a.iter().zip(&b) {
        assert_eq!(span_a, span_b, "a window never moves");
        assert_ne!(
            fp_a, fp_b,
            "a window at {span_a:?} looks the same twelve hours apart"
        );
    }
    assert_eq!(glass(noon, &mut painted), b, "one moment, one fingerprint");
}

/// `frame`'s list at local `hour`, under a clear sky.
pub(crate) fn list_at<'a>(frame: &SimFrame, office: Office<'a>, hour: u32) -> DrawList<'a> {
    let now = crate::localclock::at_hour(hour);
    let sky = crate::sky::Sky::at_with(now, crate::sky::Weather::Clear);
    build_list(
        frame,
        office,
        &Moment::resolve(sky, office.theme, 0.0, Motion::Full.clock(now)),
        crate::floor::FloorMeta::ground(),
        quiet_board(),
    )
}

/// What the rest cache relies on: between noon and midnight a static
/// piece keeps its span and fingerprint, while the room's tone and its
/// lights move.
#[test]
fn noon_and_midnight_differ_only_in_dynamic_pieces() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
    let frame = frames.last().expect("a seated frame");
    for s in [1, pack.max_density_variant().get()] {
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale: RenderScale::new(s).expect("nonzero"),
        };
        let (noon, night) = (list_at(frame, office, 12), list_at(frame, office, 23));
        let at_rest = |list: &DrawList| -> Vec<(Span, u64)> {
            list.pieces()
                .iter()
                .filter(|p| p.kind.is_static())
                .map(|p| (p.span, p.fingerprint))
                .collect()
        };
        assert_eq!(at_rest(&noon), at_rest(&night), "a static piece moved");
        assert_ne!(noon.ambient, night.ambient, "the room keeps its noon tone");
        let lit =
            |list: &DrawList| -> Vec<u64> { list.lights().iter().map(|l| l.fingerprint).collect() };
        assert_ne!(lit(&noon), lit(&night), "the lights keep their noon levels");
    }
}

/// `list`'s lights over a flat `under`, relit over `rect` alone by the
/// frame's own pass: every light that meets it, over an all-lit room.
fn lights_over(list: &DrawList<'_>, layout: &SceneLayout, rect: Span) -> RgbBuffer {
    let (w, h) = (
        list.scale.to_buffer(layout.buf_w),
        list.scale.to_buffer(layout.buf_h),
    );
    let mut buf = RgbBuffer::filled(w, h, list.theme.surface.bg_fallback);
    let pen = Pen::for_pack(list.scale, list.pack);
    let lights: Vec<&crate::cutaway::light::LightView> =
        list.lights().iter().map(|l| &l.view).collect();
    crate::cutaway::light::net_pass(
        ArtRect {
            x: pen.art(rect.x0),
            y: pen.art(rect.y0),
            w: pen.art(rect.x1 - rect.x0 + 1),
            h: pen.art(rect.y1 - rect.y0 + 1),
        },
        &lights,
        (list.ambient, list.flash),
        &crate::cutaway::light::Emission::new(w, h),
        pen,
        &mut crate::cutaway::light::NetMemo::default(),
        &mut buf,
    );
    buf
}

/// The light half of the fingerprint property, through the frame's own
/// pass: a light's span relit from the same lights over the same room paints
/// alike, so a canvas repainting a light's span on a change to any light
/// that meets it is complete. Over a walk at three hours, at scale 1 and
/// the densest, where some light changes its bands in place.
#[test]
fn one_set_of_lights_one_set_of_pixels() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
    let mut painted: std::collections::HashMap<(u16, Span, Vec<u64>), u64> =
        std::collections::HashMap::new();
    let mut seen: std::collections::HashMap<(u16, Span), std::collections::BTreeSet<u64>> =
        std::collections::HashMap::new();
    let mut repeats = 0;
    for s in [1, pack.max_density_variant().get()] {
        let scale = RenderScale::new(s).expect("nonzero");
        for hour in [18, 20, 23] {
            for frame in frames.iter().step_by(4) {
                let list = list_at(
                    frame,
                    Office {
                        layout: &layout,
                        pack: &pack,
                        theme,
                        scale,
                    },
                    hour,
                );
                for light in list.lights() {
                    seen.entry((s, light.span))
                        .or_default()
                        .insert(light.fingerprint);
                    let meeting: Vec<u64> = list
                        .lights()
                        .iter()
                        .filter(|l| {
                            l.span.x0 <= light.span.x1
                                && light.span.x0 <= l.span.x1
                                && l.span.y0 <= light.span.y1
                                && light.span.y0 <= l.span.y1
                        })
                        .map(|l| l.fingerprint)
                        .collect();
                    let pixels = {
                        use std::hash::{Hash, Hasher};
                        let mut h = std::hash::DefaultHasher::new();
                        lights_over(&list, &layout, light.span)
                            .as_slice()
                            .hash(&mut h);
                        (list.ambient, h.finish())
                    };
                    match painted.entry((s, light.span, meeting)) {
                        std::collections::hash_map::Entry::Occupied(e) => {
                            assert_eq!(
                                *e.get(),
                                pixels.1,
                                "one set of lights at {:?} painted two ways",
                                light.span
                            );
                            repeats += 1;
                        }
                        std::collections::hash_map::Entry::Vacant(v) => {
                            v.insert(pixels.1);
                        }
                    }
                }
            }
        }
    }
    assert!(repeats > 0, "no light recurred, so nothing was compared");
    assert!(
        seen.values().any(|fps| fps.len() > 1),
        "no light changed its bands in place"
    );
}

/// A light alone, through the frame's own pass over the whole room under a
/// sky that darkens nothing, changes no pixel outside its span, at every
/// scale.
#[test]
fn a_light_paints_only_inside_its_span() {
    use crate::cutaway::light::{Ambient, Emission, NetMemo, net_pass};
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
    let frame = frames.last().expect("a seated frame");
    let (mut lights, mut lit) = (0, 0);
    for s in [1, 3, pack.max_density_variant().get()] {
        let scale = RenderScale::new(s).expect("nonzero");
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale,
        };
        let list = list_at(frame, office, 23);
        let pen = Pen::for_pack(scale, &pack);
        let (w, h) = (scale.to_buffer(layout.buf_w), scale.to_buffer(layout.buf_h));
        let whole = ArtRect {
            x: ArtPx(0),
            y: ArtPx(0),
            w: pen.art(layout.buf_w),
            h: pen.art(layout.buf_h),
        };
        for light in list.lights() {
            lights += 1;
            let mut buf = RgbBuffer::filled(w, h, theme.surface.bg_fallback);
            net_pass(
                whole,
                &[&light.view],
                (Ambient::default(), crate::cutaway::light::Flash::default()),
                &Emission::new(w, h),
                pen,
                &mut NetMemo::default(),
                &mut buf,
            );
            lit += buf
                .as_slice()
                .iter()
                .filter(|&&c| c != theme.surface.bg_fallback)
                .count();
            let stray = buf.as_slice().iter().enumerate().find(|&(i, &c)| {
                let (x, y) = (
                    scale.logical((i % usize::from(w)) as u16),
                    scale.logical((i / usize::from(w)) as u16),
                );
                c != theme.surface.bg_fallback
                    && !((light.span.x0..=light.span.x1).contains(&x)
                        && (light.span.y0..=light.span.y1).contains(&y))
            });
            assert_eq!(
                stray.map(|(i, _)| i),
                None,
                "a light at scale {s} wrote outside {:?}",
                light.span
            );
        }
    }
    assert!(lights > 0, "the night office has no lights");
    assert!(lit > 0, "no light lifted a pixel, so this pins nothing");
}

/// The cutaway's sign glows only where the sign is drawn, and its lamps
/// light the night room.
#[test]
fn the_cutaway_glows_only_around_its_sign() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
    let frame = frames.last().expect("a seated frame");
    let office = Office {
        layout: &layout,
        pack: &pack,
        theme,
        scale: RenderScale::new(pack.max_density_variant().get()).expect("nonzero"),
    };
    let (mut lamps, mut glows) = (0, 0);
    for hour in [12, 18, 23] {
        let list = list_at(frame, office, hour);
        let signs = list
            .pieces()
            .iter()
            .filter(|p| matches!(p.kind, PieceKind::Neon { .. }))
            .count();
        for light in list.lights() {
            lamps += usize::from(light.view.is(crate::lighting::EmitterKind::DeskLamp));
            let glow = light.view.is(crate::lighting::EmitterKind::NeonGlow);
            assert!(!glow || signs == 1, "{hour}:00 glows with no sign");
            glows += usize::from(glow);
        }
    }
    assert!(glows > 0, "the sign never glows");
    assert!(
        lamps > 0,
        "nothing lights the night room, so this pins nothing"
    );
}

#[test]
fn the_neon_glow_takes_its_tubes_hue() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let pack = test_default_pack();
    let layout = SceneLayout::compute_with_seed(160, 96, None, 0).expect("lays out");
    let office = Office {
        layout: &layout,
        pack: &pack,
        theme,
        scale: RenderScale::new(pack.max_density_variant().get()).expect("nonzero"),
    };
    for neon in [
        crate::floor::NeonLevels::CALM,
        crate::floor::NeonLevels::ALERT,
        crate::floor::NeonLevels {
            alert: 0.5,
            power: 1.0,
        },
    ] {
        let frame = SimFrame {
            neon,
            ..empty_frame(&layout)
        };
        let list = list_at(&frame, office, 23);
        let hue = list.pieces().iter().find_map(|p| match p.kind {
            PieceKind::Neon { hue, .. } => Some(hue),
            _ => None,
        });
        let glow = list
            .lights()
            .iter()
            .find(|l| l.view.is(crate::lighting::EmitterKind::NeonGlow))
            .map(|l| l.view.tint());
        assert_eq!(glow, Some(hue), "{neon:?}");
    }
}

/// Every desk's lamp pools where its art hangs the bulb, whichever way the
/// desk faces and at every density its art is drawn at: the cutaway's art
/// stands it on the side the desk faces.
#[test]
#[cfg(feature = "density-art")]
fn a_desk_lamp_pools_under_its_painted_bulb() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let pack = test_default_pack();
    let layout = SceneLayout::compute_with_seed(240, 144, None, 0).expect("lays out");
    for s in [1, pack.max_density_variant().get()] {
        let scale = RenderScale::new(s).expect("nonzero");
        let pen = Pen::for_pack(scale, &pack);
        let frame = empty_frame(&layout);
        let list = list_at(
            &frame,
            Office {
                layout: &layout,
                pack: &pack,
                theme,
                scale,
            },
            23,
        );
        // Each painted bulb's middle, in art pixels.
        let bulbs: Vec<(f32, f32)> = layout
            .home_desks
            .iter()
            .enumerate()
            .filter_map(|(i, &at)| {
                let art = desk_art(
                    &pack,
                    layout.desk_facing(pixtuoid_core::state::FloorLocalDeskIndex(i)),
                )?;
                let span = desk_span(&pack, art, at, scale)?;
                let desk = crate::pack::densest_frame(&pack, art, 0, scale)?;
                let cells = drawn_in(&desk, &[crate::pack::DESK_BULB_KEY]);
                let w = usize::from(desk.frame.width());
                let hits: Vec<(f32, f32)> = cells
                    .iter()
                    .enumerate()
                    .filter(|&(_, &b)| b)
                    .map(|(i, _)| ((i % w) as f32, (i / w) as f32))
                    .collect();
                // Art pixels of this art, on the pen's grid.
                let k = f32::from(pen.art(1).0) / f32::from(desk.density.get());
                (!hits.is_empty()).then(|| {
                    let n = hits.len() as f32;
                    (
                        f32::from(pen.art(span.x0).0)
                            + hits.iter().map(|h| h.0).sum::<f32>() / n * k,
                        f32::from(pen.art(span.y0).0)
                            + hits.iter().map(|h| h.1).sum::<f32>() / n * k,
                    )
                })
            })
            .collect();
        assert!(!bulbs.is_empty(), "the pack's desks draw no bulb");
        let lamps: Vec<(f32, f32)> = list
            .lights()
            .iter()
            .filter(|l| l.view.is(crate::lighting::EmitterKind::DeskLamp))
            .map(|l| l.view.peak())
            .collect();
        assert_eq!(lamps.len(), bulbs.len(), "one pool a bulb");
        let near = f32::from(pen.art(1).0);
        for bulb in &bulbs {
            assert!(
                lamps
                    .iter()
                    .any(|p| (p.0 - bulb.0).abs() <= near && (p.1 - bulb.1).abs() <= near),
                "no lamp pools within {near} art px of the bulb at {bulb:?}: {lamps:?}"
            );
        }
    }
}

#[test]
fn a_piece_takes_the_pixels_it_paints_in_the_colour_already_there() {
    use crate::cutaway::light::Glow;
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let (layout, pack, frames, desk) = sit_down(crate::layout::Facing::North, 2);
    let frame = frames.last().expect("a seated frame");
    let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
    let office = Office {
        layout: &layout,
        pack: &pack,
        theme,
        scale,
    };
    let mut list = list_at(frame, office, 12);
    let (painted, _) = by_day(&list, &layout);
    let pen = Pen::for_pack(scale, &pack);
    let (w, h) = (pen.art(4).0, pen.art(2).0);
    let (x, y) = (pen.art(desk.x).0, pen.art(desk.y).0);
    let (bx, by) = (pen.buffer(ArtPx(x)), pen.buffer(ArtPx(y)));
    let px = (0..h)
        .flat_map(|dy| (0..w).map(move |dx| (dx, dy)))
        .map(|(dx, dy)| Some(painted.get(pen.buffer(ArtPx(x + dx)), pen.buffer(ArtPx(y + dy)))))
        .collect();
    list.pieces.push(Piece {
        span: Span::new(desk.x, desk.y, 4, 2, 0),
        kind: PieceKind::Glass {
            view: WindowView { x, y, w, px },
        },
        shadow: None,
        fingerprint: 0,
    });
    let (repainted, glow) = by_day(&list, &layout);
    assert_eq!(
        repainted.get(bx, by),
        painted.get(bx, by),
        "the pane repaints the desk's colour"
    );
    assert_eq!(glow.get(bx, by), Glow::Pane);
}

/// A static piece keeps its fingerprint whatever the hour and whoever is at
/// the appliances; a busy appliance plays, so it is not one.
#[test]
fn a_static_piece_holds_through_the_hours_and_the_queue() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let pack = test_default_pack();
    let layout = SceneLayout::compute_with_seed(240, 144, None, 0).expect("lays out");
    let idle = empty_frame(&layout);
    let mut busy = idle.clone();
    busy.occupied_waypoints = (0..layout.waypoints.len()).collect();
    let office = Office {
        layout: &layout,
        pack: &pack,
        theme,
        scale: RenderScale::new(pack.max_density_variant().get()).expect("nonzero"),
    };
    let lists: Vec<Vec<(Span, u64, bool, bool)>> =
        [(&idle, 12), (&busy, 12), (&idle, 23), (&busy, 23)]
            .into_iter()
            .map(|(frame, hour)| {
                list_at(frame, office, hour)
                    .pieces()
                    .iter()
                    .map(|p| {
                        (
                            p.span,
                            p.fingerprint,
                            p.kind.is_static(),
                            matches!(p.kind, PieceKind::Animated { art, .. }
                                if ["vending_machine", "printer"].contains(&art.sprite)),
                        )
                    })
                    .collect()
            })
            .collect();
    let at_rest = |l: &Vec<(Span, u64, bool, bool)>| -> Vec<(Span, u64)> {
        l.iter().filter(|p| p.2).map(|p| (p.0, p.1)).collect()
    };
    for l in &lists[1..] {
        assert_eq!(at_rest(&lists[0]), at_rest(l), "a static piece moved");
    }
    let appliances = |l: &Vec<(Span, u64, bool, bool)>| -> Vec<u64> {
        l.iter().filter(|p| p.3).map(|p| p.1).collect()
    };
    assert!(
        !appliances(&lists[0]).is_empty(),
        "the office has no appliance"
    );
    assert_ne!(
        appliances(&lists[0]),
        appliances(&lists[1]),
        "a busy appliance holds its rest frame, so this pins nothing"
    );
}

/// The figure half of the property, where it is hardest: a sitter keeps
/// their span while each thing their figure is painted from changes, so
/// only the fingerprint can tell the frames apart. Every variant is painted
/// at scale 1 and the densest, and any two sharing a span and fingerprint
/// must paint alike.
#[test]
fn a_sitters_fingerprint_moves_with_everything_their_figure_paints_from() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let now = std::time::SystemTime::UNIX_EPOCH;
    let mut painted = std::collections::HashMap::new();
    let (mut figures, mut built) = (std::collections::HashSet::new(), 0);
    for facing in [crate::layout::Facing::North, crate::layout::Facing::South] {
        let (layout, pack, frames, _) = sit_down(facing, 2);
        let seated = frames
            .iter()
            .rev()
            .find(|f| f.characters.first().is_some_and(|c| c.seat_desk.is_some()))
            .expect("the fixture sits down");
        let other = pixtuoid_core::AgentId::from_transcript_path("/cutaway/other.jsonl");
        let variants: [&FigureEdit; 7] = [
            &|_, _| {},
            &|c, a| {
                c.glow = crate::sim::CharacterGlow::Tool;
                a.state = pixtuoid_core::state::ActivityState::Active {
                    tool_use_id: None,
                    detail: None,
                    kind: pixtuoid_core::state::ToolKind::Read,
                };
            },
            &|_, a| a.model = Some(std::sync::Arc::from("claude-fable-5")),
            &|_, a| a.cwd = std::sync::Arc::from(std::path::Path::new("/w/other")),
            &|c, _| c.flip_x = !c.flip_x,
            &|c, _| {
                c.anim_name = "seated_sleeping";
                c.frame_idx = 0;
            },
            &move |_, a| a.agent_id = other,
        ];
        // A set: without the density art the densest scale is 1.
        let scales = std::collections::BTreeSet::from([1, pack.max_density_variant().get()]);
        for s in scales {
            let scale = RenderScale::new(s).expect("nonzero");
            for edit in variants {
                built += 1;
                let frame = varied(seated, edit);
                let list = build_list(
                    &frame,
                    Office {
                        layout: &layout,
                        pack: &pack,
                        theme,
                        scale,
                    },
                    &Moment::resolve(
                        crate::sky::Sky::clock(now),
                        theme,
                        0.0,
                        Motion::Full.clock(now),
                    ),
                    crate::floor::FloorMeta::ground(),
                    quiet_board(),
                );
                same_fingerprint_same_pixels(&mut painted, &list, &layout, |p| {
                    matches!(p.kind, PieceKind::Character { .. })
                });
                for p in list.pieces() {
                    if matches!(p.kind, PieceKind::Character { .. }) {
                        figures.insert((s, p.span, p.fingerprint));
                    }
                }
            }
        }
    }
    // One figure per facing, scale and variant, no two alike: had a variant
    // left the fingerprint unmoved, it would have been compared above.
    assert_eq!(
        figures.len(),
        built,
        "a variant did not move the fingerprint"
    );
}

/// An edit to one figure and its agent.
type FigureEdit = dyn Fn(&mut crate::sim::CharacterPlacement, &mut pixtuoid_core::AgentSlot);

/// `frame` with `edit` applied to its first figure and that figure's agent.
fn varied(frame: &SimFrame, edit: &FigureEdit) -> SimFrame {
    let mut varied = frame.clone();
    if let Some(c) = varied.characters.first_mut() {
        edit(c, &mut varied.agents[c.agent_idx]);
    }
    varied
}

/// Building one frame twice gives the same list, so a caller diffing two
/// frames sees only what moved.
#[test]
fn one_frame_builds_one_list() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 0);
    let frame = frames.last().expect("a seated frame");
    let now = std::time::SystemTime::UNIX_EPOCH;
    let summary = |list: &DrawList| {
        list.pieces()
            .iter()
            .map(|p| (p.span, p.fingerprint))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        summary(&build_list(
            frame,
            Office {
                layout: &layout,
                pack: &pack,
                theme,
                scale: RenderScale::ONE
            },
            &Moment::resolve(
                crate::sky::Sky::clock(now),
                theme,
                0.0,
                Motion::Full.clock(now)
            ),
            crate::floor::FloorMeta::ground(),
            quiet_board()
        )),
        summary(&build_list(
            frame,
            Office {
                layout: &layout,
                pack: &pack,
                theme,
                scale: RenderScale::ONE
            },
            &Moment::resolve(
                crate::sky::Sky::clock(now),
                theme,
                0.0,
                Motion::Full.clock(now)
            ),
            crate::floor::FloorMeta::ground(),
            quiet_board()
        )),
    );
}

/// Each drawn agent has one hover box, inside the span of a piece that
/// paints, in draw order, and every badge belongs to one of them.
#[test]
fn each_drawn_agent_hovers_inside_its_piece() {
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
    for frame in &frames {
        let list = build_list(
            frame,
            Office {
                layout: &layout,
                pack: &pack,
                theme,
                scale: RenderScale::ONE,
            },
            &Moment::resolve(
                crate::sky::Sky::clock(std::time::SystemTime::UNIX_EPOCH),
                theme,
                0.0,
                Motion::Full.clock(std::time::SystemTime::UNIX_EPOCH),
            ),
            crate::floor::FloorMeta::ground(),
            quiet_board(),
        );
        let pieces: Vec<&Piece> = list
            .pieces()
            .iter()
            .filter(|p| matches!(p.kind, PieceKind::Character { .. }))
            .collect();
        let hovers: Vec<(pixtuoid_core::AgentId, Span)> = list
            .hover_spans()
            .filter_map(|(body, agent)| Some((agent?, body)))
            .collect();
        assert_eq!(hovers.len(), pieces.len());
        assert_eq!(hovers.len(), frame.characters.len());
        for ((_, body), piece) in hovers.iter().zip(&pieces) {
            let span = piece.span;
            assert!(
                span.x0 <= body.x0
                    && body.x1 <= span.x1
                    && span.y0 <= body.y0
                    && body.y1 <= span.y1,
                "hover box {body:?} leaves its piece's span {span:?}"
            );
            let [a, b] =
                painted_over_two_fills(&piece.kind, &layout, &pack, theme, RenderScale::ONE);
            assert!(
                a.as_slice()
                    .iter()
                    .zip(b.as_slice())
                    .any(|(pa, pb)| pa == pb),
                "hover box {body:?} belongs to a piece that paints nothing"
            );
        }
        // Each drawn agent's badge, known by its text.
        let namesakes = crate::overlay::Namesakes::of(&frame.agents);
        let mut badged: Vec<_> = list.badges().map(|b| b.text.clone()).collect();
        let mut drawn: Vec<_> = hovers
            .iter()
            .filter_map(|&(id, _)| frame.agents.iter().find(|a| a.agent_id == id))
            .map(|a| namesakes.text(a))
            .collect();
        badged.sort();
        drawn.sort();
        assert_eq!(badged, drawn);
    }
}

pub(crate) fn kind_name(kind: &PieceKind) -> &'static str {
    match kind {
        PieceKind::WallSeg { .. } => "wall",
        PieceKind::Desk { .. } => "desk",
        PieceKind::Chair { .. } => "chair",
        PieceKind::Prop { .. } => "prop",
        PieceKind::PropBand { .. } => "prop band",
        PieceKind::Table { .. } => "table",
        PieceKind::Animated { .. } => "animated",
        PieceKind::Door { .. } => "door",
        PieceKind::Neon { .. } => "neon",
        PieceKind::Clock { .. } => "clock",
        PieceKind::Character { .. } => "character",
        PieceKind::Glass { .. } => "glass",
        PieceKind::Hung { .. } => "hung decor",
        PieceKind::Effect(_) => "effect",
        PieceKind::Badge { .. } => "badge",
        PieceKind::DeskProp(_) => "desk prop",
        PieceKind::Creature { .. } => "creature",
        PieceKind::Board { .. } => "board",
        PieceKind::Indicator { .. } => "indicator",
    }
}

/// One of each effect a figure carries, riding on its `top_left`, each at a
/// step it shows at.
pub(crate) fn every_effect(top_left: crate::layout::Point) -> Vec<crate::effects::Effect> {
    let at = |ms| std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms);
    vec![
        crate::effects::walking_dust(top_left, 0),
        crate::effects::flame_crown(top_left, 8, Motion::Full.beat(at(0))),
        crate::effects::sleep_z(top_left, 0, Motion::Full.beat(at(500)))
            .expect("a z rising at 500 ms"),
        crate::effects::waiting_mark(top_left),
    ]
}

/// Splitting is what makes the office above orderable, so pin it directly:
/// no band of a N-S wall may be tall enough to span a figure. An E-W wall
/// is whole on one row, which a figure is wholly north or south of.
#[test]
fn no_wall_segment_is_taller_than_the_cast() {
    let pack = test_default_pack();
    let (_, body_h) = base_size(&pack, "standing");
    let layout = SceneLayout::compute_with_seed(240, 144, None, 0).expect("lays out");
    let mut order: Vec<(Span, PieceKind)> = Vec::new();
    wall_segments(&layout, &mut order);
    order.retain(|(_, k)| {
        matches!(
            k,
            PieceKind::WallSeg {
                piece: crate::layout::WallPiece::Vertical { .. },
                ..
            }
        )
    });
    assert!(!order.is_empty(), "a laid-out office has N-S walls");
    for (span, _) in &order {
        let h = span.y1 - span.y0 + 1;
        assert!(
            h <= body_h,
            "a {h}-row wall segment can span the {body_h}-row cast, so one \
             segment would be both in front of and behind the same figure"
        );
    }
}

/// THE property the whole mixed-density contract rests on: a density variant
/// changes how a piece is DRAWN, never how big it is. `densest_frame`'s
/// variant and base arms return different (frame, factor) pairs whose
/// PRODUCT has to agree with the logical size the desk's foot (its face) is
/// placed by, and getting it wrong is silent — the desk
/// still renders, with its foot a whole desk below the surface.
#[test]
fn the_drawn_size_is_the_same_whichever_density_the_art_came_from() {
    let pack = test_default_pack();
    let (bw, bh) = base_size(&pack, "desk");
    for s in 1..=12u16 {
        let scale = RenderScale::new(s).expect("nonzero");
        let d = crate::pack::densest_frame(&pack, "desk", 0, scale).expect("desk is in the pack");
        let drawn = (
            d.frame.width() * d.blit_at.get(),
            d.frame.height() * d.blit_at.get(),
        );
        assert_eq!(
            drawn,
            (scale.to_buffer(bw), scale.to_buffer(bh)),
            "scale {s} drew a different size than the base art implies"
        );
        assert_eq!(
            drawn,
            (scale.to_buffer(d.logical.0), scale.to_buffer(d.logical.1)),
            "scale {s} drew a different size than the foot is placed by"
        );
    }
}

/// A back-view sofa sits its sitter between its two bands: the seat under
/// them, the backrest, nearest the viewer, over their lap.
#[test]
fn a_back_view_sofa_seats_its_sitter_between_its_seat_and_its_backrest() {
    let pack = test_default_pack();
    let sofa = crate::layout::Point { x: 40, y: 30 };
    let mut order = Vec::new();
    push_sofa(&mut order, &pack, sofa, true, Tie::FixtureOver);
    let [
        (seat, PieceKind::PropBand { rows: under, .. }),
        (back, PieceKind::PropBand { rows: over, .. }),
    ] = order.as_slice()
    else {
        panic!("a back-view sofa is two bands: {order:?}");
    };
    let sitter = crate::sim::seat::sofa_sitter_sort_row(sofa);
    assert!(
        seat.depth == sitter && sitter < back.depth,
        "seat {} = sitter {sitter} < backrest {}",
        seat.depth,
        back.depth
    );
    let (_, h) = base_size(&pack, MEETING_SOFA_NORTH);
    assert_eq!((under.0, under.1, over.0, over.1), (0, over.0, under.1, h));
}

/// A meeting room at its tightest still stands the table behind the
/// back-view sofa's seat: sorted on its own south edge, the seat tied the
/// table there, and the table, pushed after it, painted over its cushions.
#[test]
fn the_table_sorts_behind_the_back_view_sofas_seat_in_the_tightest_room() {
    let pack = test_default_pack();
    let mut checked = 0;
    for (w, h) in [(110, 66), (130, 90)] {
        for seed in 0..4 {
            let Some(layout) = SceneLayout::compute_with_seed(w, h, None, seed) else {
                continue;
            };
            let order = queued(&layout, &pack, RenderScale::ONE, &[], |k| {
                matches!(
                    k,
                    FixtureKind::MeetingSofa { .. } | FixtureKind::MeetingTable { .. }
                )
            });
            let seats = order.iter().filter_map(|(s, k)| match k {
                PieceKind::PropBand { rows: (0, _), .. } => Some(s.depth),
                _ => None,
            });
            let tables = order.iter().filter_map(|(s, k)| match k {
                PieceKind::Table { .. } => Some(s.depth),
                _ => None,
            });
            for (seat, table) in seats.zip(tables) {
                assert!(table < seat, "{w}x{h}/{seed}: table {table}, seat {seat}");
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "the sizes lay out meeting trios");
}

/// An office with every moving fixture: an aquarium and a cooler that
/// loop, an elevator, the sign and the clock.
pub(crate) fn lively_office() -> SceneLayout {
    many_layouts()
        .find(|l| {
            let kinds: Vec<FixtureKind> = l.fixtures().map(|f| f.kind).collect();
            [
                FixtureKind::FishTank,
                FixtureKind::WaterCooler,
                FixtureKind::Door,
                FixtureKind::Clock,
            ]
            .iter()
            .all(|k| kinds.contains(k))
        })
        .expect("an office has a lounge aquarium, a pantry cooler, an elevator and a clock")
}

/// `frame`'s list at `now`, under a clear sky.
pub(crate) fn list_now<'a>(
    frame: &SimFrame,
    office: Office<'a>,
    now: std::time::SystemTime,
) -> DrawList<'a> {
    let sky = crate::sky::Sky::at_with(now, crate::sky::Weather::Clear);
    build_list(
        frame,
        office,
        &Moment::resolve(sky, office.theme, 0.0, Motion::Full.clock(now)),
        crate::floor::FloorMeta::ground(),
        quiet_board(),
    )
}

/// A desk sorts on the roster's row, the classic painter's: a figure one row
/// south of it draws over it, one row north draws under it.
#[test]
fn a_walker_just_south_of_a_desk_front_draws_over_it() {
    let pack = test_default_pack();
    let layout = SceneLayout::compute_with_seed(160, 96, None, 0).expect("lays out");
    let desk = layout
        .fixtures()
        .find(|f| matches!(f.kind, FixtureKind::Desk(_)))
        .expect("a desk");
    let Depth::Sorted { row, .. } = desk.depth else {
        panic!("a desk sorts: {desk:?}");
    };
    let [(span, PieceKind::Desk { .. })] =
        queued(&layout, &pack, RenderScale::ONE, &[], |k| k == desk.kind)[..]
    else {
        panic!("one desk piece");
    };
    assert_eq!(span.depth, row, "the desk sorts on the roster's row");
    let walker = |depth| Span::new(span.x0, span.y0, 4, 8, 0).with_depth(depth);
    for (depth, over) in [(row + 1, true), (row - 1, false)] {
        let drawn = depth_sort(vec![(span, "desk"), (walker(depth), "walker")]);
        assert_eq!(
            drawn[1] == "walker",
            over,
            "a walker sorted on {depth}, the desk on {row}"
        );
    }
}

/// A figure at a fixture piece's row paints over it, but for a desk chair
/// and a sofa seen from behind, which hide their sitters; a back-view
/// sofa's seat, which its sitter sits on, stays under.
#[test]
fn a_fixture_ties_a_figure_as_the_roster_says() {
    let pack = test_default_pack();
    let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
    let mut over = std::collections::BTreeSet::new();
    for layout in many_layouts() {
        for fixture in layout.fixtures() {
            if fixture.depth == Depth::Backdrop {
                continue;
            }
            use FixtureKind as K;
            let hides_its_sitter = match fixture.kind {
                K::DeskChair(_) | K::LoungeCouch => true,
                K::MeetingSofa { faces_away, .. } => faces_away,
                K::Desk(_)
                | K::FilingCabinet(_)
                | K::Station { .. }
                | K::Plant { .. }
                | K::Pod { .. }
                | K::Wall { .. }
                | K::MeetingRug { .. }
                | K::MeetingTable { .. }
                | K::MeetingChair { .. }
                | K::CoatRack { .. }
                | K::Doormat { .. }
                | K::NoticeBoard { .. }
                | K::LoungeRug
                | K::SideTable
                | K::FloorLamp
                | K::FishTank
                | K::KitchenIsland
                | K::PantryMat
                | K::IslandMat
                | K::WaterCooler
                | K::TrashBin
                | K::Door
                | K::Runner
                | K::NeonSign
                | K::Clock => false,
            };
            for (span, kind) in queued(&layout, &pack, scale, &[], |k| k == fixture.kind) {
                let seat = matches!(kind, PieceKind::PropBand { rows: (0, _), .. });
                let body = Span::new(span.x0, span.y0, 1, 1, 0);
                let figure = occupant_span(body, span.depth, None);
                // The figure queued first, so push order alone would draw it under.
                let drawn = crate::display::depth_sort(vec![(figure, "figure"), (span, "fixture")]);
                let want = if hides_its_sitter && !seat {
                    over.insert(crate::layout::roster::tests::kind_key(fixture.kind));
                    ["figure", "fixture"]
                } else {
                    ["fixture", "figure"]
                };
                assert_eq!(drawn, want, "{:?}: {kind:?}", fixture.kind);
            }
        }
    }
    assert_eq!(
        over.len(),
        3,
        "the chair and both sofas hide a sitter: {over:?}"
    );
}

/// A glass wall band composites over whoever stands behind it at its row.
#[test]
fn a_wall_band_draws_over_a_figure_at_its_row() {
    let mut walls = 0;
    for layout in many_layouts() {
        let mut order = Vec::new();
        wall_segments(&layout, &mut order);
        for (span, _) in order {
            let figure = occupant_span(Span::new(span.x0, span.y0, 1, 1, 0), span.depth, None);
            let drawn = crate::display::depth_sort(vec![(span, "wall"), (figure, "figure")]);
            assert_eq!(drawn, ["figure", "wall"], "{span:?}");
            walls += 1;
        }
    }
    assert!(walls > 0, "no office had a wall");
}

/// The lounge couch faces the window, so the cutaway draws its back.
#[test]
fn the_lounge_couch_is_drawn_from_behind() {
    let pack = test_default_pack();
    let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
    let layout = many_layouts()
        .find(|l| l.lounge.is_some())
        .expect("an office with a lounge");
    let pieces = queued(&layout, &pack, scale, &[], |k| {
        k == FixtureKind::LoungeCouch
    });
    assert!(
        !pieces.is_empty()
            && pieces.iter().all(|(_, k)| matches!(
                k,
                PieceKind::PropBand { sprite, .. } if *sprite == MEETING_SOFA_NORTH
            )),
        "{pieces:?}"
    );
}

/// Layouts across the sizes and seeds that place every kind of piece this
/// module draws: pods with booths and desks, meeting rooms, a pantry.
pub(crate) fn many_layouts() -> impl Iterator<Item = SceneLayout> {
    // The small sizes are an 80x24-class terminal's, where a door can run
    // flush with its wall's end.
    [
        (64, 50),
        (80, 46),
        (80, 48),
        (160, 96),
        (200, 120),
        (240, 144),
        (320, 180),
    ]
    .into_iter()
    .flat_map(|(w, h)| {
        (0..4).filter_map(move |seed| SceneLayout::compute_with_seed(w, h, None, seed))
    })
}

/// Pins [`push_sofa`]'s front view on its sitters' key, so they paint over
/// it.
#[test]
fn a_front_view_sofa_ties_its_sitters() {
    let pack = test_default_pack();
    let sofa = crate::layout::Point { x: 40, y: 30 };
    let mut order = Vec::new();
    push_sofa(&mut order, &pack, sofa, false, Tie::FigureOver);
    let [
        (
            span,
            PieceKind::Prop {
                art: Art {
                    flip: Flip::None, ..
                },
                ..
            },
        ),
    ] = order.as_slice()
    else {
        panic!("a front sofa is one prop: {order:?}");
    };
    assert_eq!(span.depth, crate::sim::seat::sofa_sitter_sort_row(sofa));
}

/// No wall this module stands closes a doorway, and every doorway is framed
/// by two jambs.
#[test]
fn walls_leave_every_doorway_open_and_frame_it() {
    let mut checked = 0;
    for layout in many_layouts() {
        let mut order = Vec::new();
        wall_segments(&layout, &mut order);
        for d in &layout.doorways {
            let vertical = d.start.x == d.end.x;
            let (lo, hi) = if vertical {
                (d.start.y.min(d.end.y), d.start.y.max(d.end.y))
            } else {
                (d.start.x.min(d.end.x), d.start.x.max(d.end.x))
            };
            for v in lo + 1..hi {
                let (x, y) = if vertical {
                    (d.start.x, v)
                } else {
                    (v, d.start.y)
                };
                // Its own wall's pieces only: a crossing wall's glass rising
                // in front of the opening is occlusion, not a wall in it.
                let blocked = order.iter().any(|(s, k)| {
                    let own = match k {
                        PieceKind::WallSeg {
                            piece: crate::layout::WallPiece::Vertical { x: px, .. },
                            ..
                        } => vertical && *px == d.start.x,
                        PieceKind::WallSeg {
                            piece: crate::layout::WallPiece::Horizontal { y_face, .. },
                            ..
                        } => !vertical && *y_face == d.start.y,
                        _ => false,
                    };
                    own && (s.x0..=s.x1).contains(&x) && (s.y0..=s.y1).contains(&y)
                });
                assert!(
                    !blocked,
                    "a wall stands in the doorway at ({x}, {y}): {d:?}"
                );
            }
            checked += 1;
        }
        let jambs: std::collections::HashSet<_> = order
            .iter()
            .filter_map(|(_, k)| match k {
                PieceKind::WallSeg { piece, .. } => Some(*piece),
                _ => None,
            })
            .flat_map(|piece| {
                let (at, size) = piece.visual();
                piece.jambs().inspect(move |&(p, s)| {
                    assert!(
                        p.x >= at.x
                            && p.y >= at.y
                            && p.x + s.w <= at.x + size.w
                            && p.y + s.h <= at.y + size.h,
                        "a jamb {p:?}+{s:?} reaches past its wall {piece:?}"
                    );
                })
            })
            .collect();
        assert_eq!(
            jambs.len(),
            2 * layout.doorways.len(),
            "two jambs a doorway"
        );
    }
    assert!(checked > 0, "the layouts cut doorways");
}

/// The pantry counter stands where the layout stands it: on the Pantry
/// waypoint the mask blocks and the visitor faces.
#[test]
fn the_pantry_counter_stands_on_its_waypoint() {
    let pack = test_default_pack();
    let mut checked = 0;
    for layout in many_layouts() {
        let Some(wp) = layout
            .waypoints
            .iter()
            .find(|wp| wp.kind == crate::layout::WaypointKind::Pantry)
        else {
            continue;
        };
        let order = queued(&layout, &pack, RenderScale::ONE, &[], |k| {
            matches!(
                k,
                FixtureKind::Station {
                    station: Station::PantryCounter,
                    ..
                }
            )
        });
        let [(_, PieceKind::Prop { at, .. })] = order.as_slice() else {
            panic!("one counter: {order:?}");
        };
        assert_eq!(*at, wp.pos);
        checked += 1;
    }
    assert!(checked > 0, "the layouts have pantries");
}

/// No piece is queued twice, and every pod decor piece once: a phone booth
/// or a standing desk is pod decor, and its waypoint is only where a
/// visitor stands.
#[test]
fn no_piece_is_queued_twice() {
    let pack = test_default_pack();
    let mut booths = 0;
    for layout in many_layouts() {
        let order = queued(&layout, &pack, RenderScale::ONE, &[], |_| true);
        let mut seen = std::collections::HashSet::new();
        for (span, kind) in &order {
            assert!(
                seen.insert((span.x0, span.y0, span.x1, span.y1, fingerprint(kind))),
                "{} queued twice at {span:?}",
                kind_name(kind)
            );
        }
        for d in &layout.pod_decor {
            let queued = order
                .iter()
                .filter(|(_, k)| {
                    matches!(k, PieceKind::Prop { at, art }
                        if *at == d.pos && art.sprite == d.kind.sprite_name())
                })
                .count();
            assert_eq!(queued, 1, "{:?} at {:?}", d.kind, d.pos);
            booths += usize::from(d.kind == crate::layout::PodDecor::PhoneBooth);
        }
    }
    assert!(booths > 0, "the layouts place a phone booth");
}

/// Wall decor that stands on the ground sorts among the ground's pieces, so a
/// figure north of a whiteboard between pods goes behind it; decor that
/// only hangs on the band is left to the backdrop.
#[test]
fn ground_standing_wall_decor_sorts_with_the_ground() {
    let pack = test_default_pack();
    let mut standing = 0;
    for layout in many_layouts() {
        let order = queued(&layout, &pack, RenderScale::ONE, &[], |k| {
            matches!(k, FixtureKind::Wall { .. })
        });
        for item in &layout.wall_decor {
            let queued = order.iter().any(|(_, k)| {
                matches!(k, PieceKind::Prop { art, .. } if art.sprite == item.kind.sprite_name())
            });
            assert_eq!(queued, item.kind.stands_on_floor(), "{:?}", item.kind);
            standing += usize::from(queued);
        }
    }
    assert!(standing > 0, "the layouts place floor-standing decor");
}

/// The split row is the art's at every density: the back-view sofa's
/// backrest starts on its lit ridge, just under the seam where the seat
/// meets it, so the seam paints under the sitter and the ridge over them.
#[test]
fn the_north_sofas_backrest_starts_on_its_lit_ridge() {
    let pack = test_default_pack();
    let densities =
        std::iter::once(pixtuoid_core::sprite::format::Density::ONE).chain(pack.density_variants());
    for d in densities {
        let name = if d == pixtuoid_core::sprite::format::Density::ONE {
            MEETING_SOFA_NORTH.to_owned()
        } else {
            pixtuoid_core::sprite::format::density_variant_name(MEETING_SOFA_NORTH, d)
        };
        let Some(f) = pack.animation(&name).and_then(|a| a.frames().first()) else {
            continue;
        };
        let (x, split) = (f.width() / 2, NORTH_SOFA_SEAT_ROWS * d.get());
        let luma = |y: u16| {
            let c = f
                .get(x, y)
                .copied()
                .flatten()
                .expect("the sofa is opaque at its centre");
            c.lightness()
        };
        assert!(
            luma(split) > luma(split - 1),
            "{name}: row {split} must be the ridge, lit over the seam above it"
        );
    }
}

/// A sitter in front of a glowing screen takes the room's light over it: the
/// last piece to paint a pixel says how it glows.
#[test]
fn a_sitter_over_a_screen_takes_the_rooms_light() {
    use crate::cutaway::light::Glow;
    let theme = crate::theme::theme_by_name("normal").expect("theme");
    let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
    let frame = frames.last().expect("a seated frame");
    let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
    let office = Office {
        layout: &layout,
        pack: &pack,
        theme,
        scale,
    };
    let mut list = list_at(frame, office, 23);
    let (with, glow) = by_day(&list, &layout);
    list.pieces
        .retain(|p| !matches!(p.kind, PieceKind::Character { .. }));
    let (without, glow_without) = by_day(&list, &layout);
    let mut over = 0;
    for y in 0..with.height() {
        for x in 0..with.width() {
            if with.get(x, y) == without.get(x, y) {
                continue;
            }
            assert_eq!(glow.get(x, y), Glow::Lit, "({x}, {y}) of a sitter glows");
            over += usize::from(glow_without.get(x, y) != Glow::Lit);
        }
    }
    assert!(
        over > 0,
        "no sitter covers a screen, so nothing was compared"
    );
}
