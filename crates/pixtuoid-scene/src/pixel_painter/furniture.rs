//! Standalone furniture paint helpers — meeting table, area rug,
//! side table, kitchen island, and the procedural
//! room-fill decor (notice board, doormat, water cooler, trash bin).
//!
//! Extracted from `mod.rs` to keep the orchestrator focused on
//! the render pipeline rather than individual furniture geometry.

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::layout::Bounds;

/// Low meeting-room table between the sofas. Wood top with darker
/// trim along the front edge so it reads as a real piece of furniture,
/// not just a brown rectangle.
pub(super) fn paint_meeting_table(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    w: u16,
    h: u16,
    theme: &crate::theme::Theme,
) {
    let top = theme.furniture.wood_top;
    let trim = theme.furniture.wood_trim;
    let min_x = cx.saturating_sub(w / 2);
    let max_x = (cx + w / 2 + (w & 1)).min(buf.width());
    let min_y = cy.saturating_sub(h / 2);
    let max_y = (cy + h / 2 + (h & 1)).min(buf.height());
    for y in min_y..max_y {
        for x in min_x..max_x {
            let on_front = y + 1 == max_y;
            buf.put(x, y, if on_front { trim } else { top });
        }
    }
}

/// Meeting-room area rug — warm Persian-tone rectangle painted under
/// the meeting table. Border ring in a darker shade so the rug reads as
/// having a fringe/binding rather than a flat blob. Centred on `cx,cy`.
pub(super) fn paint_area_rug(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    w: u16,
    h: u16,
    theme: &crate::theme::Theme,
) {
    let rug_field = theme.furniture.rug_field;
    let rug_trim = theme.furniture.rug_trim;
    let rug_accent = theme.furniture.rug_accent;
    let half_w = w as i32 / 2;
    let half_h = h as i32 / 2;
    for dy in 0..h as i32 {
        for dx in 0..w as i32 {
            let px = cx as i32 - half_w + dx;
            let py = cy as i32 - half_h + dy;
            if px < 0 || py < 0 || px >= buf.width() as i32 || py >= buf.height() as i32 {
                continue;
            }
            let on_border = dx == 0 || dx == w as i32 - 1 || dy == 0 || dy == h as i32 - 1;
            let on_inner_border = dx == 1 || dx == w as i32 - 2 || dy == 1 || dy == h as i32 - 2;
            let color = if on_border {
                rug_trim
            } else if on_inner_border {
                rug_accent
            } else {
                rug_field
            };
            buf.put(px as u16, py as u16, color);
        }
    }
}

/// Lounge side table — 7×4 wood block next to the viewing couch
/// (opposite side from the floor lamp). Bumped from 5×3 to clear the
/// skill's ~5-cell-wide subzone threshold. Carries a 3-cell magazine
/// stack on top so the silhouette reads as "side table with a book".
pub(super) fn paint_side_table(buf: &mut RgbBuffer, cx: u16, cy: u16, theme: &crate::theme::Theme) {
    let top = theme.furniture.wood_top;
    let trim = theme.furniture.wood_trim;
    let mag = theme.furniture.magazine;
    let mag_trim = theme.furniture.magazine_trim;
    // Sprite dimensions from the one furniture table (== the mask footprint for
    // the side table) so the painted block can't drift from the blocked ground.
    let Some(fp) =
        crate::layout::furniture_def(crate::layout::Furniture::LoungeSideTable).footprint
    else {
        return;
    };
    let (w, h) = (fp.w as i32, fp.h as i32);
    for dy in 0..h {
        for dx in 0..w {
            let px = cx as i32 - w / 2 + dx;
            let py = cy as i32 - h / 2 + dy;
            if px < 0 || py < 0 || px >= buf.width() as i32 || py >= buf.height() as i32 {
                continue;
            }
            let on_bottom = dy == h - 1;
            buf.put(px as u16, py as u16, if on_bottom { trim } else { top });
        }
    }
    let mag_pixels: &[((i32, i32), Rgb)] = &[
        ((-1, -1), mag),
        ((0, -1), mag),
        ((1, -1), mag),
        ((-1, 0), mag_trim),
        ((0, 0), mag_trim),
        ((1, 0), mag_trim),
    ];
    for ((dx, dy), c) in mag_pixels {
        let px = cx as i32 + dx;
        let py = cy as i32 + dy;
        if px >= 0 && py >= 0 && (px as u16) < buf.width() && (py as u16) < buf.height() {
            buf.put(px as u16, py as u16, *c);
        }
    }
}

