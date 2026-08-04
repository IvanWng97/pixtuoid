//! Standalone furniture paint helpers — the room and corridor pieces the pixel
//! painter stamps (tables, rugs, appliances, and the procedural room-fill decor).

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::layout::Bounds;

/// Low meeting-room table between the sofas.
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

/// Bordered area rug centred on `cx,cy` — the meeting rug and both pantry mats.
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

/// Lounge side table next to the viewing couch, opposite the floor lamp, with a
/// magazine stack on top so the silhouette reads as "side table with a book".
pub(super) fn paint_side_table(buf: &mut RgbBuffer, cx: u16, cy: u16, theme: &crate::theme::Theme) {
    let top = theme.furniture.wood_top;
    let trim = theme.furniture.wood_trim;
    let mag = theme.furniture.magazine;
    let mag_trim = theme.furniture.magazine_trim;
    // Dims come from the one furniture table so the painted block can't drift
    // from the blocked ground.
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

/// Kitchen island — the pantry's counter-height centre piece; ALL dims read from
/// the FurnitureDef row. The mask blocks only the south-anchored base
/// (footprint.h = visual.h − 2, invariant #6).
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
            // Rows 2+ inset 1px per side so the countertop reads as overhanging.
            if dy >= 2 && (dx == 0 || dx == w - 1) {
                continue;
            }
            let color = if dy < 2 {
                top
            } else if dy == h - 1 {
                shade
            } else {
                body
            };
            buf.put(px as u16, py as u16, color);
        }
    }
    // Front detail: door seams + handles so the body reads as cabinetry, not a slab.
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
    putxy(buf, 3, 0, accents[0]);
    putxy(buf, 4, 0, accents[1]);
    // One mug — a THIRD accent so it can't blend into the fruit pair (the
    // vending panel color is theme-dependent and collided in default).
    putxy(buf, w - 5, 0, accents[2]);
}

/// Notice board on the meeting room's south wall.
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

/// Small doormat at the meeting-room entrance. Placement + fit-gate come from
/// [`MeetingRoom::doormat_rect`] — the ONE authority the hover hit-test shares.
pub(super) fn paint_doormat(
    buf: &mut RgbBuffer,
    room: &crate::layout::MeetingRoom,
    theme: &crate::theme::Theme,
) {
    let Some(mat) = room.doormat_rect() else {
        return;
    };
    let mat_color = theme.furniture.rug_trim;
    let mat_accent = theme.furniture.rug_field;
    for dy in 0..mat.height {
        for dx in 0..mat.width {
            let px = mat.x + dx;
            let py = mat.y + dy;
            if px < buf.width() && py < buf.height() {
                let on_border = dx == 0 || dx == mat.width - 1 || dy == 0 || dy == mat.height - 1;
                buf.put(px, py, if on_border { mat_color } else { mat_accent });
            }
        }
    }
}

/// The cooler bottle's fill — theme-independent, so every theme's
/// `tank_water_line` glug bubble must stay distinguishable from it.
pub(crate) const COOLER_WATER: Rgb = Rgb {
    r: 100,
    g: 180,
    b: 230,
};

/// Water cooler; placement + fit-gate come from [`PantryRoom::water_cooler_rect`]
/// — the ONE authority the hover hit-test shares.
pub(super) fn paint_water_cooler(
    buf: &mut RgbBuffer,
    room: &crate::layout::PantryRoom,
    now: std::time::SystemTime,
    theme: &crate::theme::Theme,
) {
    let Some(cooler) = room.water_cooler_rect() else {
        return;
    };
    let cooler_body = theme.office.building_light;
    let cooler_water = COOLER_WATER;
    let (wx, wy) = (cooler.x, cooler.y);
    for dy in 0..cooler.height {
        for dx in 0..cooler.width {
            let px = wx + dx;
            let py = wy + dy;
            if px < buf.width() && py < buf.height() {
                let color = if dy < 2 { cooler_water } else { cooler_body };
                buf.put(px, py, color);
            }
        }
    }
    // Ambient glug: a bubble climbs the bottle each cycle. Reusing
    // tank_water_line also keeps it off the mascot harness's bubble sentinel.
    const GLUG_CYCLE_MS: u64 = 2_000;
    const GLUG_STEP_MS: u64 = 400;
    let phase = (super::epoch_ms(now) % GLUG_CYCLE_MS) / GLUG_STEP_MS;
    if phase < 2 {
        let (bx, by) = (wx + 1, wy + 1 - phase as u16);
        if bx < buf.width() && by < buf.height() {
            buf.put(bx, by, theme.furniture.tank_water_line);
        }
    }
}

