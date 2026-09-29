//! Drop the bundled pack's density art — its `@Nx` animations and the
//! hairstyles that dress them — for a build without the `density-art` feature. `build.rs` and `the_pack_without_density_art_loads_whole`
//! both run it, so the manifest a feature-less build embeds is one a test has
//! loaded.

use std::collections::BTreeSet;

/// `pack_toml` without its density-variant animations and its hairstyles (which
/// only ever dress density art), and the frame files only those drew.
///
/// Every `@` key in the bundled manifest's `[animations]` is a density variant:
/// `embedded_default_pack_animations_are_all_in_the_registry` fails on any
/// other, so core's `DENSITY_VARIANT_SEP` alone identifies them here, where its
/// `split_density_variant` is crate-private and core is no build-dependency.
pub(crate) fn strip_density_art(pack_toml: &str) -> (String, BTreeSet<String>) {
    let mut doc: toml_edit::DocumentMut = pack_toml.parse().expect("the bundled pack.toml parses");
    let mut hair_files: Vec<String> = Vec::new();
    if let Some(styles) = doc.remove("hairstyles") {
        for (_, style) in styles.as_table_like().into_iter().flat_map(|t| t.iter()) {
            for (_, layers) in style.as_table_like().into_iter().flat_map(|t| t.iter()) {
                for (_, file) in layers.as_table_like().into_iter().flat_map(|t| t.iter()) {
                    hair_files.extend(file.as_str().map(str::to_owned));
                }
            }
        }
    }
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
    let mut dropped: BTreeSet<String> = hair_files.into_iter().collect();
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
    fn hairstyles_go_with_the_density_art_they_dress() {
        let (toml, dropped) = strip_density_art(
            "[animations.a]\nframes = [\"a.sprite\"]\n\
             [hairstyles.\"mop@2x\"]\nfront = { behind = \"b.sprite\", over = \"o.sprite\" }\n",
        );
        assert_eq!(
            dropped.into_iter().collect::<Vec<_>>(),
            ["b.sprite", "o.sprite"]
        );
        assert!(!toml.contains("hairstyles"), "{toml}");
    }

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
