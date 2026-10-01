//! The cutaway painted into a buffer kept across frames: a frame whose draw
//! list matches the last one's is not painted at all, and one that differs is
//! painted whole but reports where it can differ, so a painter re-encodes only
//! there.

use std::collections::HashMap;
use std::sync::Arc;

use pixtuoid_core::AgentId;
use pixtuoid_core::sprite::RgbBuffer;
use pixtuoid_core::sprite::format::Pack;

use crate::cutaway::light::Ambient;
use crate::cutaway::order::Span;
use crate::cutaway::paint::{Office, Showing, frame_list, paint};
use crate::floor::ObservedFloor;
use crate::layout::{Bounds, Layout};
use crate::render_scale::RenderScale;
use crate::theme::Theme;

/// A cutaway painter's frame buffer and what it shows, for one pack.
pub struct CutawayCanvas {
    // Held, so no other pack can take its place unnoticed.
    pack: Arc<Pack>,
    buf: RgbBuffer,
    /// `None` until the first frame.
    shown: Option<Shown>,
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

/// What every pixel of a frame is painted under, beyond its draw list and
/// the canvas's pack.
struct Epoch {
    // Held, so a later layout cannot reuse its address.
    layout: Arc<Layout>,
    // A static, so its address is its identity.
    theme: &'static Theme,
    scale: RenderScale,
    ambient: Ambient,
}

impl PartialEq for Epoch {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.layout, &other.layout)
            && std::ptr::eq(self.theme, other.theme)
            && self.scale == other.scale
            && self.ambient == other.ambient
    }
}

struct Shown {
    epoch: Epoch,
    /// Every piece's reach and every light's span, each with its fingerprint.
    footprints: Vec<(Span, u64)>,
    /// [`DrawList::hover_spans`](crate::cutaway::paint::DrawList::hover_spans).
    hovers: Vec<(Span, Option<AgentId>)>,
}

impl CutawayCanvas {
    /// A canvas that draws with `pack`.
    pub fn new(pack: Arc<Pack>) -> Self {
        Self {
            pack,
            buf: RgbBuffer::filled(0, 0, pixtuoid_core::sprite::Rgb { r: 0, g: 0, b: 0 }),
            shown: None,
        }
    }

