//! The cutaway painted into a buffer kept across frames: a frame whose draw
//! list matches the last one's is not painted at all, and one that differs is
//! painted whole but reports where it can differ, so a painter re-encodes only
//! there.

use std::collections::HashMap;

use pixtuoid_core::sprite::{Rgb, RgbBuffer};

use crate::cutaway::light::Ambient;
use crate::cutaway::order::Span;
use crate::cutaway::paint::{Office, frame_list, paint};
use crate::layout::Bounds;
use crate::pixel_painter::SimFrame;
use crate::render_scale::RenderScale;

/// A cutaway painter's frame buffer and what it shows.
pub struct CutawayCanvas {
    buf: RgbBuffer,
    /// `None` until the first frame.
    shown: Option<Shown>,
}

impl Default for CutawayCanvas {
    fn default() -> Self {
        Self {
            buf: RgbBuffer::filled(0, 0, Rgb { r: 0, g: 0, b: 0 }),
            shown: None,
        }
    }
}

/// One frame from [`CutawayCanvas::frame`].
pub struct CanvasFrame<'a> {
    /// The whole frame, as [`render_cutaway`](crate::cutaway::paint::render_cutaway)
    /// paints it.
    pub buf: &'a RgbBuffer,
    /// Where it may differ from the canvas's previous frame.
    pub dirty: Dirty,
}

/// Where a frame's pixels may differ from the frame before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dirty {
    /// Anywhere.
    All,
    /// Only inside these, in buffer pixels on whole layout cells; none at all
    /// when the frame was not painted.
    Rects(Vec<Bounds>),
}

/// What every pixel of a frame is painted under, beyond its draw list.
#[derive(PartialEq, Eq)]
struct Epoch {
    // By address, as a floor memoizes it (`FloorCtx::frame_layout`): a
    // recompute allocates while the layout it replaces is alive, and one of
    // another size fails the buffer's size check too.
    layout: usize,
    theme: usize,
    pack: usize,
    scale: RenderScale,
    ambient: Ambient,
}

struct Shown {
    epoch: Epoch,
    /// Every piece's reach and every light's span, each with its fingerprint.
    footprints: Vec<(Span, u64)>,
}

impl CutawayCanvas {
    /// Paint `frame`'s `office` as [`render_cutaway`](crate::cutaway::paint::render_cutaway)
    /// does, unless the frame would be the one already shown.
    pub fn frame(
        &mut self,
        frame: &SimFrame,
        office: Office<'_>,
        floor: crate::floor::FloorMeta,
        now: std::time::SystemTime,
        cache: &mut crate::frame_cache::FrameCache,
    ) -> CanvasFrame<'_> {
        let list = frame_list(frame, office, floor, now);
        let epoch = Epoch {
            layout: std::ptr::from_ref(office.layout).addr(),
            theme: std::ptr::from_ref(office.theme).addr(),
            pack: std::ptr::from_ref(office.pack).addr(),
            scale: office.scale,
            ambient: list.ambient(),
        };
        let footprints: Vec<(Span, u64)> = list
            .pieces()
            .iter()
            .map(|p| (p.reach(), p.fingerprint))
            .chain(list.lights().iter().map(|l| (l.span, l.fingerprint)))
            .collect();
        let size = (
            office.scale.to_buffer(office.layout.buf_w),
            office.scale.to_buffer(office.layout.buf_h),
        );
        let dirty = match self.shown.take() {
            Some(shown)
                if shown.epoch == epoch && (self.buf.width(), self.buf.height()) == size =>
            {
                Dirty::Rects(
                    changed(&shown.footprints, &footprints)
                        .into_iter()
                        .filter_map(|s| on_buffer(s, office.scale, size))
                        .collect(),
                )
            }
            _ => Dirty::All,
        };
        if dirty != Dirty::Rects(Vec::new()) {
            if (self.buf.width(), self.buf.height()) != size {
                self.buf = RgbBuffer::filled(size.0, size.1, office.theme.surface.bg_fallback);
            }
            paint(office.layout, &list, cache, &mut self.buf);
        }
        self.shown = Some(Shown { epoch, footprints });
        CanvasFrame {
            buf: &self.buf,
            dirty,
        }
    }
}

/// The spans of the footprints in `was` or `now` but not both, counting
/// repeats; each once, in order.
fn changed(was: &[(Span, u64)], now: &[(Span, u64)]) -> Vec<Span> {
    let mut count: HashMap<(Span, u64), isize> = HashMap::new();
    for &f in was {
        *count.entry(f).or_default() += 1;
    }
    for &f in now {
        *count.entry(f).or_default() -= 1;
    }
    was.iter()
        .chain(now)
        .filter_map(|f| {
            let n = count.get_mut(f)?;
            (std::mem::take(n) != 0).then_some(f.0)
        })
        .collect()
}