/// Kitchen island — the pantry's counter-height centre piece (centred at
/// `pos`; ALL dims read from the FurnitureDef row): 2 rows of dressed
/// countertop (a clustered fruit pair + one mug reusing the vending-drinks
/// accent palette — zero new theme fields), a cabinet body with door
/// seams and handles, and a base row. The mask blocks only the
/// south-anchored base (footprint.h = visual.h − 2, invariant #6).
pub(super) fn paint_kitchen_island(
    buf: &mut RgbBuffer,
    cx: u16,
    cy: u16,
    theme: &crate::theme::Theme,
) {
    let top = theme.furniture.wood_top;
    let body = theme.furniture.wood_trim;
    let shade = theme.furniture.chair_trim;
    let accents = theme.appliance.vending_drinks;
    let vis = crate::layout::furniture_def(crate::layout::Furniture::KitchenIsland).visual;
    let (w, h) = (vis.w as i32, vis.h as i32);
    for dy in 0..h {
        for dx in 0..w {
            let on_corner = (dx == 0 || dx == w - 1) && (dy == 0 || dy == h - 1);
            if on_corner {
                continue;
            }
            let px = cx as i32 - w / 2 + dx;
            let py = cy as i32 - h / 2 + dy;
            if px < 0 || py < 0 || px >= buf.width() as i32 || py >= buf.height() as i32 {
                continue;
            }
            // Rows 2+ (the cabinet body + base) inset 1px per side so the
            // countertop reads as OVERHANGING the cabinetry.
            if dy >= 2 && (dx == 0 || dx == w - 1) {
                continue;
            }
            let color = if dy < 2 {
                top // countertop surface
            } else if dy == h - 1 {
                shade // base row grounds the piece
            } else {
                body // front face
            };
            buf.put(px as u16, py as u16, color);
        }
    }
    // Front detail: two cabinet-door seams + handles so the body reads as
    // kitchen cabinetry, not a slab (rows 2..h-1, i.e. the front face).
    let putxy = |buf: &mut RgbBuffer, dx: i32, dy: i32, c: Rgb| {
        let px = cx as i32 - w / 2 + dx;
        let py = cy as i32 - h / 2 + dy;
        if px >= 0 && py >= 0 && (px as u16) < buf.width() && (py as u16) < buf.height() {
            buf.put(px as u16, py as u16, c);
        }
    };
    for dy in 2..(h - 1) {
        putxy(buf, w / 2, dy, shade); // centre seam splits two doors
    }
    putxy(buf, w / 2 - 2, 3, shade); // left door handle
    putxy(buf, w / 2 + 2, 3, shade); // right door handle
                                     // Countertop dressing (row 0): a CLUSTERED fruit bowl (two adjacent
                                     // accents) + one cup — clustered so it reads as objects, not confetti.
    putxy(buf, 3, 0, accents[0]);
    putxy(buf, 4, 0, accents[1]);
    // One mug — a THIRD accent so it can't blend into the fruit pair (the
    // vending panel color is theme-dependent and collided in default).
    putxy(buf, w - 5, 0, accents[2]);
}

/// Notice board on the meeting room's south wall (8×5 framed rectangle).
/// Painted in the background-fill pass; no-op for rooms too small to host it.
pub(super) fn paint_notice_board(buf: &mut RgbBuffer, mr: Bounds, theme: &crate::theme::Theme) {
    if !(mr.height > 20 && mr.width > 15) {
        return;
    }
    let wall_color = theme.office.room_wall_trim_dark;
    let accent = theme.furniture.rug_accent;
    let bx = mr.x + 4;
    let by = mr.y + mr.height - 8;
    for dy in 0..5u16 {
        for dx in 0..8u16 {
            let px = bx + dx;
            let py = by + dy;
            if px < buf.width() && py < buf.height() {
                let on_edge = dx == 0 || dx == 7 || dy == 0 || dy == 4;
                buf.put(px, py, if on_edge { wall_color } else { accent });
            }
        }
    }
}

