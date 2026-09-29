//! The cutaway's time of day: the room darkens with the sky, and its own lights
//! ([`crate::lighting`]) lift what they fall on, both in whole [`Rgb::ramp`]
//! steps on the art grid. A light's edge is an ordered dither between two steps
//! and its colour a tint at fixed stops, so the room stays a palette: nothing
//! blends continuously, the rule the rest of the cutaway is drawn by.

use std::collections::HashMap;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::cutaway::order::Span;
use crate::cutaway::pen::{dithered, ArtPx, ArtRect, Pen};
use crate::lighting::{Emitter, EmitterKind};
use crate::theme::Theme;

/// Ramp steps the room drops by at full dark.
const AMBIENT_MAX_STEPS: u8 = 4;

/// Ramp steps a light lifts per unit of its level: a fluorescent's pool at full
/// dark then lifts its centre most of the way back to the room's daylight tone,
/// so the night room reads as lit pools in the dark, not as day with stains.
const LIFT_STOPS_PER_LEVEL: f32 = 7.0;

/// The most a light lifts a room no darker than this: one step shows a lamp is
/// on at noon, and a second would bleach the floor under it. A darker room it
/// lifts at most back to its daylight tone, never past.
const DAYLIGHT_LIFT: u8 = 1;

/// The share of a step, at its top, over which a light dithers into the next:
/// below it the band is solid, the pixel-art way of lighting, and a whole band
/// of dither reads as grain.
const SEAM: f32 = 0.3;

/// The whole steps a light `stops` strong lifts the art pixel at `(x, y)`:
/// solid through each band, dithered into the next only across its [`SEAM`].
fn step_at(stops: f32, x: ArtPx, y: ArtPx) -> u8 {
    let whole = stops.max(0.0).floor();
    let into_seam = (stops - whole - (1.0 - SEAM)) / SEAM;
    whole as u8 + u8::from(into_seam > 0.0 && dithered(x, y, into_seam))
}

/// The share of the way to its light's colour a pixel is tinted per step of
/// lift: the brightest cells take the most colour, as they would.
const TINT_PER_STEP: f32 = 0.05;
/// The deepest tint, in steps of [`TINT_PER_STEP`]: past it a lit pixel reads
/// as painted in the light's colour rather than lit by it.
const TINT_MAX_STEPS: u8 = 3;

/// How dark the room is: the sky's darkness in whole steps, so a frame's tone
/// changes a handful of times a day rather than every frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

    fn steps(self) -> u8 {
        self.0
    }

    /// `c` in a room this dark.
    pub(crate) fn on(self, c: Rgb) -> Rgb {
        c.ramp(-(self.0 as i8))
    }
}

/// Step every pixel of `buf` down by `ambient`, or only those that differ from
/// `since`: what a pass painted since it was taken.
pub(crate) fn wash(buf: &mut RgbBuffer, since: Option<&RgbBuffer>, ambient: Ambient) {
    if ambient.steps() == 0 {
        return;
    }
    let mut memo: HashMap<Rgb, Rgb> = HashMap::new();
    let mut step = |c: Rgb| *memo.entry(c).or_insert_with(|| ambient.on(c));
    match since {
        None => {
            for p in buf.as_mut_slice() {
                *p = step(*p);
            }
        }
        Some(since) => {
            debug_assert_eq!(buf.as_slice().len(), since.as_slice().len());
            for (p, old) in buf.as_mut_slice().iter_mut().zip(since.as_slice()) {
                if p != old {
                    *p = step(*p);
                }
            }
        }
    }
}

/// One light as the cutaway paints it, resolved when the list is built: how many
/// steps it lifts each art pixel of its box, row by row from the top-left, and
/// the colour it tints them toward.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct LightView {
    x: u16,
    y: u16,
    w: u16,
    lift: Vec<u8>,
    tint: Option<Rgb>,
}