/// `span`'s cells in buffer pixels at `scale`, clipped to a `w`×`h` buffer;
/// `None` where none of it is left.
fn on_buffer(span: Span, scale: RenderScale, (w, h): (u16, u16)) -> Option<Bounds> {
    let (x, y) = (
        scale.to_buffer(span.x0).min(w),
        scale.to_buffer(span.y0).min(h),
    );
    let x1 = scale.to_buffer(span.x1.saturating_add(1)).min(w);
    let y1 = scale.to_buffer(span.y1.saturating_add(1)).min(h);
    (x1 > x && y1 > y).then_some(Bounds {
        x,
        y,
        width: x1 - x,
        height: y1 - y,
    })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use pixtuoid_core::sprite::format::Pack;

    use super::*;
    use crate::cutaway::paint::render_cutaway;
    use crate::cutaway::paint::tests::{empty_frame, lively_office, sit_down};
    use crate::floor::FloorMeta;
    use crate::layout::Layout;

    /// How a run of frames through a canvas was reported.
    #[derive(Debug, Default)]
    struct Run {
        skipped: usize,
        partial: usize,
        whole: usize,
        /// Pixels that changed outside what diffing by span, not reach, reports.
        missed_by_span: usize,
    }

    /// Drive `steps` through one canvas beside a full render of each, at the
    /// pack's densest scale: every frame it shows, painted or skipped, is the
    /// full render, and every pixel two full renders in a row differ in lies in
    /// what it reported.
    fn run(layout: &Layout, pack: &Pack, steps: &[(SimFrame, SystemTime)]) -> Run {
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let office = Office {
            layout,
            pack,
            theme,
            scale,
        };
        let floor = FloorMeta::ground();
        let (w, h) = (scale.to_buffer(layout.buf_w), scale.to_buffer(layout.buf_h));
        let inside = |rects: &[Bounds], i: usize| {
            let (x, y) = ((i % usize::from(w)) as u16, (i / usize::from(w)) as u16);
            rects
                .iter()
                .any(|r| (r.x..r.x + r.width).contains(&x) && (r.y..r.y + r.height).contains(&y))
        };
        let mut canvas = CutawayCanvas::default();
        let (mut canvas_cache, mut cache) = (
            crate::frame_cache::FrameCache::new(),
            crate::frame_cache::FrameCache::new(),
        );
        let mut tally = Run::default();
        let mut last: Option<(RgbBuffer, Vec<(Span, u64)>)> = None;
        for (k, (frame, now)) in steps.iter().enumerate() {
            let mut full = RgbBuffer::filled(w, h, theme.surface.bg_fallback);
            render_cutaway(frame, office, floor, *now, &mut cache, &mut full);
            let list = frame_list(frame, office, floor, *now);
            let spans: Vec<(Span, u64)> = list
                .pieces()
                .iter()
                .map(|p| (p.span, p.fingerprint))
                .chain(list.lights().iter().map(|l| (l.span, l.fingerprint)))
                .collect();
            let shown = canvas.frame(frame, office, floor, *now, &mut canvas_cache);
            assert!(
                shown.buf.as_slice() == full.as_slice(),
                "step {k}: the canvas shows other than the full render ({:?})",
                shown.dirty
            );
            if let Some((before, was_spans)) = &last {
                let differ: Vec<usize> = (0..full.as_slice().len())
                    .filter(|&i| before.as_slice()[i] != full.as_slice()[i])
                    .collect();
                match &shown.dirty {
                    Dirty::All => tally.whole += 1,
                    Dirty::Rects(rects) => {
                        if rects.is_empty() {
                            tally.skipped += 1;
                        } else {
                            tally.partial += 1;
                        }
                        assert_eq!(
                            differ.iter().find(|&&i| !inside(rects, i)),
                            None,
                            "step {k}: a pixel changed outside {rects:?}"
                        );
                        let by_span: Vec<Bounds> = changed(was_spans, &spans)
                            .into_iter()
                            .filter_map(|s| on_buffer(s, scale, (w, h)))
                            .collect();
                        tally.missed_by_span +=
                            differ.iter().filter(|&&i| !inside(&by_span, i)).count();
                    }
                }
            }
            last = Some((full, spans));
        }
        tally
    }

    /// `n` instants a frame tick apart from `start`.
    fn ticks(start: SystemTime, n: u64) -> impl Iterator<Item = SystemTime> {
        (0..n).map(move |i| start + Duration::from_millis(100 * i))
    }

    /// A walk to a desk and the sit, at noon when shadows are deepest. Diffing
    /// by span instead of reach misses where the walker's shadow moved, so the
    /// walk proves the reach is needed.
    #[test]
    fn a_walk_repaints_only_where_it_moves() {
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::South, 4);
        let steps: Vec<_> = frames
            .into_iter()
            .zip(ticks(crate::localclock::at_hour(12), u64::MAX))
            .collect();
        let tally = run(&layout, &pack, &steps);
        assert!(tally.partial > 0, "{tally:?}");
        assert!(
            tally.missed_by_span > 0,
            "diffing by span missed no shadow, so the reach goes untested: {tally:?}"
        );
    }

    /// From afternoon into night, ten minutes a step, the room's tone moves
    /// with the hour and the weather, repainting the whole frame; the steps
    /// between move only the sky and the lights.
    #[test]
    fn each_change_of_the_rooms_tone_repaints_the_whole_frame() {
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
        let seated = frames.last().expect("a seated frame");
        let steps: Vec<_> = (0..36)
            .map(|i| {
                (
                    seated.clone(),
                    crate::localclock::at_hour_min(16 + i / 6, (i % 6) * 10),
                )
            })
            .collect();
        let tally = run(&layout, &pack, &steps);
        assert!(tally.whole > 0 && tally.partial > 0, "{tally:?}");
    }

    /// The elevator opening, the sign fading up to alert and both appliances
    /// busy, a tick apart.
    #[test]
    fn what_plays_repaints_only_where_it_plays() {
        let layout = lively_office();
        let pack = crate::embedded_pack::test_default_pack();
        let quiet = empty_frame(&layout);
        let steps: Vec<_> = ticks(crate::localclock::at_hour(21), 30)
            .enumerate()
            .map(|(i, now)| {
                let frame = SimFrame {
                    door_frame: i.min(5) / 2,
                    neon: crate::floor::NeonLevels {
                        alert: (i as f32 / 20.0).min(1.0),
                        ..crate::floor::NeonLevels::CALM
                    },
                    occupied_waypoints: (0..layout.waypoints.len()).collect(),
                    ..quiet.clone()
                };
                (frame, now)
            })
            .collect();
        let tally = run(&layout, &pack, &steps);
        assert!(tally.partial > 0, "{tally:?}");
    }

    /// An office with nobody in it skips the ticks on which nothing it shows
    /// moves, and reports only where the sign breathes or a loop plays on the
    /// rest.
    #[test]
    fn an_idle_office_skips_the_ticks_that_change_nothing() {
        let layout = lively_office();
        let pack = crate::embedded_pack::test_default_pack();
        let quiet = empty_frame(&layout);
        let steps: Vec<_> = ticks(crate::localclock::at_hour(12), 30)
            .map(|now| (quiet.clone(), now))
            .collect();
        let tally = run(&layout, &pack, &steps);
        assert!(tally.skipped > 0 && tally.partial > 0, "{tally:?}");
        assert_eq!(tally.whole, 0, "{tally:?}");
    }

    /// A change of anything the list does not fingerprint repaints the whole
    /// frame: the theme, the scale.
    #[test]
    fn a_new_theme_or_scale_repaints_everything() {
        let layout = lively_office();
        let pack = crate::embedded_pack::test_default_pack();
        let frame = empty_frame(&layout);
        let now = crate::localclock::at_hour(12);
        let floor = FloorMeta::ground();
        let mut cache = crate::frame_cache::FrameCache::new();
        let mut canvas = CutawayCanvas::default();
        let normal = crate::theme::theme_by_name("normal").expect("theme");
        let other = crate::theme::ALL_THEMES
            .iter()
            .copied()
            .find(|t| !std::ptr::eq(*t, normal))
            .expect("a second theme");
        let mut dirty = |theme, s| {
            let office = Office {
                layout: &layout,
                pack: &pack,
                theme,
                scale: RenderScale::new(s).expect("nonzero"),
            };
            canvas.frame(&frame, office, floor, now, &mut cache).dirty
        };
        assert_eq!(dirty(normal, 2), Dirty::All, "the first frame");
        assert_eq!(dirty(normal, 2), Dirty::Rects(Vec::new()), "the same frame");
        assert_eq!(dirty(other, 2), Dirty::All, "a new theme");
        assert_eq!(dirty(other, 3), Dirty::All, "a new scale");
    }
}
