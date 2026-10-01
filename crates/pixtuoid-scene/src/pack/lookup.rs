//! Which sprite a piece draws, which frame of it shows, and the palette keys a
//! theme recolours.

use pixtuoid_core::sprite::format::Pack;
use pixtuoid_core::sprite::{Frame, Pixel, Rgb, Sprite};

/// The pack key of a monitor's glass.
pub(crate) const SCREEN_GLASS_KEY: char = 'j';

/// The pack key of the dim content an idle screen shows on its glass.
pub(crate) const SCREEN_TEXT_KEY: char = 'J';

/// The pack key of a desk lamp's bulb, which glows of its own at any hour.
pub(crate) const DESK_BULB_KEY: char = '9';

/// The pack key of the wall clock's face, inside its rim.
pub(crate) const CLOCK_FACE_KEY: char = 'ц';

/// The cooler bottle's fill — theme-independent, so every theme's
/// `tank_water_line` glug bubble must stay distinguishable from it.
pub(crate) const COOLER_WATER: Rgb = Rgb {
    r: 100,
    g: 180,
    b: 230,
};

/// The fixtures' [`appliance_overrides`].
pub(crate) fn fixture_overrides(theme: &crate::theme::Theme) -> [(char, Pixel); 16] {
    let (f, o) = (&theme.furniture, &theme.office);
    let [c0, c1, c2] = theme.appliance.coats;
    [
        ('Д', Some(f.tank_water)),
        ('З', Some(f.tank_water_line)),
        ('И', Some(f.tank_fish)),
        ('Л', Some(f.tank_fish_alt)),
        ('Ь', Some(f.tank_plant)),
        ('ж', Some(o.room_wall_trim_dark)),
        ('б', Some(o.building_light)),
        ('ы', Some(f.magazine)),
        ('э', Some(f.magazine_trim)),
        ('ч', Some(c0)),
        ('ш', Some(c1)),
        ('щ', Some(c2)),
        ('ф', Some(o.clock_rim)),
        (CLOCK_FACE_KEY, Some(o.clock_face)),
        ('з', Some(o.clock_hand)),
        ('χ', Some(COOLER_WATER)),
    ]
}

/// The pack keys a corridor appliance's art is drawn in, each with the
/// [`ApplianceColors`](crate::theme::ApplianceColors) role it takes: the art
/// owns the form, the theme the palette. The pack's `[ramps]` of these keys are
/// the shading, re-derived by the recolour.
pub(crate) fn appliance_overrides(a: &crate::theme::ApplianceColors) -> [(char, Pixel); 13] {
    let [d0, d1, d2, d3] = a.vending_drinks;
    [
        ('Б', Some(a.vending_body)),
        ('П', Some(a.vending_panel)),
        ('Ч', Some(d0)),
        ('Ш', Some(d1)),
        ('Щ', Some(d2)),
        ('Э', Some(d3)),
        ('Ф', Some(a.vending_trim)),
        ('Ы', Some(a.vending_dark)),
        ('Ю', Some(a.printer_body)),
        ('Я', Some(a.printer_top)),
        ('Ё', Some(a.printer_glass)),
        ('Й', Some(a.printer_paper)),
        ('Ц', Some(a.printer_tray)),
    ]
}

/// The monitor bezel standing proud of the desk back, above `desk.y`.
pub(crate) const DESK_BEZEL_RAISE: u16 = 1;

/// The base desk's pack animation, whose bottom row every desk's art keeps.
pub(crate) const DESK_SPRITE: &str = "desk";

/// The row a desk's art `art_h` tall blits from at `desk_y`: the bezel raise,
/// plus whatever a taller art adds ABOVE `desk.y`, so it keeps the base
/// [`DESK_SPRITE`]'s bottom row. Both profiles blit desks from this.
pub(crate) fn desk_art_top(pack: &Pack, desk_y: u16, art_h: u16) -> u16 {
    let base_h = pack
        .animation(DESK_SPRITE)
        .and_then(|a| a.frames().first())
        .map_or(0, |f| f.height());
    desk_y.saturating_sub(DESK_BEZEL_RAISE + art_h.saturating_sub(base_h))
}