/// Small doormat at the meeting-room entrance (4×5 bordered rug, cubicle side).
pub(super) fn paint_doormat(buf: &mut RgbBuffer, mr: Bounds, theme: &crate::theme::Theme) {
    if mr.width <= 10 {
        return;
    }
    let mat_x = mr.x + mr.width;
    let mat_y = mr.y + mr.height / 2 - 2;
    let mat_color = theme.furniture.rug_trim;
    let mat_accent = theme.furniture.rug_field;
    for dy in 0..5u16 {
        for dx in 0..4u16 {
            let px = mat_x + dx + 1;
            let py = mat_y + dy;
            if px < buf.width() && py < buf.height() {
                let on_border = dx == 0 || dx == 3 || dy == 0 || dy == 4;
                buf.put(px, py, if on_border { mat_color } else { mat_accent });
            }
        }
    }
}

/// Water cooler near the pantry wall (3×6: blue bottle over a light body).
/// The cooler bottle's fill — theme-independent, so every theme's
/// `tank_water_line` glug bubble must stay distinguishable from it
/// (pinned in `appliance_palette_is_legible_for_every_theme`).
pub(crate) const COOLER_WATER: Rgb = Rgb {
    r: 100,
    g: 180,
    b: 230,
};

pub(super) fn paint_water_cooler(
    buf: &mut RgbBuffer,
    pr: Bounds,
    now: std::time::SystemTime,
    theme: &crate::theme::Theme,
) {
    if !(pr.height > 25 && pr.width > 12) {
        return;
    }
    let cooler_body = theme.office.building_light;
    let cooler_water = COOLER_WATER;
    let wx = pr.x + pr.width - 6;
    let wy = pr.y + 8;
    for dy in 0..6u16 {
        for dx in 0..3u16 {
            let px = wx + dx;
            let py = wy + dy;
            if px < buf.width() && py < buf.height() {
                let color = if dy < 2 { cooler_water } else { cooler_body };
                buf.put(px, py, color);
            }
        }
    }
    // Ambient glug (B-4, owner-ratified): a lit-water bubble climbs the
    // bottle each cycle. Color reuses tank_water_line — THE lit-water
    // highlight — which also keeps it off the mascot harness's exclusive
    // bubble sentinel.
    const GLUG_CYCLE_MS: u64 = 2_000;
    const GLUG_STEP_MS: u64 = 400;
    let phase = (super::epoch_ms(now) % GLUG_CYCLE_MS) / GLUG_STEP_MS; // 0..5
    if phase < 2 {
        let (bx, by) = (wx + 1, wy + 1 - phase as u16);
        if bx < buf.width() && by < buf.height() {
            buf.put(bx, by, theme.furniture.tank_water_line);
        }
    }
}

/// Trash bin near the pantry counter (4×5 with a visible bag-liner peek). Its
/// colours are intentionally un-themed neutral greys (a semantic object, like
/// the water bottle's blue), so it takes no theme.
pub(super) fn paint_trash_bin(buf: &mut RgbBuffer, pr: Bounds) {
    if pr.height <= 20 {
        return;
    }
    let tx = pr.x + 3;
    let ty = pr.y + pr.height - 14;
    let bin_outer = Rgb {
        r: 70,
        g: 70,
        b: 78,
    };
    let bin_rim = Rgb {
        r: 100,
        g: 100,
        b: 108,
    };
    let bag_liner = Rgb {
        r: 200,
        g: 200,
        b: 210,
    };
    let bag_fill = Rgb {
        r: 160,
        g: 160,
        b: 170,
    };
    for dy in 0..5u16 {
        for dx in 0..4u16 {
            let px = tx + dx;
            let py = ty + dy;
            if px < buf.width() && py < buf.height() {
                let color = if dy == 0 {
                    // Rim row — lighter metal rim with bag liner peek
                    if dx == 0 || dx == 3 {
                        bin_rim
                    } else {
                        bag_liner
                    }
                } else if dy == 1 {
                    // Bag liner visible
                    if dx == 0 || dx == 3 {
                        bin_outer
                    } else {
                        bag_fill
                    }
                } else {
                    // Bin body
                    bin_outer
                };
                buf.put(px, py, color);
            }
        }
    }
}