/// Trash bin near the pantry counter. Its colours are intentionally un-themed
/// neutral greys (a semantic object, like the water bottle's blue), so it takes no
/// theme; placement + fit-gate come from [`PantryRoom::trash_bin_rect`] — the ONE
/// authority the hover hit-test shares.
pub(super) fn paint_trash_bin(buf: &mut RgbBuffer, room: &crate::layout::PantryRoom) {
    let Some(bin) = room.trash_bin_rect() else {
        return;
    };
    let (tx, ty) = (bin.x, bin.y);
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
    // Loop bounds derive from the rect so the painted box can't drift from the
    // hover box.
    for dy in 0..bin.height {
        for dx in 0..bin.width {
            let px = tx + dx;
            let py = ty + dy;
            if px < buf.width() && py < buf.height() {
                let on_edge = dx == 0 || dx == bin.width - 1;
                let color = if dy == 0 {
                    // Rim row.
                    if on_edge {
                        bin_rim
                    } else {
                        bag_liner
                    }
                } else if dy == 1 {
                    // Bag-liner peek.
                    if on_edge {
                        bin_outer
                    } else {
                        bag_fill
                    }
                } else {
                    // Body.
                    bin_outer
                };
                buf.put(px, py, color);
            }
        }
    }
}

/// Entry mat centered under the pantry's north doorway. One clear floor row
/// separates it from the wall face — the offset derives from the SAME
/// `WALL_THICK_H` the wall painter is thick by, so they can't drift.
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

/// Thin bar mat under the kitchen island: the island body covers most of it,
/// leaving a sliver peeking out along the bar's south serving front.
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

/// Aquarium on a low cabinet: theme water behind a shared-dark frame, two fish
/// patrolling opposite lanes on the anim clock, a rising bubble and a plant sprig.
/// Geometry derives from the `FishTank` furniture row, matching its mask stamp.
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
    // Fish patrol: a triangle wave over the interior span, one lane each. Distinct
    // periods (and a phase offset) keep the pair from mirroring in lockstep.
    let t = super::epoch_ms(now);
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
    let bubble_dy = (h - 5) - ((t / BUBBLE_RISE_STEP_MS) % (h as u64 - 6)) as u16;
    put(w - 3, bubble_dy, fc.tank_water_line);
    // Plant sprig last so the fish swim behind it.
    put(2, 5, fc.tank_plant);
    put(2, 6, fc.tank_plant);
    put(2, 7, fc.tank_plant);
    put(3, 6, fc.tank_plant);
}

/// Head-of-table meeting chair centered on its MeetingChair waypoint. The
/// backrest bar rides the OUTER side (`back_west`), so it carries the sitter's
/// orientation even when the chair is empty.
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
    for dx in [1u16, chair.w - 2] {
        put(dx, 5, fc.chair_trim);
        put(dx, 6, fc.chair_trim);
    }
}

/// The meeting chairs upholster in the SAME fabric as the sofas they flank — and
/// the sofa is a SPRITE (un-themed), so the chair cannot read the value from
/// `Theme`. Deliberate second copies of the pack palette entries, pinned by
/// `meeting_chair_fabric_matches_the_sofa_sprite_palette`.
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

/// Vending machine centred at `pos` — drinks panel, a grid of themed cans, and
/// the pickup slot. When `busy`, a product cell darkens and its can lands in the
/// slot each cycle. The slot cell reuses `VENDING_PICKUP_SLOT`, the one authority
/// the pixel test also derives from.
pub(super) fn paint_vending_machine(
    buf: &mut RgbBuffer,
    pos: crate::layout::Point,
    busy: bool,
    now: std::time::SystemTime,
    theme: &crate::theme::Theme,
) {
    let body = theme.appliance.vending_body;
    let panel = theme.appliance.vending_panel;
    let drinks = theme.appliance.vending_drinks;
    let vend = super::drawable::VENDING_BODY;
    let vx = pos.x.saturating_sub(vend.w / 2);
    let vy = pos.y.saturating_sub(vend.h / 2);
    for dy in 0..vend.h {
        for dx in 0..vend.w {
            let px = vx + dx;
            let py = vy + dy;
            if px < buf.width() && py < buf.height() {
                let color = if dy == 0 {
                    panel
                } else if (1..=3).contains(&dy) && (1..=2).contains(&dx) {
                    let idx = ((dy - 1) * 2 + (dx - 1)) as usize;
                    if idx < drinks.len() {
                        drinks[idx]
                    } else {
                        body
                    }
                } else if (dx, dy) == super::drawable::VENDING_PICKUP_SLOT {
                    theme.appliance.vending_trim
                } else if dy == 5 {
                    theme.appliance.vending_dark
                } else {
                    body
                };
                buf.put(px, py, color);
            }
        }
    }

    if busy {
        // A product cell goes dark and its can lands in the pickup slot; the
        // product rotates per cycle.
        const DROP_CYCLE_MS: u64 = 3_000;
        const DROP_STEP_MS: u64 = 500;
        let t = super::epoch_ms(now);
        let phase = (t % DROP_CYCLE_MS) / DROP_STEP_MS;
        let pick = ((t / DROP_CYCLE_MS) % drinks.len() as u64) as u16;
        let (ddx, ddy) = (1 + pick % 2, 1 + pick / 2);
        let mut put = |x: u16, y: u16, c| {
            if x < buf.width() && y < buf.height() {
                buf.put(x, y, c);
            }
        };
        if (1..=4).contains(&phase) {
            put(vx + ddx, vy + ddy, theme.appliance.vending_dark);
        }
        if (2..=4).contains(&phase) {
            let can = drinks[(pick as usize) % drinks.len()];
            put(
                vx + super::drawable::VENDING_PICKUP_SLOT.0,
                vy + super::drawable::VENDING_PICKUP_SLOT.1,
                can,
            );
        }
    }
}

