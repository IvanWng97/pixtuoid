
#[cfg(test)]
mod scratch_walk {
    use super::tests::*;
    use super::*;

    #[test]
    #[ignore]
    fn scratch_walk() {
        let full = std::env::var("SCRATCH_FULL").is_ok();
        let dir = std::env::var("SCRATCH_DIR").expect("SCRATCH_DIR");
        let kind = match std::env::var("SCRATCH_PET").expect("SCRATCH_PET").as_str() {
            "dog" => crate::pet::PetKind::Dog,
            _ => crate::pet::PetKind::Cat,
        };
        let theme = crate::theme::theme_by_name("normal").expect("theme");
        let (layout, pack, frames, _) = sit_down(crate::layout::Facing::South, 2);
        let base = frames.last().expect("seated").clone();
        let a = base.characters[0].anchor;
        let (steps, step_ms) = (40u64, 110u64);
        // West along the clear floor beside the desk at the sim's measured pace
        // (~3.6 px/s), the walk frame turning every 220 ms as  does.
        let (steps, step_ms): (u64, u64) = (std::env::var("SCRATCH_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(48), std::env::var("SCRATCH_STEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(110));
        let y = 66u16;
        let x_at = |i: u64| 156 - (i * step_ms * 36 / 10_000) as u16;
        let scale = RenderScale::new(std::env::var("SCRATCH_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(4)).expect("nonzero");
        let (w, h) = (176u16, 128u16);
        let (x0, y0) = (scale.to_buffer(118), scale.to_buffer(48)); let _ = a;
        let (x0, y0, w, h) = if full { (0, 0, scale.to_buffer(layout.buf_w), scale.to_buffer(layout.buf_h)) } else { (x0, y0, w, h) };
        let noon = crate::localclock::at_hour(12);
        let sky = crate::sky::Sky::at_with(noon, crate::sky::Weather::Clear);
        for i in 0..steps {
            let still: Option<&'static str> = std::env::var("SCRATCH_STILL").ok().map(|a| &*Box::leak(a.into_boxed_str()));
            let anim_name = still.unwrap_or(kind.walk_anim());
            let walk = pack.animation(anim_name).expect("the anim");
            let frame_idx = if still.is_some() { i as usize % walk.frames().len() } else { ((i * step_ms) / u64::from(walk.frame_ms())) as usize % walk.frames().len() };
            let (pos, flip, anim) = if still.is_some() { (Point { x: 147, y }, false, anim_name) } else { (Point { x: x_at(i), y }, true, kind.walk_anim()) };
            let mut frame = base.clone();
            frame.pet = Some(crate::sim::PetPlacement { kind, pos, flip, anim_name: anim, frame_idx, effects: Vec::new() });
            let list = build_list(&frame, Office { layout: &layout, pack: &pack, theme, scale }, &Moment::resolve(sky, theme, 0.0, noon), crate::floor::FloorMeta::ground(), quiet_board());
            let mut buf = RgbBuffer::filled(scale.to_buffer(layout.buf_w), scale.to_buffer(layout.buf_h), theme.surface.bg_fallback);
            paint(&layout, &list, &mut CutawayCache::default(), &mut buf);
            let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
            for y in y0..y0 + h { for x in x0..x0 + w { let c = buf.get(x.min(buf.width() - 1), y.min(buf.height() - 1)); out.extend([c.r, c.g, c.b]); } }
            std::fs::write(format!("{dir}/f{i:03}.ppm"), out).expect("write");
        }
        std::fs::write(format!("{dir}/meta.txt"), format!("x {}..{} y {y} step_ms {step_ms}", x_at(0), x_at(steps - 1))).expect("meta");
    }
}