impl LightView {
    /// `emitter` on `pen`'s grid under `ambient`, clipped to a `buf_w`×`buf_h`
    /// office, with its [`Span`]; `None` where it lifts nothing.
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
        let ceiling = ambient.steps().max(DAYLIGHT_LIFT);
        let lift: Vec<u8> = (ay0..ay1)
            .flat_map(|ay| (ax0..ax1).map(move |ax| (ax, ay)))
            .map(|(ax, ay)| {
                let at = |a: u16| (f32::from(a) + 0.5) / d;
                let Some(level) = emitter.level_at_f(at(ax), at(ay)) else {
                    return 0;
                };
                step_at(level * LIFT_STOPS_PER_LEVEL, ArtPx(ax), ArtPx(ay)).min(ceiling)
            })
            .collect();
        lift.iter().any(|&l| l > 0).then(|| {
            (
                Span::new(x0, y0, x1 - x0, y1 - y0, 0),
                Self {
                    x: ax0,
                    y: ay0,
                    w: ax1 - ax0,
                    lift,
                    tint,
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
}

/// The colour a light of `kind` tints toward under `theme`, or `None` for one
/// the cutaway leaves untinted. A monitor's tool light tints only a dark theme's
/// room, as in the classic ([`paint_ceiling_halos`]): on a light one it reads
/// as grime.
///
/// [`paint_ceiling_halos`]: crate::pixel_painter
pub(crate) fn tint_of(kind: EmitterKind, theme: &Theme) -> Option<Rgb> {
    let lighting = &theme.lighting;
    match kind {
        EmitterKind::CeilingPool => Some(lighting.ceiling_pool),
        EmitterKind::FloorLamp => Some(lighting.floor_lamp_halo),
        EmitterKind::DeskLamp => Some(lighting.desk_lamp),
        EmitterKind::WindowSpill => Some(lighting.sun_spill),
        EmitterKind::NeonGlow => Some(theme.ui.neon_brand),
        EmitterKind::MonitorHalo(tool) => (theme.kind == crate::theme::ThemeKind::Dark)
            .then(|| crate::pixel_painter::tool_glow_for_kind(tool, &theme.tool_glow)),
    }
}

/// Lift what `lights` fall on, clipped to the buffer, leaving every art pixel
/// of `skip` alone. Where two overlap the brighter one lights the pixel, with
/// its own tint, rather than the two compounding.
pub(crate) fn paint_lights<'v>(
    lights: impl Iterator<Item = &'v LightView> + Clone,
    skip: &[ArtRect],
    pen: Pen,
    buf: &mut RgbBuffer,
) {
    let Some(bounds) = lights.clone().map(LightView::rect).reduce(union) else {
        return;
    };
    let (w, h) = (usize::from(bounds.w.0), usize::from(bounds.h.0));
    let mut best: Vec<(u8, Option<Rgb>)> = vec![(0, None); w * h];
    for view in lights {
        let r = view.rect();
        for (i, &lift) in view.lift.iter().enumerate() {
            let (dx, dy) = (i % usize::from(r.w.0), i / usize::from(r.w.0));
            let (x, y) = (
                usize::from(r.x.0 - bounds.x.0) + dx,
                usize::from(r.y.0 - bounds.y.0) + dy,
            );
            let slot = &mut best[y * w + x];
            if lift > 0 && lift >= slot.0 {
                *slot = (lift, view.tint);
            }
        }
    }
    let mut memo: HashMap<(Rgb, u8, Option<Rgb>), Rgb> = HashMap::new();
    for (i, &(lift, tint)) in best.iter().enumerate() {
        if lift == 0 {
            continue;
        }
        let at = ArtRect {
            x: ArtPx(bounds.x.0 + (i % w) as u16),
            y: ArtPx(bounds.y.0 + (i / w) as u16),
            w: ArtPx(1),
            h: ArtPx(1),
        };
        if skip.iter().any(|s| contains(*s, at.x, at.y)) {
            continue;
        }
        pen.recolour(buf, at, |_, _, under| {
            *memo
                .entry((under, lift, tint))
                .or_insert_with(|| lit(under, lift, tint))
        });
    }
}

/// `c` lifted `lift` steps and tinted toward `tint` at that many stops.
fn lit(c: Rgb, lift: u8, tint: Option<Rgb>) -> Rgb {
    let lifted = c.ramp(lift as i8);
    match tint {
        Some(t) => lifted.mix(t, f32::from(lift.min(TINT_MAX_STEPS)) * TINT_PER_STEP),
        None => lifted,
    }
}

fn union(a: ArtRect, b: ArtRect) -> ArtRect {
    let (x0, y0) = (a.x.0.min(b.x.0), a.y.0.min(b.y.0));
    let x1 = (a.x.0 + a.w.0).max(b.x.0 + b.w.0);
    let y1 = (a.y.0 + a.h.0).max(b.y.0 + b.h.0);
    ArtRect {
        x: ArtPx(x0),
        y: ArtPx(y0),
        w: ArtPx(x1 - x0),
        h: ArtPx(y1 - y0),
    }
}

fn contains(r: ArtRect, x: ArtPx, y: ArtPx) -> bool {
    x.0 >= r.x.0 && x.0 < r.x.0 + r.w.0 && y.0 >= r.y.0 && y.0 < r.y.0 + r.h.0
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

    fn lamp(strength: f32) -> Emitter {
        Emitter {
            kind: EmitterKind::DeskLamp,
            light: Light::Halo {
                centre: Point { x: 10, y: 8 },
                radius: 5,
                share: 1.0,
            },
            strength,
        }
    }

    fn lit_floor(emitter: &Emitter, ambient: Ambient, pen: Pen) -> (RgbBuffer, LightView) {
        let (_, view) =
            LightView::of(emitter, Some(WARM), ambient, pen, (20, 16)).expect("it lifts");
        let (w, h) = (pen.art(20).0, pen.art(16).0);
        let scale = RenderScale::new(4).expect("nonzero");
        let mut buf = RgbBuffer::filled(scale.to_buffer(20), scale.to_buffer(16), FLOOR);
        debug_assert_eq!((w, h), (80, 64));
        paint_lights(std::iter::once(&view), &[], pen, &mut buf);
        (buf, view)
    }

    #[test]
    fn a_lit_lamp_lifts_its_pool_at_night_and_nothing_past_it() {
        let pen = Pen::new(RenderScale::new(4).expect("nonzero"), 4).expect("4 divides 4");
        let night = Ambient(AMBIENT_MAX_STEPS);
        let (buf, _) = lit_floor(&lamp(0.6), night, pen);
        let luma = |c: Rgb| u32::from(c.r) + u32::from(c.g) + u32::from(c.b);
        // The bulb's cell, and a corner the halo does not reach.
        assert!(luma(buf.get(10 * 4 + 1, 8 * 4 + 1)) > luma(FLOOR));
        assert_eq!(buf.get(0, 0), FLOOR);
    }

    /// Nothing blends: a lit pixel is its colour some whole steps up the ramp,
    /// tinted at that many fixed stops, and nothing between.
    #[test]
    fn a_light_paints_only_whole_steps_and_tint_stops() {
        let pen = Pen::new(RenderScale::new(4).expect("nonzero"), 4).expect("4 divides 4");
        let night = Ambient(AMBIENT_MAX_STEPS);
        let (buf, _) = lit_floor(&lamp(0.6), night, pen);
        let allowed: std::collections::HashSet<Rgb> = std::iter::once(FLOOR)
            .chain((1..=AMBIENT_MAX_STEPS).map(|l| lit(FLOOR, l, Some(WARM))))
            .collect();
        let seen: std::collections::HashSet<Rgb> = buf.as_slice().iter().copied().collect();
        assert!(seen.len() > 2, "a pool steps through tones: {seen:?}");
        assert!(
            seen.is_subset(&allowed),
            "off-stop colours: {:?}",
            seen.difference(&allowed)
        );
    }

    /// A band is one step through its middle; only the seam at its top mixes
    /// in the next.
    #[test]
    fn a_light_band_is_solid_but_for_its_seam() {
        for band in 0..4u8 {
            for tenth in 0..10 {
                let stops = f32::from(band) + tenth as f32 / 10.0;
                let steps: std::collections::BTreeSet<u8> = (0..8)
                    .flat_map(|y| (0..8).map(move |x| step_at(stops, ArtPx(x), ArtPx(y))))
                    .collect();
                if (tenth as f32 / 10.0) < 1.0 - SEAM {
                    assert_eq!(
                        steps,
                        [band].into(),
                        "{stops} stops is dithered off its seam"
                    );
                } else {
                    assert!(
                        steps.is_subset(&[band, band + 1].into()),
                        "{stops}: {steps:?}"
                    );
                }
            }
        }
    }

    /// Noon keeps the room's own tone, a light lifts it at most a step, and
    /// midnight takes it down the full ramp.
    #[test]
    fn the_room_steps_down_with_the_dark_and_a_light_back_up() {
        assert_eq!(Ambient(0).darkness(), 0.0);
        assert_eq!(Ambient(AMBIENT_MAX_STEPS).darkness(), 1.0);
        let pen = Pen::new(RenderScale::new(4).expect("nonzero"), 4).expect("4 divides 4");
        let (_, noon) = lit_floor(&lamp(1.0), Ambient(0), pen);
        assert_eq!(noon.lift.iter().max(), Some(&DAYLIGHT_LIFT));
        let mut buf = RgbBuffer::filled(4, 4, FLOOR);
        wash(&mut buf, None, Ambient(AMBIENT_MAX_STEPS));
        assert_eq!(buf.get(0, 0), FLOOR.ramp(-(AMBIENT_MAX_STEPS as i8)));
    }
}
