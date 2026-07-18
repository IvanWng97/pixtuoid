use std::time::SystemTime;

use crate::layout::{Point, Size};

/// Duration (ms) the pet stays frozen in place after being petted.
pub const PET_DURATION_MS: u64 = 2000;

/// State for the "pet the animal" interaction. Lives on `TuiRenderer`
/// (render-side only) — petting is a local visual effect, not a data
/// model concern. Same pattern as `mouse_pos`.
pub struct PetState {
    pub petted_at: SystemTime,
    pub pet_pos: Point,
    pub kind: PetKind,
    pub floor_idx: usize,
}

impl PetState {
    pub fn is_active(&self, now: SystemTime) -> bool {
        now.duration_since(self.petted_at)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(PET_DURATION_MS + 1)
            < PET_DURATION_MS
    }

    pub fn elapsed_ms(&self, now: SystemTime) -> u64 {
        // Delegate to the crate's saturate-to-0 helper (byte-identical). `is_active`
        // above keeps its own form: its backward-clock fallback is `PET_DURATION_MS+1`
        // (treat-as-expired), the deliberate variant anim.rs says not to migrate.
        crate::anim::elapsed_ms(now, self.petted_at)
    }
}

/// The pet's resolved render frame for one tick (position + anim + kind),
/// produced by the pixel pass and consumed by the renderer/tooltip/hit-test.
#[derive(Clone, Copy)]
pub struct PetFrame {
    pub pos: Point,
    pub anim: &'static str,
    pub kind: PetKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PetKind {
    Cat,
    Dog,
}

impl PetKind {
    pub const ALL: &'static [PetKind] = &[PetKind::Cat, PetKind::Dog];

    pub fn from_config_name(s: &str) -> Option<Self> {
        match s {
            "cat" => Some(PetKind::Cat),
            "dog" => Some(PetKind::Dog),
            _ => None,
        }
    }

    pub fn walk_anim(self) -> &'static str {
        match self {
            PetKind::Cat => "cat_walk",
            PetKind::Dog => "dog_walk",
        }
    }

    pub fn sit_anim(self) -> &'static str {
        match self {
            PetKind::Cat => "cat_sit",
            PetKind::Dog => "dog_sit",
        }
    }

    pub fn sleep_anim(self) -> &'static str {
        match self {
            PetKind::Cat => "cat_sleep",
            PetKind::Dog => "dog_sleep",
        }
    }

    /// Default display name shown in the hover tooltip when a `[[pets]]` stanza
    /// gives no `name`. Single source for these strings (the tooltip reads this
    /// rather than hardcoding them).
    pub fn default_name(self) -> &'static str {
        match self {
            PetKind::Cat => "Office Cat",
            PetKind::Dog => "Office Dog",
        }
    }

    pub fn sleeps_near_idle(self) -> bool {
        match self {
            PetKind::Cat => true,
            PetKind::Dog => false,
        }
    }

    pub fn hitbox(self, anim_name: &str) -> Size {
        if anim_name == self.walk_anim() {
            Size { w: 8, h: 6 }
        } else if anim_name == self.sleep_anim() {
            Size { w: 6, h: 4 }
        } else {
            Size { w: 6, h: 6 }
        }
    }
}

/// A pet configured for the office: its [`PetKind`] plus the display name shown
/// in the hover tooltip. The name is resolved ONCE (custom from the `[[pets]]`
/// stanza, else [`PetKind::default_name`]) so the render path never does a name
/// lookup or fallback — it reads `pet.name` directly. Keying the office's pets
/// as a `&[Pet]` (not a parallel `Vec<PetKind>` + name map) makes "every enabled
/// pet has a name" true by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pet {
    pub kind: PetKind,
    pub name: String,
}

impl Pet {
    /// A pet of `kind` with its default name — the no-custom-name case.
    pub fn defaulted(kind: PetKind) -> Self {
        Self {
            kind,
            name: kind.default_name().to_string(),
        }
    }
}

