//! The cutaway's time of day: the room darkens with the sky, and its own lights
//! ([`crate::lighting`]) lift what they fall on, in whole [`Rgb::ramp`] steps
//! on the art grid ([`crate::dither::step`]), and its colour is a tint at
//! fixed stops, so the room stays a palette: nothing blends continuously, the rule the rest of the cutaway is
//! drawn by.
//!
//! The pieces are painted as by day; one pass ([`net_pass`]) then takes each
//! pixel `lift − ambient` steps along its ramp in a single step, so a pixel is
//! never darkened and relit (the ramp does not invert) nor darkened twice.

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::cutaway::order::Span;
use crate::cutaway::pen::{ArtPx, ArtRect, Pen};
use crate::lighting::{Emitter, EmitterKind};
use crate::theme::Theme;

/// Ramp steps the room drops by at full dark.
const AMBIENT_MAX_STEPS: u8 = 4;

/// Ramp steps a light lifts per unit of its level: a desk lamp at full dark then
/// lifts the desk under its bulb most of the way back to its daylight tone, so
/// the night room reads as lamp-lit pools in the dark, not as day with stains.
const LIFT_STOPS_PER_LEVEL: f32 = 7.0;

/// The most a light lifts the daylit room: one step shows a lamp is on at noon,
/// and a second would bleach the floor under it.
const DAYLIGHT_LIFT: u8 = 1;

/// The share of the way to its light's colour a pixel is tinted per step of
/// lift: the brightest cells take the most colour, as they would.
const TINT_PER_STEP: f32 = 0.08;
/// The deepest tint, in steps of [`TINT_PER_STEP`]: past it a lit pixel reads
/// as painted in the light's colour rather than lit by it.
const TINT_MAX_STEPS: u8 = 3;

/// How dark the room is: the sky's darkness in whole steps, so a frame's tone
/// changes a handful of times a day rather than every frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct Ambient(u8);

impl Ambient {
    /// The room under `look`.
    pub(crate) fn of(look: &crate::atmosphere::Look) -> Self {
        let steps = (look.darkness.clamp(0.0, 1.0) * f32::from(AMBIENT_MAX_STEPS)).round();
        Self(steps as u8)
    }

    /// The darkness it stands for, stepped as the room is, so what else the
    /// hour darkens (the shadows) moves with the room and not between its steps.
    pub(crate) fn darkness(self) -> f32 {
        f32::from(self.0) / f32::from(AMBIENT_MAX_STEPS)
    }

    /// `c` in a room this dark, unlit.
    pub(crate) fn on(self, c: Rgb) -> Rgb {
        c.ramp(-(self.0 as i8))
    }

    /// The most a light may lift a pixel in a room this dark: back to its
    /// daylight tone and never past it, or [`DAYLIGHT_LIFT`] by day.
    fn ceiling(self) -> u8 {
        if self.0 == 0 { DAYLIGHT_LIFT } else { self.0 }
    }
}

/// How a painted pixel takes the room's light.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Glow {
    /// Darkened with the room, lifted by its lights: most of it.
    Lit,
    /// Its own light, as painted: the sky in a window, a screen that glows, a
    /// bulb.
    Emissive,
    /// Darkened with the room but never lit: a dark screen's glass, which a
    /// lamp would show only as a reflection.
    Shaded,
    /// Its own light, which the room's lights still lift: a window's sky.
    Pane,
}

/// Each buffer pixel's [`Glow`], set by the last piece that painted it, so a
/// piece in front of a screen takes the room's light over it.
pub(crate) struct Emission {
    w: u16,
    glow: Vec<Glow>,
}

impl Emission {
    /// A `w`×`h` buffer's, every pixel [`Glow::Lit`].
    pub(crate) fn new(w: u16, h: u16) -> Self {
        Self {
            w,
            glow: vec![Glow::Lit; usize::from(w) * usize::from(h)],
        }
    }

