//! What lies on the floor under the room's light, pixel-free: elliptical pools of
//! light and of shadow. The classic painter blends a pool per pixel; the cutaway
//! steps it on its art grid.

/// A shadow's strength at noon, where a solid's shadow is crispest.
const NOON_SHADOW: f32 = 0.5;
/// How much of [`NOON_SHADOW`] night takes away: the lamps' shadows are softer.
const NIGHT_SHADOW_LOSS: f32 = 0.3;

/// How far a contact shadow reaches past each side of its solid, in logical
/// units.
pub(crate) const CONTACT_REACH: u16 = 1;
/// How much flatter than wide a contact shadow lies on the floor.
const CONTACT_FLATTEN: u16 = 3;
/// How deep a contact shadow reaches either side of its base, in rows: at
/// least enough to read as a pool rather than a stroke under a narrow solid.
const CONTACT_HALF_H: std::ops::RangeInclusive<u16> = 2..=3;

/// An axis-aligned ellipse on the floor, in logical units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Ellipse {
    pub(crate) cx: u16,
    pub(crate) cy: u16,
    pub(crate) half_w: u16,
    pub(crate) half_h: u16,
}

impl Ellipse {
    /// The first column and row it can reach, and the ones just past it.
    pub(crate) fn bounds(self) -> ((u16, u16), (u16, u16)) {
        (
            (
                self.cx.saturating_sub(self.half_w),
                self.cy.saturating_sub(self.half_h),
            ),
            (self.cx + self.half_w, self.cy + self.half_h),
        )
    }
}

/// The shadow a solid casts where it meets the floor, in logical units: centred
/// under the solid's middle on the row under its south edge, so the solid hides
/// its north half and the south half falls toward the viewer, away from the
/// windows' light. An odd-width solid's middle falls mid-column, so its centre
/// and half-width are kept doubled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Contact {
    cx2: u32,
    cy: u16,
    half_w2: u32,
    half_h: u16,
}

impl Contact {
    /// Under a solid `w` wide from `x`, meeting the floor on row `base`.
    pub(crate) fn under(x: u16, w: u16, base: u16) -> Self {
        let half_w2 = u32::from(w) + 2 * u32::from(CONTACT_REACH);
        Self {
            cx2: 2 * u32::from(x) + u32::from(w),
            cy: base,
            half_w2,
            half_h: ((half_w2 / 2) as u16 / CONTACT_FLATTEN)
                .clamp(*CONTACT_HALF_H.start(), *CONTACT_HALF_H.end()),
        }
    }

    /// How much of it lands at `(x, y)`: full at its centre, none past its rim.
    pub(crate) fn falloff(self, x: f32, y: f32) -> Option<f32> {
        falloff(
            (2.0 * x - self.cx2 as f32) / self.half_w2 as f32,
            (y - f32::from(self.cy)) / f32::from(self.half_h),
        )
    }

    /// The first column and row it can reach, and the ones just past it.
    pub(crate) fn bounds(self) -> ((u16, u16), (u16, u16)) {
        let x0 = self.cx2.saturating_sub(self.half_w2) / 2;
        let x1 = (self.cx2 + self.half_w2).div_ceil(2);
        (
            (x0 as u16, self.cy.saturating_sub(self.half_h)),
            (x1 as u16, self.cy + self.half_h),
        )
    }
}

/// How much of a pool lands `nx`, `ny` radii from its centre: most at its
/// centre and fading to nothing at its rim, so a pool, stepped into its
/// painter's tones, reads as rings and not a stamped oval.
pub(crate) fn falloff(nx: f32, ny: f32) -> Option<f32> {
    let r2 = nx * nx + ny * ny;
    (r2 <= 1.0).then_some(1.0 - r2)
}

/// A shadow's strength under `darkness` (0 at noon, 1 at night).
pub(crate) fn shadow_strength(darkness: f32) -> f32 {
    NOON_SHADOW - NIGHT_SHADOW_LOSS * darkness
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_contact_shadow_is_centred_under_its_solid_and_flatter_than_wide() {
        for (x, w) in [(10, 8), (10, 7)] {
            let c = Contact::under(x, w, 30);
            let mid = f32::from(x) + f32::from(w) / 2.0;
            assert_eq!(
                c.falloff(mid, 30.0),
                Some(1.0),
                "full under the middle of {w}"
            );
            let off = f32::from(w) / 2.0 + f32::from(CONTACT_REACH) - 0.25;
            assert_eq!(
                c.falloff(mid - off, 30.0),
                c.falloff(mid + off, 30.0),
                "a {w}-wide solid's shadow is as wide east as west"
            );
            assert!(
                c.falloff(mid - off, 30.0).is_some(),
                "it reaches past the sides"
            );
            let ((x0, _), (x1, _)) = c.bounds();
            assert_eq!(x1 - (x + w), x - x0, "bounds as wide east as west for {w}");
            assert!(c.half_h * 2 < w + 2 * CONTACT_REACH, "it lies flat");
        }
    }

    #[test]
    fn night_softens_a_shadow() {
        assert!(shadow_strength(1.0) < shadow_strength(0.0));
        assert!(shadow_strength(1.0) > 0.0, "a lamp still casts one");
    }
}