pub fn select_pet_for_floor(floor_seed: u64, pets: &[Pet]) -> Option<&Pet> {
    if pets.is_empty() {
        return None;
    }
    Some(&pets[(floor_seed as usize) % pets.len()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_name_cat_and_dog() {
        assert_eq!(PetKind::Cat.default_name(), "Office Cat");
        assert_eq!(PetKind::Dog.default_name(), "Office Dog");
    }

    #[test]
    fn default_name_all_nonempty() {
        for &k in PetKind::ALL {
            assert!(!k.default_name().is_empty(), "{k:?} default_name empty");
        }
    }

    #[test]
    fn every_pet_kind_is_reachable_from_config() {
        for &kind in PetKind::ALL {
            // Exhaustive match — adding a PetKind without a config string
            // breaks compile HERE instead of warn-skipping at config load.
            // (This forcing function used to live in the deleted
            // config_name's own exhaustive match.)
            let name = match kind {
                PetKind::Cat => "cat",
                PetKind::Dog => "dog",
            };
            assert_eq!(PetKind::from_config_name(name), Some(kind));
        }
    }

    #[test]
    fn from_config_name_unknown_returns_none() {
        assert_eq!(PetKind::from_config_name("hamster"), None);
    }

    #[test]
    fn select_pet_empty_returns_none() {
        assert_eq!(select_pet_for_floor(42, &[]), None);
    }

    #[test]
    fn select_pet_single_always_returns_it() {
        let pets = [Pet::defaulted(PetKind::Dog)];
        assert_eq!(
            select_pet_for_floor(0, &pets).map(|p| p.kind),
            Some(PetKind::Dog)
        );
        assert_eq!(
            select_pet_for_floor(99, &pets).map(|p| p.kind),
            Some(PetKind::Dog)
        );
    }

    #[test]
    fn select_pet_two_pets_alternates_by_seed() {
        let pets = vec![Pet::defaulted(PetKind::Cat), Pet::defaulted(PetKind::Dog)];
        let floor0 = select_pet_for_floor(0, &pets).map(|p| p.kind);
        let floor1 = select_pet_for_floor(1, &pets).map(|p| p.kind);
        assert_ne!(floor0, floor1);
    }

    #[test]
    fn defaulted_uses_default_name() {
        assert_eq!(Pet::defaulted(PetKind::Cat).name, "Office Cat");
        assert_eq!(Pet::defaulted(PetKind::Dog).kind, PetKind::Dog);
    }

    #[test]
    fn anim_names_match_kind() {
        assert!(PetKind::Cat.walk_anim().starts_with("cat_"));
        assert!(PetKind::Dog.walk_anim().starts_with("dog_"));
    }

    #[test]
    fn dog_anim_methods() {
        assert_eq!(PetKind::Dog.walk_anim(), "dog_walk");
        assert_eq!(PetKind::Dog.sit_anim(), "dog_sit");
        assert_eq!(PetKind::Dog.sleep_anim(), "dog_sleep");
    }

    #[test]
    fn dog_does_not_sleep_near_idle() {
        assert!(!PetKind::Dog.sleeps_near_idle());
        assert!(PetKind::Cat.sleeps_near_idle());
    }

    #[test]
    fn hitbox_walk_larger_than_sit() {
        for &kind in PetKind::ALL {
            let ww = kind.hitbox(kind.walk_anim()).w;
            let sw = kind.hitbox(kind.sit_anim()).w;
            assert!(ww > sw, "{:?} walk should be wider than sit", kind);
        }
    }

    #[test]
    fn hitbox_sleep_shorter_than_sit() {
        for &kind in PetKind::ALL {
            let sh = kind.hitbox(kind.sit_anim()).h;
            let slh = kind.hitbox(kind.sleep_anim()).h;
            assert!(slh < sh, "{:?} sleep should be shorter than sit", kind);
        }
    }

    #[test]
    fn hitbox_unknown_anim_returns_default() {
        assert_eq!(PetKind::Cat.hitbox("unknown"), Size { w: 6, h: 6 });
        assert_eq!(PetKind::Dog.hitbox("unknown"), Size { w: 6, h: 6 });
    }

    #[test]
    fn is_active_true_within_window_false_after_and_on_a_backward_clock() {
        use std::time::Duration;
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let pet = PetState {
            petted_at: t0,
            pet_pos: Point { x: 10, y: 10 },
            kind: PetKind::Cat,
            floor_idx: 0,
        };
        // Within the freeze window → active.
        assert!(pet.is_active(t0 + Duration::from_millis(PET_DURATION_MS / 2)));
        // At/after the window → inactive (the `< PET_DURATION_MS` boundary).
        assert!(!pet.is_active(t0 + Duration::from_millis(PET_DURATION_MS)));
        assert!(!pet.is_active(t0 + Duration::from_millis(PET_DURATION_MS + 500)));
        // Backward clock (now < petted_at): the deliberate `unwrap_or(PET_DURATION_MS+1)`
        // fallback treats it as EXPIRED, not active — guards that constant against a
        // `unwrap_or(0)` regression that would freeze the pet on a clock step.
        assert!(!pet.is_active(t0 - Duration::from_secs(1)));
    }
}