    /// Paint `observed` as [`render_cutaway`](crate::cutaway::paint::render_cutaway)
    /// does, unless the frame would be the one already shown.
    pub fn frame(
        &mut self,
        observed: &ObservedFloor,
        theme: &'static Theme,
        scale: RenderScale,
        showing: Showing<'_>,
        cache: &mut crate::frame_cache::FrameCache,
    ) -> CanvasFrame<'_> {
        let layout = &observed.layout;
        let office = Office {
            layout,
            pack: &self.pack,
            theme,
            scale,
        };
        let list = frame_list(&observed.frame, office, showing);
        let epoch = Epoch {
            layout: Arc::clone(layout),
            theme,
            scale,
            ambient: list.ambient(),
        };
        let footprints: Vec<(Span, u64)> = list
            .pieces()
            .iter()
            .map(|p| (p.reach(), p.fingerprint))
            .chain(list.lights().iter().map(|l| (l.span, l.fingerprint)))
            .collect();
        let hovers = list.hover_spans().collect();
        let size = (scale.to_buffer(layout.buf_w), scale.to_buffer(layout.buf_h));
        let dirty = match self.shown.take() {
            Some(shown) if shown.epoch == epoch => Dirty::Rects(
                changed(&shown.footprints, &footprints)
                    .into_iter()
                    .filter_map(|s| on_buffer(s, scale, size))
                    .collect(),
            ),
            _ => Dirty::All,
        };
        if dirty != Dirty::Rects(Vec::new()) {
            if (self.buf.width(), self.buf.height()) != size {
                self.buf = RgbBuffer::filled(size.0, size.1, theme.surface.bg_fallback);
            }
            paint(layout, &list, cache, &mut self.buf);
        }
        self.shown = Some(Shown {
            epoch,
            footprints,
            hovers,
        });
        CanvasFrame {
            buf: &self.buf,
            dirty,
        }
    }

    /// The agent the last frame shows topmost over `area`, in LOGICAL units
    /// as [`Layout`], not [`Dirty::Rects`]' buffer pixels; `None` where a
    /// piece that is no agent lies over it, or none does.
    pub fn hover_at(&self, area: Bounds) -> Option<AgentId> {
        let shown = self.shown.as_ref()?;
        shown.hovers.iter().rev().find(|(s, _)| s.meets(area))?.1
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

    use super::*;
    use crate::cutaway::paint::render_cutaway;
    use crate::cutaway::paint::tests::{empty_frame, lively_office, sit_down};
    use crate::embedded_pack::test_default_pack;
    use crate::floor::FloorMeta;
    use crate::pixel_painter::SimFrame;

    /// How a run of frames through a canvas was reported.
    #[derive(Debug, Default)]
    struct Run {
        skipped: usize,
        partial: usize,
        whole: usize,
        /// Pixels that changed outside what diffing by span, not reach, reports.
        missed_by_span: usize,
    }

    fn normal() -> &'static Theme {
        crate::theme::theme_by_name("normal").expect("theme")
    }

    /// `layout`'s full render of `frame` at `now`, under the normal theme.
    fn full_render(
        layout: &Layout,
        pack: &Pack,
        scale: RenderScale,
        frame: &SimFrame,
        now: SystemTime,
    ) -> RgbBuffer {
        let theme = normal();
        let mut buf = RgbBuffer::filled(
            scale.to_buffer(layout.buf_w),
            scale.to_buffer(layout.buf_h),
            theme.surface.bg_fallback,
        );
        let office = Office {
            layout,
            pack,
            theme,
            scale,
        };
        let mut cache = crate::frame_cache::FrameCache::new();
        render_cutaway(
            frame,
            office,
            crate::cutaway::paint::tests::showing(FloorMeta::ground(), now),
            &mut cache,
            &mut buf,
        );
        buf
    }

    /// Drive `steps` through one canvas beside a full render of each, at the
    /// pack's densest scale: every frame it shows, painted or skipped, is the
    /// full render, and every pixel two full renders in a row differ in lies in
    /// what it reported.
    fn run(layout: Layout, pack: Pack, steps: &[(SimFrame, SystemTime)]) -> Run {
        let (layout, pack) = (Arc::new(layout), Arc::new(pack));
        let theme = normal();
        let scale = RenderScale::new(pack.max_density_variant().get()).expect("nonzero");
        let office = Office {
            layout: &layout,
            pack: &pack,
            theme,
            scale,
        };
        let floor = FloorMeta::ground();
        let w = scale.to_buffer(layout.buf_w);
        let inside = |rects: &[Bounds], i: usize| {
            let (x, y) = ((i % usize::from(w)) as u16, (i / usize::from(w)) as u16);
            rects
                .iter()
                .any(|r| (r.x..r.x + r.width).contains(&x) && (r.y..r.y + r.height).contains(&y))
        };
        let mut canvas = CutawayCanvas::new(Arc::clone(&pack));
        let mut cache = crate::frame_cache::FrameCache::new();
        let mut tally = Run::default();
        let mut last: Option<(RgbBuffer, Vec<(Span, u64)>)> = None;
        for (k, (frame, now)) in steps.iter().enumerate() {
            let full = full_render(&layout, &pack, scale, frame, *now);
            let list = frame_list(
                frame,
                office,
                crate::cutaway::paint::tests::showing(floor, *now),
            );
            let spans: Vec<(Span, u64)> = list
                .pieces()
                .iter()
                .map(|p| (p.span, p.fingerprint))
                .chain(list.lights().iter().map(|l| (l.span, l.fingerprint)))
                .collect();
            let observed = ObservedFloor {
                layout: Arc::clone(&layout),
                frame: frame.clone(),
            };
            let shown = canvas.frame(
                &observed,
                theme,
                scale,
                crate::cutaway::paint::tests::showing(floor, *now),
                &mut cache,
            );
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
                            .filter_map(|s| on_buffer(s, scale, (w, scale.to_buffer(layout.buf_h))))
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
        let tally = run(layout, pack, &steps);
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
        let tally = run(layout, pack, &steps);
        assert!(tally.whole > 0 && tally.partial > 0, "{tally:?}");
    }

    /// The elevator opening, the sign fading up to alert and both appliances
    /// busy, a tick apart.
    #[test]
    fn what_plays_repaints_only_where_it_plays() {
        let layout = lively_office();
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
        let tally = run(layout, test_default_pack(), &steps);
        assert!(tally.partial > 0, "{tally:?}");
    }

    /// An office with nobody in it skips the ticks on which nothing it shows
    /// moves, and reports only where the sign breathes or a loop plays on the
    /// rest.
    #[test]
    fn an_idle_office_skips_the_ticks_that_change_nothing() {
        // Rain or snow on the glass moves every tick.
        let _clear = crate::sky::ForcedWeather::new(crate::sky::Weather::Clear);
        let layout = lively_office();
        let quiet = empty_frame(&layout);
        let steps: Vec<_> = ticks(crate::localclock::at_hour(12), 30)
            .map(|now| (quiet.clone(), now))
            .collect();
        let tally = run(layout, test_default_pack(), &steps);
        assert!(tally.skipped > 0 && tally.partial > 0, "{tally:?}");
        assert_eq!(tally.whole, 0, "{tally:?}");
    }

    /// A change of anything the list does not fingerprint repaints the whole
    /// frame: the theme, the scale.
    #[test]
    fn a_new_theme_or_scale_repaints_everything() {
        let layout = Arc::new(lively_office());
        let observed = ObservedFloor {
            frame: empty_frame(&layout),
            layout,
        };
        let now = crate::localclock::at_hour(12);
        let mut cache = crate::frame_cache::FrameCache::new();
        let mut canvas = CutawayCanvas::new(Arc::new(test_default_pack()));
        let other = crate::theme::ALL_THEMES
            .iter()
            .copied()
            .find(|t| !std::ptr::eq(*t, normal()))
            .expect("a second theme");
        let mut dirty = |theme, s| {
            let scale = RenderScale::new(s).expect("nonzero");
            canvas
                .frame(
                    &observed,
                    theme,
                    scale,
                    crate::cutaway::paint::tests::showing(FloorMeta::ground(), now),
                    &mut cache,
                )
                .dirty
        };
        assert_eq!(dirty(normal(), 2), Dirty::All, "the first frame");
        assert_eq!(
            dirty(normal(), 2),
            Dirty::Rects(Vec::new()),
            "the same frame"
        );
        assert_eq!(dirty(other, 2), Dirty::All, "a new theme");
        assert_eq!(dirty(other, 3), Dirty::All, "a new scale");
    }

    /// A walk to a north-facing desk, shown at scale 2.
    struct Hovering {
        layout: Arc<Layout>,
        pack: Arc<Pack>,
        frames: Vec<SimFrame>,
        scale: RenderScale,
    }

    impl Hovering {
        fn new() -> Self {
            let (layout, pack, frames, _) = sit_down(crate::layout::Facing::North, 2);
            Self {
                layout: Arc::new(layout),
                pack: Arc::new(pack),
                frames,
                scale: RenderScale::new(2).expect("nonzero"),
            }
        }

        fn now() -> SystemTime {
            crate::localclock::at_hour(12)
        }

        /// `frame`'s pieces as `(is a desk, hover box, agent)`, back to front.
        fn boxes(&self, frame: &SimFrame) -> Vec<(bool, Span, Option<AgentId>)> {
            let office = Office {
                layout: &self.layout,
                pack: &self.pack,
                theme: normal(),
                scale: self.scale,
            };
            let list = frame_list(
                frame,
                office,
                crate::cutaway::paint::tests::showing(FloorMeta::ground(), Self::now()),
            );
            list.pieces()
                .iter()
                .filter(|p| !matches!(p.kind, crate::cutaway::paint::PieceKind::Badge { .. }))
                .zip(list.hover_spans())
                .map(|(p, (span, agent))| {
                    (
                        matches!(p.kind, crate::cutaway::paint::PieceKind::Desk { .. }),
                        span,
                        agent,
                    )
                })
                .collect()
        }

        /// `canvas` after showing `frame`.
        fn show(&self, canvas: &mut CutawayCanvas, frame: &SimFrame) {
            let observed = ObservedFloor {
                layout: Arc::clone(&self.layout),
                frame: frame.clone(),
            };
            let mut cache = crate::frame_cache::FrameCache::new();
            canvas.frame(
                &observed,
                normal(),
                self.scale,
                crate::cutaway::paint::tests::showing(FloorMeta::ground(), Self::now()),
                &mut cache,
            );
        }
    }

    /// What one half-block terminal cell shows, from logical `(x, y)` down.
    fn cell((x, y): (u16, u16)) -> Bounds {
        Bounds {
            x,
            y,
            width: 1,
            height: 2,
        }
    }

    /// Where a cell shows the walker over a desk drawn before them, hovering
    /// names them: the topmost box wins.
    #[test]
    fn hovering_a_walker_in_front_of_a_desk_names_the_walker() {
        let h = Hovering::new();
        let (frame, area, id) = h
            .frames
            .iter()
            .find_map(|frame| {
                let boxes = h.boxes(frame);
                boxes.iter().enumerate().find_map(|(i, &(desk, d, _))| {
                    if !desk {
                        return None;
                    }
                    boxes[i + 1..].iter().find_map(|&(_, b, agent)| {
                        let area = cell((d.x0.max(b.x0), d.y0.max(b.y0)));
                        (d.meets(area) && b.meets(area)).then_some((frame, area, agent?))
                    })
                })
            })
            .expect("the walk passes in front of a desk");
        let mut canvas = CutawayCanvas::new(Arc::clone(&h.pack));
        h.show(&mut canvas, frame);
        assert_eq!(canvas.hover_at(area), Some(id));
    }

    /// A cell showing floor no piece stands on hovers nothing.
    #[test]
    fn hovering_bare_floor_names_nobody() {
        let h = Hovering::new();
        let frame = h.frames.last().expect("a frame");
        let boxes = h.boxes(frame);
        let area = (h.layout.top_margin..h.layout.buf_h - 1)
            .rev()
            .flat_map(|y| (0..h.layout.buf_w).map(move |x| cell((x, y))))
            .find(|&a| boxes.iter().all(|&(_, s, _)| !s.meets(a)))
            .expect("bare floor");
        let mut canvas = CutawayCanvas::new(Arc::clone(&h.pack));
        assert_eq!(canvas.hover_at(area), None, "before any frame");
        h.show(&mut canvas, frame);
        assert_eq!(canvas.hover_at(area), None);
    }

    /// Hovering answers from the last frame: where the walker stood before
    /// they walked to their desk no longer names them.
    #[test]
    fn hovering_where_a_walker_left_names_them_no_more() {
        let h = Hovering::new();
        let (first, last) = (&h.frames[0], h.frames.last().expect("a frame"));
        let (body, id) = h
            .boxes(first)
            .into_iter()
            .find_map(|(_, s, agent)| Some((s, agent?)))
            .expect("the walker");
        let area = cell(((body.x0 + body.x1) / 2, (body.y0 + body.y1) / 2));
        assert!(
            h.boxes(last)
                .iter()
                .all(|&(_, s, agent)| agent != Some(id) || !s.meets(area)),
            "the walker never left {area:?}"
        );
        let mut canvas = CutawayCanvas::new(Arc::clone(&h.pack));
        h.show(&mut canvas, first);
        assert_eq!(canvas.hover_at(area), Some(id));
        h.show(&mut canvas, last);
        assert_ne!(canvas.hover_at(area), Some(id));
    }

    /// A neighbour sitting just south has their badge plate over the sitter's
    /// body; hovering the body there still names the sitter, as in the classic,
    /// where a badge is no hover target.
    #[test]
    fn hovering_a_sitter_under_a_neighbours_badge_names_the_sitter() {
        const NEIGHBOUR: &str = "cc\u{b7}neighbour";
        let h = Hovering::new();
        let seated = h.frames.last().expect("a seated frame");
        let a = seated.characters[0].clone();
        let a_id = seated.agents[a.agent_idx].agent_id;
        let body = h
            .boxes(seated)
            .into_iter()
            .find_map(|(_, s, agent)| (agent == Some(a_id)).then_some(s))
            .expect("the sitter's body");
        let mut tried = 0;
        for dy in 1..body.y1 - body.y0 + 16 {
            let mut both = seated.clone();
            let mut b = both.agents[a.agent_idx].clone();
            b.agent_id = AgentId::from_transcript_path("/neighbour.jsonl");
            b.label = NEIGHBOUR.into();
            both.agents.push(b.clone());
            both.characters
                .push(crate::pixel_painter::CharacterPlacement {
                    agent_idx: both.agents.len() - 1,
                    anchor: crate::layout::Point {
                        x: a.anchor.x + 2,
                        y: a.anchor.y + dy,
                    },
                    anchor_y: a.anchor_y + dy,
                    seat_desk: None,
                    seated: false,
                    ..a.clone()
                });
            let plate = {
                let office = Office {
                    layout: &h.layout,
                    pack: &h.pack,
                    theme: normal(),
                    scale: h.scale,
                };
                let list = frame_list(
                    &both,
                    office,
                    crate::cutaway::paint::tests::showing(FloorMeta::ground(), Hovering::now()),
                );
                // Known by its text: the sitter's badge reads otherwise.
                list.pieces().iter().find(|p| {
                    matches!(&p.kind, crate::cutaway::paint::PieceKind::Badge { badge } if badge.text == NEIGHBOUR)
                })
                    .map(|p| p.span)
                    .expect("the neighbour's plate")
            };
            let b_body = h
                .boxes(&both)
                .into_iter()
                .find_map(|(_, s, agent)| (agent == Some(b.agent_id)).then_some(s))
                .expect("the neighbour's body");
            let Some(at) = (body.y0..=body.y1)
                .flat_map(|y| (body.x0..=body.x1).map(move |x| cell((x, y))))
                .find(|&c| plate.meets(c) && !b_body.meets(c))
            else {
                continue;
            };
            let mut alone = CutawayCanvas::new(Arc::clone(&h.pack));
            h.show(&mut alone, seated);
            if alone.hover_at(at) != Some(a_id) {
                continue;
            }
            tried += 1;
            let mut canvas = CutawayCanvas::new(Arc::clone(&h.pack));
            h.show(&mut canvas, &both);
            assert_eq!(
                canvas.hover_at(at),
                Some(a_id),
                "at {at:?}, {dy} rows south"
            );
        }
        assert!(
            tried > 0,
            "no neighbour's plate ever lay over the sitter's body"
        );
    }

    /// A badge is a piece of its own: a sitter whose name or state changes
    /// repaints their badge, and a renamed one nothing else.
    #[test]
    fn a_changed_badge_repaints_only_itself() {
        let h = Hovering::new();
        let seated = h.frames.last().expect("a seated frame");
        let plate = |frame: &SimFrame| {
            let office = Office {
                layout: &h.layout,
                pack: &h.pack,
                theme: normal(),
                scale: h.scale,
            };
            let list = frame_list(
                frame,
                office,
                crate::cutaway::paint::tests::showing(FloorMeta::ground(), Hovering::now()),
            );
            list.pieces()
                .iter()
                .find(|p| matches!(p.kind, crate::cutaway::paint::PieceKind::Badge { .. }))
                .map(|p| (p.span, p.fingerprint))
                .expect("a badge")
        };
        let mut renamed = seated.clone();
        renamed.agents[0].label = "cc\u{b7}renamed".into();
        let mut canvas = CutawayCanvas::new(Arc::clone(&h.pack));
        h.show(&mut canvas, seated);
        let observed = ObservedFloor {
            layout: Arc::clone(&h.layout),
            frame: renamed.clone(),
        };
        let mut cache = crate::frame_cache::FrameCache::new();
        let size = (
            h.scale.to_buffer(h.layout.buf_w),
            h.scale.to_buffer(h.layout.buf_h),
        );
        let dirty = canvas
            .frame(
                &observed,
                normal(),
                h.scale,
                crate::cutaway::paint::tests::showing(FloorMeta::ground(), Hovering::now()),
                &mut cache,
            )
            .dirty;
        let spans = [plate(seated).0, plate(&renamed).0];
        let want: Vec<Bounds> = spans
            .iter()
            .filter_map(|&s| on_buffer(s, h.scale, size))
            .collect();
        assert_eq!(dirty, Dirty::Rects(want));
        let mut waiting = seated.clone();
        waiting.agents[0].state = pixtuoid_core::state::ActivityState::Waiting {
            reason: "permission?".into(),
        };
        let (was, now) = (plate(seated), plate(&waiting));
        assert_eq!(was.0, now.0, "the plate stays put");
        assert_ne!(was.1, now.1, "its tone is in its fingerprint");
    }

    /// The board's text is a piece of its own: a new tally repaints the board
    /// and nothing else.
    #[test]
    fn a_new_tally_repaints_only_the_board() {
        let h = Hovering::new();
        let seated = h.frames.last().expect("a seated frame");
        let quiet = crate::cutaway::paint::tests::showing(FloorMeta::ground(), Hovering::now());
        let counts = crate::board::StateCounts {
            active: 3,
            total: 3,
            ..crate::board::StateCounts::default()
        };
        let busy = crate::board::build_board(counts, 60, None, None, Hovering::now());
        let busy = Showing {
            board: &busy,
            ..quiet
        };
        let board = |showing| {
            let office = Office {
                layout: &h.layout,
                pack: &h.pack,
                theme: normal(),
                scale: h.scale,
            };
            frame_list(seated, office, showing)
                .pieces()
                .iter()
                .find(|p| matches!(p.kind, crate::cutaway::paint::PieceKind::Board { .. }))
                .map(|p| p.span)
                .expect("the board")
        };
        let mut canvas = CutawayCanvas::new(Arc::clone(&h.pack));
        h.show(&mut canvas, seated);
        let observed = ObservedFloor {
            layout: Arc::clone(&h.layout),
            frame: seated.clone(),
        };
        let mut cache = crate::frame_cache::FrameCache::new();
        let dirty = canvas
            .frame(&observed, normal(), h.scale, busy, &mut cache)
            .dirty;
        let size = (
            h.scale.to_buffer(h.layout.buf_w),
            h.scale.to_buffer(h.layout.buf_h),
        );
        let want: Vec<Bounds> = [board(quiet), board(busy)]
            .into_iter()
            .filter_map(|s| on_buffer(s, h.scale, size))
            .collect();
        assert_eq!(dirty, Dirty::Rects(want));
    }

    /// A new layout of the same size repaints everything, even one built after
    /// the last was dropped, where the allocator may hand back its address.
    #[test]
    fn a_new_layout_repaints_everything() {
        let pack = Arc::new(test_default_pack());
        let scale = RenderScale::new(2).expect("nonzero");
        let now = crate::localclock::at_hour(12);
        let mut cache = crate::frame_cache::FrameCache::new();
        let mut canvas = CutawayCanvas::new(Arc::clone(&pack));
        let observe = |seed| {
            let layout = Layout::compute_with_seed(160, 96, None, seed).expect("lays out");
            ObservedFloor {
                frame: empty_frame(&layout),
                layout: Arc::new(layout),
            }
        };
        let floor = FloorMeta::ground();
        canvas.frame(
            &observe(0),
            normal(),
            scale,
            crate::cutaway::paint::tests::showing(floor, now),
            &mut cache,
        );
        let b = observe(1);
        let shown = canvas.frame(
            &b,
            normal(),
            scale,
            crate::cutaway::paint::tests::showing(floor, now),
            &mut cache,
        );
        assert_eq!(shown.dirty, Dirty::All);
        assert!(
            shown.buf.as_slice() == full_render(&b.layout, &pack, scale, &b.frame, now).as_slice(),
            "the canvas shows other than the new layout"
        );
    }
}
