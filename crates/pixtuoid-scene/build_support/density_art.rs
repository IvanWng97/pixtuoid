//! Drop the bundled pack's density variants, for a build without the
//! `density-art` feature. `build.rs` and `the_pack_without_density_art_loads_whole`
//! both run it, so the manifest a feature-less build embeds is one a test has
//! loaded.

use std::collections::BTreeSet;

/// `pack_toml` without its density-variant animations, and the frame files only
/// those animations drew.
///
/// Every `@` key in the bundled manifest is a density variant:
/// `embedded_default_pack_animations_are_all_in_the_registry` fails on any
/// other, so core's `DENSITY_VARIANT_SEP` alone identifies them here, where its
/// `split_density_variant` is crate-private and core is no build-dependency.
pub(crate) fn strip_density_art(pack_toml: &str) -> (String, BTreeSet<String>) {
    let mut doc: toml_edit::DocumentMut = pack_toml.parse().expect("the bundled pack.toml parses");
    let animations = doc["animations"]
        .as_table_like_mut()
        .expect("the bundled pack.toml has an [animations] table");
    let variants: Vec<String> = animations
        .iter()
        .map(|(name, _)| name.to_string())
        .filter(|name| name.contains('@'))
        .collect();
    let frames_of = |item: Option<&toml_edit::Item>| -> Vec<String> {
        item.and_then(|a| a.get("frames"))
            .and_then(|f| f.as_array())
            .map(|f| {
                f.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut dropped: BTreeSet<String> = BTreeSet::new();
    for name in &variants {
        dropped.extend(frames_of(animations.get(name)));
        animations.remove(name);
    }
    // A frame a kept animation still draws stays, even if a variant shared it.
    for (_, kept) in animations.iter() {
        for frame in frames_of(Some(kept)) {
            dropped.remove(&frame);
        }
    }
    (doc.to_string(), dropped)
}

#[cfg(test)]
mod tests {
    use super::strip_density_art;

    #[test]
    fn a_frame_a_kept_animation_draws_survives_its_variant() {
        let (toml, dropped) = strip_density_art(
            "[animations.a]\nframes = [\"a.sprite\"]\n\
             [animations.\"a@2x\"]\nframes = [\"a.sprite\", \"a@2x.sprite\"]\n",
        );
        assert_eq!(dropped.into_iter().collect::<Vec<_>>(), ["a@2x.sprite"]);
        assert!(
            toml.contains("[animations.a]") && !toml.contains("a@2x"),
            "{toml}"
        );
    }
}