/// Printer centred at `pos` — dark lid with a glass strip, white chassis, and an
/// output tray. When `busy`, a page slides out below the tray, rests, then clears.
pub(super) fn paint_printer(
    buf: &mut RgbBuffer,
    pos: crate::layout::Point,
    busy: bool,
    now: std::time::SystemTime,
    theme: &crate::theme::Theme,
) {
    let body_white = theme.appliance.printer_body;
    let top_dark = theme.appliance.printer_top;
    let glass = theme.appliance.printer_glass;
    let paper = theme.appliance.printer_paper;
    let tray = theme.appliance.printer_tray;
    let pbody = super::drawable::PRINTER_BODY;
    let px0 = pos.x.saturating_sub(pbody.w / 2);
    let py0 = pos.y.saturating_sub(pbody.h / 2);
    for dy in 0..pbody.h {
        for dx in 0..pbody.w {
            let px = px0 + dx;
            let py = py0 + dy;
            if px < buf.width() && py < buf.height() {
                let color = if dy == 0 {
                    if (1..=3).contains(&dx) {
                        glass
                    } else {
                        top_dark
                    }
                } else if dy == 3 {
                    if (1..=3).contains(&dx) {
                        paper
                    } else {
                        tray
                    }
                } else if dx == 0 || dx == 4 {
                    tray
                } else {
                    body_white
                };
                buf.put(px, py, color);
            }
        }
    }

    if busy {
        const PAGE_CYCLE_MS: u64 = 2_400;
        const PAGE_STEP_MS: u64 = 300;
        let phase = (super::epoch_ms(now) % PAGE_CYCLE_MS) / PAGE_STEP_MS;
        let rows = match phase {
            1 => 1,
            2..=6 => 2,
            _ => 0,
        };
        for dy in 0..rows {
            for dx in 1..=3u16 {
                let px = px0 + dx;
                let py = py0 + 4 + dy;
                if px < buf.width() && py < buf.height() {
                    buf.put(px, py, paper);
                }
            }
        }
    }
}

/// Meeting-room coat rack centred on `pos`, the pole top.
pub(super) fn paint_coat_rack(
    buf: &mut RgbBuffer,
    pos: crate::layout::Point,
    theme: &crate::theme::Theme,
) {
    let (cx, cy) = (pos.x, pos.y);
    let pole = theme.furniture.wood_trim;
    let base = theme.furniture.wood_top;
    let coats = theme.appliance.coats;
    for dy in 0..8u16 {
        let py = cy + dy;
        if py < buf.height() && cx < buf.width() {
            buf.put(cx, py, pole);
        }
    }
    let by = cy + 7;
    for dx in 0..3u16 {
        let px = cx.saturating_sub(1) + dx;
        if px < buf.width() && by < buf.height() {
            buf.put(px, by, base);
        }
    }
    // Coat blobs on alternating hooks.
    for (i, &coat_color) in coats.iter().enumerate() {
        let hook_y = cy + 1 + (i as u16) * 2;
        let side: i16 = if i % 2 == 0 { -1 } else { 1 };
        let hx = (cx as i16 + side) as u16;
        for dy in 0..2u16 {
            for dx in 0..2u16 {
                let px = hx.wrapping_add(if side < 0 { dx.wrapping_sub(1) } else { dx });
                let py = hook_y + dy;
                if px < buf.width() && py < buf.height() {
                    buf.put(px, py, coat_color);
                }
            }
        }
    }
}