/// Entry mat centered under the pantry's north doorway — the pantry-scale
/// sibling of the meeting doormat (decor-arc taste pin B1). Reuses the area-rug
/// palette so every soft-goods piece stays one family. One clear floor row
/// separates it from the wall face (offset derived from the SAME
/// `WALL_THICK_H` the wall painter is thick by, so they can't drift).
pub(super) fn paint_pantry_entry_mat(
    buf: &mut RgbBuffer,
    layout: &crate::layout::SceneLayout,
    theme: &crate::theme::Theme,
) {
    const ENTRY_MAT_W: u16 = 16;
    const ENTRY_MAT_H: u16 = 5;
    let Some(p) = layout.pantry else { return };
    let Some(dw) = layout
        .doorways
        .iter()
        .find(|d| d.start.y == d.end.y && d.start.y == p.bounds.y)
    else {
        return;
    };
    let cx = (dw.start.x + dw.end.x) / 2;
    let cy = dw.start.y + crate::layout::WALL_THICK_H + 1 + ENTRY_MAT_H / 2;
    paint_area_rug(buf, cx, cy, ENTRY_MAT_W, ENTRY_MAT_H, theme);
}

/// Thin bordered bar mat under the kitchen island (decor-arc taste pin B2):
/// the island body covers most of it, leaving a mat sliver peeking out along
/// the bar's south serving front. Painted in the background pass so the
/// island drawable and every character stack on top.
pub(super) fn paint_island_bar_mat(
    buf: &mut RgbBuffer,
    layout: &crate::layout::SceneLayout,
    theme: &crate::theme::Theme,
) {
    const BAR_MAT_W: u16 = 26;
    const BAR_MAT_H: u16 = 4;
    // The island anchor is its body center; +4 drops the mat's center to the
    // seat row so the sliver clears the body's south edge (mock-verified).
    const BAR_MAT_Y_OFF: u16 = 4;
    let Some(isl) = layout.pantry.and_then(|p| p.kitchen_island) else {
        return;
    };
    paint_area_rug(
        buf,
        isl.x,
        isl.y + BAR_MAT_Y_OFF,
        BAR_MAT_W,
        BAR_MAT_H,
        theme,
    );
}

/// Aquarium on a low cabinet (decor arc): theme water behind a shared-dark
/// frame, two fish patrolling opposite lanes on the anim clock, a rising
/// bubble and a plant sprig rooted in the gravel. Geometry derives from the
/// `FishTank` furniture row (center-anchored, matching its mask stamp); only
/// water/fish/plant are tank-specific theme fields — frame reuses
/// `room_wall_trim_dark`, cabinet + gravel the wood family.
pub(super) fn paint_fish_tank(
    buf: &mut RgbBuffer,
    pos: crate::layout::Point,
    now: std::time::SystemTime,
    theme: &crate::theme::Theme,
) {
    use crate::layout::{furniture_def, Furniture};
    let def = furniture_def(Furniture::FishTank);
    let (w, h) = (def.visual.w, def.visual.h);
    let x0 = pos.x.saturating_sub(w / 2);
    let y0 = pos.y.saturating_sub(h / 2);
    let frame = theme.office.room_wall_trim_dark;
    let fc = &theme.furniture;
    let mut put = |dx: u16, dy: u16, c: Rgb| {
        let (px, py) = (x0 + dx, y0 + dy);
        if px < buf.width() && py < buf.height() {
            buf.put(px, py, c);
        }
    };
    // Lid, glass walls, water (lit surface row under the lid), gravel bed.
    for dx in 0..w {
        put(dx, 0, frame);
        put(dx, h - 3, frame);
    }
    for dy in 1..=(h - 4) {
        put(0, dy, frame);
        put(w - 1, dy, frame);
        for dx in 1..w - 1 {
            let c = if dy == 1 {
                fc.tank_water_line
            } else if dy == h - 4 {
                if dx % 2 == 0 {
                    fc.wood_trim
                } else {
                    fc.wood_top
                }
            } else {
                fc.tank_water
            };
            put(dx, dy, c);
        }
    }
    // Cabinet: wood door face with a center seam, then the plinth shadow row.
    for dx in 0..w {
        put(
            dx,
            h - 2,
            if dx == w / 2 {
                fc.wood_trim
            } else {
                fc.wood_top
            },
        );
        put(dx, h - 1, fc.wood_trim);
    }
    // Fish patrol: a triangle wave over the interior span, one lane each,
    // opposite phases so they rarely mirror. 3px bodies read at half-block
    // scale; direction comes free from the wave's half.
    let t = super::epoch_ms(now);
    // Patrol/rise cadences: one cell per step. Distinct fish periods (and a
    // phase offset) keep the pair from mirroring in lockstep.
    const FISH_STEP_MS: u64 = 430;
    const FISH_ALT_STEP_MS: u64 = 520;
    const FISH_ALT_PHASE_STEPS: u64 = 7;
    const BUBBLE_RISE_STEP_MS: u64 = 300;
    let span = (w - 5) as u64;
    let mut fish = |lane_dy: u16, color: Rgb, step_ms: u64, phase: u64| {
        let cycle = span * 2;
        let step = ((t / step_ms) + phase) % cycle;
        let start = if step < span {
            1 + step as u16
        } else {
            1 + (cycle - step) as u16
        };
        for dx in start..start + 3 {
            put(dx, lane_dy, color);
        }
    };
    fish(3, fc.tank_fish, FISH_STEP_MS, 0);
    fish(5, fc.tank_fish_alt, FISH_ALT_STEP_MS, FISH_ALT_PHASE_STEPS);
    // One bubble rising near the east glass.
    let bubble_dy = (h - 5) - ((t / BUBBLE_RISE_STEP_MS) % (h as u64 - 6)) as u16;
    put(w - 3, bubble_dy, fc.tank_water_line);
    // Plant sprig last: fish swim behind it.
    put(2, 5, fc.tank_plant);
    put(2, 6, fc.tank_plant);
    put(2, 7, fc.tank_plant);
    put(3, 6, fc.tank_plant);
}

