//! Alpha compositing, painter-neutral: one colour over another by coverage, per
//! sRGB channel, as the web composites. A gradient between two hues whose middle
//! must not sag is [`Rgb::mix`] instead.

use pixtuoid_core::sprite::Rgb;

/// Channel `b` over `a` at coverage `t`: cheap per pixel, and channel-separable,
/// so a constant-tint pass can tabulate it.
pub(crate) fn blend(a: u8, b: u8, t: f32) -> u8 {
    ((a as f32) * (1.0 - t) + (b as f32) * t)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// [`blend`] on each channel, `b` over `a` at one coverage `t`.
pub(crate) fn blend_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    Rgb {
        r: blend(a.r, b.r, t),
        g: blend(a.g, b.g, t),
        b: blend(a.b, b.b, t),
    }
}