    pub(crate) fn set(&mut self, x: u16, y: u16, glow: Glow) {
        if x < self.w {
            if let Some(g) = self
                .glow
                .get_mut(usize::from(y) * usize::from(self.w) + usize::from(x))
            {
                *g = glow;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn get(&self, x: u16, y: u16) -> Glow {
        if x >= self.w {
            return Glow::Lit;
        }
        self.glow
            .get(usize::from(y) * usize::from(self.w) + usize::from(x))
            .copied()
            .unwrap_or(Glow::Lit)
    }
}

/// One light as the cutaway paints it, resolved when the list is built: how many
/// steps it lifts each art pixel of its box, row by row from the top-left, the
/// colour it tints them toward, and the room it lights, which bounds the lift.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct LightView {
    x: u16,
    y: u16,
    w: u16,
    lift: Vec<u8>,
    tint: Option<Rgb>,
    ambient: Ambient,
    /// Where two lights lift a pixel alike, the lower rank lights it: a total
    /// order on what the light is — its kind, where it stands and its tint —
    /// so the list's order cannot decide.
    rank: (u8, u16, u16, (u8, u8, u8)),
}

impl LightView {
    /// `emitter` on `pen`'s grid under `ambient`, clipped to a `buf_w`×`buf_h`
    /// office, with its [`Span`]; `None` where it lifts no pixel a whole step,
    /// since a light with no solid band is only its seam's speckle.
    pub(crate) fn of(
        emitter: &Emitter,
        tint: Option<Rgb>,
        ambient: Ambient,
        pen: Pen,
        (buf_w, buf_h): (u16, u16),
    ) -> Option<(Span, Self)> {
        let ((x0, y0), (x1, y1)) = emitter.bounds();
        let (x1, y1) = (x1.min(buf_w), y1.min(buf_h));
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let (ax0, ay0) = (pen.art(x0).0, pen.art(y0).0);
        let (ax1, ay1) = (pen.art(x1).0, pen.art(y1).0);
        let d = f32::from(pen.art(1).0);
        // An art pixel's centre, in layout cells: at one art pixel a cell, the
        // cell itself, as the classic samples it.
        let at = |a: u16| (f32::from(a) + 0.5) / d - 0.5;
        let mut solid = false;
        let lift: Vec<u8> = (ay0..ay1)
            .flat_map(|ay| (ax0..ax1).map(move |ax| (ax, ay)))
            .map(|(ax, ay)| {
                let Some(level) = emitter.level_at_f(at(ax), at(ay)) else {
                    return 0;
                };
                let stops = level * LIFT_STOPS_PER_LEVEL;
                solid |= stops >= 1.0;
                // Floored, not rounded: the lift constants and the `solid` test
                // are tuned in whole steps.
                crate::dither::step(stops, ax, ay).min(ambient.ceiling())
            })
            .collect();
        solid.then(|| {
            (
                Span::new(x0, y0, x1 - x0, y1 - y0, 0),
                Self {
                    x: ax0,
                    y: ay0,
                    w: ax1 - ax0,
                    lift,
                    tint,
                    ambient,
                    rank: (
                        rank_of(emitter.kind),
                        x0,
                        y0,
                        tint.map_or((0, 0, 0), |t| (t.r, t.g, t.b)),
                    ),
                },
            )
        })
    }

    /// Its box on the art grid.
    pub(crate) fn rect(&self) -> ArtRect {
        let h = (self.lift.len() / usize::from(self.w.max(1))) as u16;
        ArtRect {
            x: ArtPx(self.x),
            y: ArtPx(self.y),
            w: ArtPx(self.w),
            h: ArtPx(h),
        }
    }

    /// Whether it is a `kind`'s light.
    #[cfg(test)]
    pub(crate) fn is(&self, kind: EmitterKind) -> bool {
        self.rank.0 == rank_of(kind)
    }

    /// Where it lifts most, in art pixels: the middle of its brightest.
    #[cfg(test)]
    pub(crate) fn peak(&self) -> (f32, f32) {
        let top = self.lift.iter().copied().max().unwrap_or(0);
        let w = usize::from(self.w.max(1));
        let (mut n, mut sx, mut sy) = (0.0, 0.0, 0.0);
        for (i, _) in self.lift.iter().enumerate().filter(|(_, &l)| l == top) {
            n += 1.0;
            sx += (i % w) as f32;
            sy += (i / w) as f32;
        }
        (f32::from(self.x) + sx / n, f32::from(self.y) + sy / n)
    }

    /// Its lift at art pixel `(x, y)`, 0 outside its box.
    pub(crate) fn lift_at(&self, x: u16, y: u16) -> u8 {
        let (dx, dy) = (x.wrapping_sub(self.x), y.wrapping_sub(self.y));
        if dx >= self.w {
            return 0;
        }
        self.lift
            .get(usize::from(dy) * usize::from(self.w) + usize::from(dx))
            .copied()
            .unwrap_or(0)
    }
}

/// An emitter kind's place in the light order ([`LightView::rank`]).
fn rank_of(kind: EmitterKind) -> u8 {
    match kind {
        EmitterKind::WindowSpill => 0,
        EmitterKind::FloorLamp => 1,
        EmitterKind::DeskLamp => 2,
        EmitterKind::MonitorHalo(_) => 3,
        EmitterKind::NeonGlow => 4,
    }
}

/// The colour a light of `kind` tints toward under `theme`, or `None` for one
/// the cutaway leaves untinted. A monitor's tool light tints only a dark
/// theme's room, as in the classic's `pixel_painter::ambient::paint_ceiling_halos`:
/// on a light one it reads as grime.
pub(crate) fn tint_of(kind: EmitterKind, theme: &Theme) -> Option<Rgb> {
    let lighting = &theme.lighting;
    match kind {
        EmitterKind::FloorLamp => Some(lighting.floor_lamp_halo),
        EmitterKind::DeskLamp => Some(lighting.desk_lamp),
        EmitterKind::WindowSpill => Some(lighting.sun_spill),
        EmitterKind::NeonGlow => Some(theme.ui.neon_brand),
        EmitterKind::MonitorHalo(tool) => (theme.kind == crate::theme::ThemeKind::Dark)
            .then(|| crate::pixel_painter::tool_glow_for_kind(tool, &theme.tool_glow)),
    }
}

/// Light the art pixels of `rect`, clipped to the buffer: each pixel takes
/// `lift − ambient` steps along its ramp at once, where its lift is the most
/// any of `lights` gives it, as its [`Glow`] in `emission` allows. A rect
/// repainted alone comes out as the whole frame does there, so long as
/// `lights` holds every light that meets it.
pub(crate) fn net_pass(
    rect: ArtRect,
    lights: &[&LightView],
    ambient: Ambient,
    emission: &Emission,
    pen: Pen,
    memo: &mut NetMemo,
    buf: &mut RgbBuffer,
) {
    let mut lights: Vec<&LightView> = lights
        .iter()
        .copied()
        .filter(|l| overlaps(l.rect(), rect))
        .collect();
    lights.sort_by_key(|l| l.rank);
    let (w, h) = (usize::from(rect.w.0), usize::from(rect.h.0));
    // The first of the brightest, in rank order.
    let mut lift = vec![0u8; w * h];
    let mut tint: Vec<Option<Rgb>> = vec![None; w * h];
    for l in &lights {
        let r = l.rect();
        let (x0, x1) = (
            r.x.0.max(rect.x.0),
            r.x.0
                .saturating_add(r.w.0)
                .min(rect.x.0.saturating_add(rect.w.0)),
        );
        let (y0, y1) = (
            r.y.0.max(rect.y.0),
            r.y.0
                .saturating_add(r.h.0)
                .min(rect.y.0.saturating_add(rect.h.0)),
        );
        for ay in y0..y1 {
            for ax in x0..x1 {
                let i = usize::from(ay - rect.y.0) * w + usize::from(ax - rect.x.0);
                let here = l.lift_at(ax, ay);
                if here > lift[i] {
                    lift[i] = here;
                    tint[i] = l.tint;
                }
            }
        }
    }
    let k = pen.buffer(ArtPx(1));
    let (bx0, by0) = (pen.buffer(rect.x), pen.buffer(rect.y));
    let bx1 = bx0.saturating_add(pen.buffer(rect.w)).min(buf.width());
    let by1 = by0.saturating_add(pen.buffer(rect.h)).min(buf.height());
    let bw = usize::from(buf.width());
    let pixels = buf.as_mut_slice();
    debug_assert_eq!(emission.glow.len(), pixels.len(), "one class per pixel");
    for by in by0..by1 {
        let art_row = usize::from(by / k - rect.y.0) * w;
        for bx in bx0..bx1 {
            let a = art_row + usize::from(bx / k - rect.x.0);
            let (lift, tint) = (lift[a], tint[a]);
            if lift == 0 && ambient.0 == 0 {
                continue;
            }
            let i = usize::from(by) * bw + usize::from(bx);
            let glow = emission.glow.get(i).copied().unwrap_or(Glow::Lit);
            pixels[i] = memo.of(pixels[i], glow, lift, tint, ambient);
        }
    }
}

/// [`net_pass`]'s colours, kept across frames: OKLab maths a room asks again
/// every frame.
#[derive(Default)]
pub(crate) struct NetMemo {
    colours: std::collections::HashMap<NetKey, Rgb, std::hash::BuildHasherDefault<SplitMix>>,
    /// Neighbouring pixels mostly ask the last question again.
    last: Option<(NetKey, Rgb)>,
}

/// Everything [`net_colour`] reads for one pixel.
type NetKey = (Rgb, Glow, u8, Option<Rgb>, Ambient);

/// Bounds the memo in a room whose colours never settle.
const NET_MEMO_CAP: usize = 1 << 16;

impl NetMemo {
    fn of(&mut self, under: Rgb, glow: Glow, lift: u8, tint: Option<Rgb>, ambient: Ambient) -> Rgb {
        let key = (under, glow, lift, tint, ambient);
        if let Some((k, c)) = self.last {
            if k == key {
                return c;
            }
        }
        if self.colours.len() >= NET_MEMO_CAP {
            self.colours.clear();
        }
        let c = *self.colours.entry(key).or_insert_with(|| match glow {
            Glow::Lit => net_colour(under, lift, ambient, tint),
            Glow::Emissive => under,
            Glow::Shaded => ambient.on(under),
            Glow::Pane => net_colour(under, lift, Ambient::default(), tint),
        });
        self.last = Some((key, c));
        c
    }
}

/// A small fixed key needs mixing, not SipHash.
#[derive(Default)]
pub(crate) struct SplitMix(u64);

impl std::hash::Hasher for SplitMix {
    fn finish(&self) -> u64 {
        pixtuoid_core::id::splitmix64(self.0)
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(b);
        }
    }
}

/// `c` in a room `ambient` dark, lifted `lift` steps: `lift − ambient` steps
/// along its ramp in one, then tinted toward `tint` a fixed stop per step of
/// lift.
fn net_colour(c: Rgb, lift: u8, ambient: Ambient, tint: Option<Rgb>) -> Rgb {
    let net = lift.min(ambient.ceiling()) as i8 - ambient.0 as i8;
    let stepped = c.ramp(net);
    match tint {
        Some(t) if lift > 0 => stepped.mix(t, f32::from(lift.min(TINT_MAX_STEPS)) * TINT_PER_STEP),
        _ => stepped,
    }
}

fn overlaps(a: ArtRect, b: ArtRect) -> bool {
    a.x.0 < b.x.0.saturating_add(b.w.0)
        && b.x.0 < a.x.0.saturating_add(a.w.0)
        && a.y.0 < b.y.0.saturating_add(b.h.0)
        && b.y.0 < a.y.0.saturating_add(a.h.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Point;
    use crate::lighting::Light;
    use crate::render_scale::RenderScale;

    const FLOOR: Rgb = Rgb {
        r: 110,
        g: 96,
        b: 84,
    };
    const WARM: Rgb = Rgb {
        r: 255,
        g: 214,
        b: 150,
    };

    fn lamp(strength: f32, at: Point) -> Emitter {
        Emitter {
            kind: EmitterKind::DeskLamp,
            light: Light::Halo {
                centre: at,
                radius: 5,
                share: 1.0,
            },
            strength,
        }
    }

    fn pen() -> Pen {
        Pen::new(RenderScale::new(4).expect("nonzero"), 4).expect("4 divides 4")
    }

    fn view(e: &Emitter, tint: Option<Rgb>, ambient: Ambient) -> LightView {
        LightView::of(e, tint, ambient, pen(), (40, 16))
            .expect("it lifts")
            .1
    }

    fn whole(w: u16, h: u16) -> ArtRect {
        ArtRect {
            x: ArtPx(0),
            y: ArtPx(0),
            w: pen().art(w),
            h: pen().art(h),
        }
    }

    fn lit_floor(views: &[&LightView], ambient: Ambient) -> RgbBuffer {
        let mut buf = RgbBuffer::filled(160, 64, FLOOR);
        net_pass(
            whole(40, 16),
            views,
            ambient,
            &Emission::new(160, 64),
            pen(),
            &mut NetMemo::default(),
            &mut buf,
        );
        buf
    }

    #[test]
    fn a_lit_lamp_lifts_its_pool_at_night_and_nothing_past_it() {
        let night = Ambient(AMBIENT_MAX_STEPS);
        let lamp = view(&lamp(0.6, Point { x: 10, y: 8 }), Some(WARM), night);
        let buf = lit_floor(&[&lamp], night);
        let luma = |c: Rgb| u32::from(c.r) + u32::from(c.g) + u32::from(c.b);
        assert!(luma(buf.get(10 * 4 + 1, 8 * 4 + 1)) > luma(night.on(FLOOR)));
        assert_eq!(buf.get(0, 0), night.on(FLOOR));
    }

    /// A lit pixel is taken `lift − ambient` steps in one: never darkened, then
    /// relit, which the ramp would not undo.
    #[test]
    fn a_lit_pixel_steps_once_by_its_net() {
        let night = Ambient(AMBIENT_MAX_STEPS);
        let lamp = view(&lamp(0.6, Point { x: 10, y: 8 }), None, night);
        let buf = lit_floor(&[&lamp], night);
        let mut checked = 0;
        for ay in 0..64u16 {
            for ax in 0..160u16 {
                let lift = lamp.lift_at(ax, ay);
                let want = FLOOR.ramp(lift as i8 - night.0 as i8);
                assert_eq!(buf.get(ax, ay), want, "({ax}, {ay}) lifted {lift}");
                checked += usize::from(lift > 0);
            }
        }
        assert!(checked > 0, "the lamp lit nothing");
    }

    /// Nothing blends: a lit pixel is its colour some whole steps along the ramp,
    /// tinted at that many fixed stops, and nothing between.
    #[test]
    fn a_light_paints_only_whole_steps_and_tint_stops() {
        let night = Ambient(AMBIENT_MAX_STEPS);
        let lamp = view(&lamp(0.6, Point { x: 10, y: 8 }), Some(WARM), night);
        let buf = lit_floor(&[&lamp], night);
        let allowed: std::collections::HashSet<Rgb> = (0..=AMBIENT_MAX_STEPS)
            .map(|l| net_colour(FLOOR, l, night, Some(WARM)))
            .collect();
        let seen: std::collections::HashSet<Rgb> = buf.as_slice().iter().copied().collect();
        assert!(seen.len() > 2, "a pool steps through tones: {seen:?}");
        assert!(
            seen.is_subset(&allowed),
            "off-stop colours: {:?}",
            seen.difference(&allowed)
        );
    }

    /// A light takes the night room back to its daylight tone at most, and the
    /// daylit room [`DAYLIGHT_LIFT`] past it.
    #[test]
    fn a_light_lifts_the_night_room_to_daylight_and_the_day_room_a_step() {
        let night = Ambient(AMBIENT_MAX_STEPS);
        let bright = lamp(1.0, Point { x: 10, y: 8 });
        let at_night = view(&bright, None, night);
        assert_eq!(at_night.lift.iter().max(), Some(&night.0));
        let by_day = view(&bright, None, Ambient(0));
        assert_eq!(by_day.lift.iter().max(), Some(&DAYLIGHT_LIFT));
    }

    /// A light too faint for a solid band is left out: all it would show is its
    /// seam's speckle.
    #[test]
    fn a_light_with_no_solid_band_is_left_out() {
        let faint = lamp(0.1, Point { x: 10, y: 8 });
        assert!(LightView::of(&faint, None, Ambient(0), pen(), (40, 16)).is_none());
    }

    /// Where two lights lift a pixel alike, the same one lights it whatever order
    /// they come in.
    #[test]
    fn overlapping_lights_resolve_alike_in_any_order() {
        let night = Ambient(AMBIENT_MAX_STEPS);
        let (a, b) = (
            view(&lamp(0.8, Point { x: 10, y: 8 }), Some(WARM), night),
            view(
                &lamp(0.8, Point { x: 14, y: 8 }),
                Some(Rgb {
                    r: 120,
                    g: 160,
                    b: 255,
                }),
                night,
            ),
        );
        assert_eq!(
            lit_floor(&[&a, &b], night).as_slice(),
            lit_floor(&[&b, &a], night).as_slice()
        );
    }

    /// A rect repainted alone comes out as the whole frame does there, so the
    /// canvas may repaint only what changed.
    #[test]
    fn a_rect_relit_alone_matches_the_whole_frame() {
        let night = Ambient(AMBIENT_MAX_STEPS);
        let (a, b) = (
            view(&lamp(0.8, Point { x: 10, y: 8 }), Some(WARM), night),
            view(&lamp(0.7, Point { x: 16, y: 9 }), None, night),
        );
        let full = lit_floor(&[&a, &b], night);
        let rect = ArtRect {
            x: ArtPx(40),
            y: ArtPx(20),
            w: ArtPx(30),
            h: ArtPx(24),
        };
        let mut part = RgbBuffer::filled(160, 64, FLOOR);
        net_pass(
            rect,
            &[&a, &b],
            night,
            &Emission::new(160, 64),
            pen(),
            &mut NetMemo::default(),
            &mut part,
        );
        for y in rect.y.0..rect.y.0 + rect.h.0 {
            for x in rect.x.0..rect.x.0 + rect.w.0 {
                assert_eq!(part.get(x, y), full.get(x, y), "({x}, {y})");
            }
        }
    }

    /// An emissive pixel keeps its colour and a shaded one only darkens, under
    /// the brightest light.
    #[test]
    fn a_glowing_pixel_is_its_own_light() {
        let night = Ambient(AMBIENT_MAX_STEPS);
        let lamp = view(&lamp(1.0, Point { x: 10, y: 8 }), Some(WARM), night);
        let mut emission = Emission::new(160, 64);
        emission.set(41, 33, Glow::Emissive);
        emission.set(42, 33, Glow::Shaded);
        let mut buf = RgbBuffer::filled(160, 64, FLOOR);
        net_pass(
            whole(40, 16),
            &[&lamp],
            night,
            &emission,
            pen(),
            &mut NetMemo::default(),
            &mut buf,
        );
        assert_eq!(buf.get(41, 33), FLOOR);
        assert_eq!(buf.get(42, 33), night.on(FLOOR));
        assert_ne!(
            buf.get(43, 33),
            night.on(FLOOR),
            "the lamp lights its bulb's cell"
        );
    }
}
