use std::time::SystemTime;

use crate::tui::layout::Point;

/// Duration (ms) the pet stays frozen in place after being petted.
pub const PET_DURATION_MS: u64 = 2000;

/// State for the "pet the animal" interaction. Lives on `TuiRenderer`
/// (render-side only) — petting is a local visual effect, not a data
/// model concern. Same pattern as `mouse_pos` and `pinned_agent`.
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
        now.duration_since(self.petted_at)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PetKind {
    Cat,
    Dog,
}

impl PetKind {
    pub const ALL: &'static [PetKind] = &[PetKind::Cat, PetKind::Dog];

    pub fn config_name(self) -> &'static str {
        match self {
            PetKind::Cat => "cat",
            PetKind::Dog => "dog",
        }
    }

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

    pub fn hitbox(self, anim_name: &str) -> (u16, u16) {
        if anim_name == self.walk_anim() {
            (8, 6)
        } else if anim_name == self.sleep_anim() {
            (6, 4)
        } else {
            (6, 6)
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
    fn config_name_roundtrip() {
        for &kind in PetKind::ALL {
            assert_eq!(PetKind::from_config_name(kind.config_name()), Some(kind));
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
            let (ww, _) = kind.hitbox(kind.walk_anim());
            let (sw, _) = kind.hitbox(kind.sit_anim());
            assert!(ww > sw, "{:?} walk should be wider than sit", kind);
        }
    }

    #[test]
    fn hitbox_sleep_shorter_than_sit() {
        for &kind in PetKind::ALL {
            let (_, sh) = kind.hitbox(kind.sit_anim());
            let (_, slh) = kind.hitbox(kind.sleep_anim());
            assert!(slh < sh, "{:?} sleep should be shorter than sit", kind);
        }
    }

    #[test]
    fn hitbox_unknown_anim_returns_default() {
        assert_eq!(PetKind::Cat.hitbox("unknown"), (6, 6));
        assert_eq!(PetKind::Dog.hitbox("unknown"), (6, 6));
    }
}