/// The desk art for a seat facing `facing`. Only a back-turned seat needs its
/// own — its occupant y-sorts in FRONT and covers the screen.
pub(crate) fn desk_sprite_name(facing: crate::layout::Facing) -> &'static str {
    match facing {
        crate::layout::Facing::North => "desk_north",
        crate::layout::Facing::South
        | crate::layout::Facing::East
        | crate::layout::Facing::West => DESK_SPRITE,
    }
}

/// The art a desk seating its occupant toward `facing` draws.
pub(crate) fn desk_art(pack: &Pack, facing: crate::layout::Facing) -> Option<&Frame> {
    pack.animation_or_source(desk_sprite_name(facing))
        .and_then(|a| a.frames().first())
}

/// The desk task chair's pack animation.
pub(crate) const DESK_CHAIR_SPRITE: &str = "desk_chair";

/// The meeting table's pack animation.
pub(crate) const MEETING_TABLE_SPRITE: &str = "meeting_table";

/// The frame to paint for `idx`, via [`frame_index`]. `None` only for a
/// genuinely empty animation.
pub(crate) fn frame_at(anim: &Sprite, idx: usize) -> Option<&Frame> {
    anim.frames().get(frame_index(anim, idx))
}

/// `idx`, or `0` once it runs past the animation: a custom pack's animation
/// with fewer frames than the shared cycle's `frame_idx` would
/// otherwise vanish the sprite.
pub(super) fn frame_index(anim: &Sprite, idx: usize) -> usize {
    if idx < anim.frames().len() { idx } else { 0 }
}

pub(crate) const VENDING_MACHINE_SPRITE: &str = "vending_machine";
pub(crate) const PRINTER_SPRITE: &str = "printer";

/// The pack art a corridor appliance at a `kind` waypoint is drawn from.
pub(crate) fn appliance_sprite(kind: crate::layout::WaypointKind) -> Option<&'static str> {
    use crate::layout::WaypointKind as K;
    match kind {
        K::VendingMachine => Some(VENDING_MACHINE_SPRITE),
        K::Printer => Some(PRINTER_SPRITE),
        K::Couch
        | K::Pantry
        | K::PhoneBooth
        | K::StandingDesk
        | K::MeetingSofa
        | K::MeetingChair
        | K::Island
        | K::SnackShelf => None,
    }
}

/// The frame of an appliance's `anim` showing on `beat`: frame 0 when idle,
/// else its busy loop — the frames after 0, one each of the art's own
/// `frame_ms`.
pub(crate) fn appliance_frame_index(anim: &Sprite, busy: bool, beat: crate::anim::Beat) -> usize {
    let loop_len = anim.frames().len().saturating_sub(1);
    if !busy || loop_len == 0 {
        return 0;
    }
    let step = beat.ms() / u64::from(anim.frame_ms().max(1));
    1 + usize::try_from(step % loop_len as u64).unwrap_or(0)
}

/// The frame of a looping `anim` showing on `beat`: one each of the art's own
/// `frame_ms`, round and round.
pub(crate) fn looping_frame_index(anim: &Sprite, beat: crate::anim::Beat) -> usize {
    let frames = anim.frames().len().max(1) as u64;
    let step = beat.ms() / u64::from(anim.frame_ms().max(1));
    usize::try_from(step % frames).unwrap_or(0)
}

/// The frame of `pack`'s looping animation `name` showing on `beat`; 0 for one
/// the pack lacks.
pub(crate) fn animation_frame_at(pack: &Pack, name: &str, beat: crate::anim::Beat) -> usize {
    pack.animation(name)
        .map_or(0, |anim| looping_frame_index(anim, beat))
}