/// Head-of-table meeting chair (decor arc): a 7x7 cushion-and-backrest body
/// centered on its MeetingChair waypoint. The backrest bar rides the OUTER
/// side (`back_west`), reinforcing the profile sitter's orientation — and
/// carrying it alone when the chair is empty. Colors reuse the desk-chair
/// family.
pub(super) fn paint_meeting_chair(
    buf: &mut RgbBuffer,
    pos: crate::layout::Point,
    back_west: bool,
    theme: &crate::theme::Theme,
) {
    let fc = &theme.furniture;
    let chair = crate::layout::furniture_def(crate::layout::Furniture::MeetingChair).visual;
    let (x0, y0) = (
        pos.x.saturating_sub(chair.w / 2),
        pos.y.saturating_sub(chair.h / 2),
    );
    let mut put = |dx: u16, dy: u16, c: Rgb| {
        let (px, py) = (x0 + dx, y0 + dy);
        if px < buf.width() && py < buf.height() {
            buf.put(px, py, c);
        }
    };
    let back_dx = if back_west { 0 } else { chair.w - 1 };
    for dy in 0..5u16 {
        put(back_dx, dy, fc.chair_trim);
    }
    for dy in 1..5u16 {
        for dx in 1..chair.w - 1 {
            let c = if dy == 1 {
                MEETING_FABRIC_LIT
            } else {
                MEETING_FABRIC
            };
            put(dx, dy, c);
        }
    }
    // Feet on the ground row, table side + outer side.
    for dx in [1u16, chair.w - 2] {
        put(dx, 5, fc.chair_trim);
        put(dx, 6, fc.chair_trim);
    }
}

/// The meeting chairs upholster in the SAME fabric as the sofas they flank —
/// and the sofa is a SPRITE (un-themed, palette keys "C"/"G"), so the chair
/// cannot read the value from `Theme`. These are deliberate second copies of
/// the pack palette entries, pinned by
/// `meeting_chair_fabric_matches_the_sofa_sprite_palette` so a sofa retint
/// can't silently strand the chairs.
pub(super) const MEETING_FABRIC: Rgb = Rgb {
    r: 0x4f,
    g: 0x6d,
    b: 0x77,
};
pub(super) const MEETING_FABRIC_LIT: Rgb = Rgb {
    r: 0x6a,
    g: 0x8e,
    b: 0x98,
};
