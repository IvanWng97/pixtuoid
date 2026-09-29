//! What lies on the floor under the room's light, pixel-free: elliptical pools of
//! light and of shadow. The classic painter blends a pool per pixel; the cutaway
//! steps it on its art grid.

/// A shadow's strength at noon, where a solid's shadow is crispest.
const NOON_SHADOW: f32 = 0.5;
/// How much of [`NOON_SHADOW`] night takes away: the lamps' shadows are softer.
const NIGHT_SHADOW_LOSS: f32 = 0.3;

/// How much flatter than wide a contact shadow lies on the floor.
const CONTACT_FLATTEN: u16 = 3;
/// The deepest a contact shadow reaches, in rows either side of its base.
const CONTACT_MAX_HALF_H: u16 = 3;

/// An axis-aligned ellipse on the floor, in logical units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Ellipse {
    pub(crate) cx: u16,
    pub(crate) cy: u16,
    pub(crate) half_w: u16,
    pub(crate) half_h: u16,
}

impl Ellipse {
    /// The shadow a solid `w` wide from `x` casts where it meets the floor on
    /// row `base`: centred on that row, so the solid hides its north half and
    /// the south half falls toward the viewer, away from the windows' light.
    pub(crate) fn contact(x: u16, w: u16, base: u16) -> Self {
        let half_w = w / 2 + 1;
        Self {
            cx: x + w / 2,
            cy: base,
            half_w,
            half_h: (half_w / CONTACT_FLATTEN).clamp(1, CONTACT_MAX_HALF_H),
        }
    }

    /// How much of the pool lands at `(x, y)`: full at its centre, none past
    /// its rim.
    pub(crate) fn falloff(self, x: f32, y: f32) -> Option<f32> {
        if self.half_w == 0 || self.half_h == 0 {
            return None;
        }
        falloff(
            (x - f32::from(self.cx)) / f32::from(self.half_w),
            (y - f32::from(self.cy)) / f32::from(self.half_h),
        )
    }

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

/// How much of a pool lands `nx`, `ny` radii from its centre: `1 − r²` inside
/// its rim, nothing outside.
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
    fn a_contact_shadow_is_centred_on_its_base_and_flatter_than_wide() {
        let e = Ellipse::contact(10, 8, 30);
        assert_eq!((e.cx, e.cy), (14, 30));
        assert!(e.half_w > 8 / 2, "it reaches past the solid's sides");
        assert!(e.half_h < e.half_w, "it lies flat");
        assert_eq!(e.falloff(14.0, 30.0), Some(1.0), "full under the centre");
        assert_eq!(e.falloff(14.0, 30.0 + f32::from(e.half_h) + 0.5), None);
    }

    #[test]
    fn night_softens_a_shadow() {
        assert!(shadow_strength(1.0) < shadow_strength(0.0));
        assert!(shadow_strength(1.0) > 0.0, "a lamp still casts one");
    }
}
