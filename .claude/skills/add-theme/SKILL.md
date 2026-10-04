---
name: add-theme
version: 2.0.0
description: "Add a new color theme to pixtuoid (a full `Theme` palette, rendered into the office). Use when the user says 'add a <name> theme', 'new color scheme', or 'port <palette> to pixtuoid'. Orchestrates the Rust registration PLUS the two steps agents miss — the site manifest bridge test and the committed-media regen."
metadata:
  scope: "pixtuoid repo only"
---

# add-theme

1. Read the `Theme` struct in `crates/pixtuoid-scene/src/theme/mod.rs` and an
   existing theme (e.g. `theme/dracula.rs`) for the full field set.
2. Create `crates/pixtuoid-scene/src/theme/<name>.rs` defining
   `pub static <NAME>: Theme = Theme { ... }`.
3. Register it: add the `mod` in `theme/mod.rs` and append `&<NAME>` to the
   `ALL_THEMES` slice; its `name` field is the kebab-case `--theme` id.
4. Add a row to `site/src/themes.json` (`id` = the kebab-case `name`, plus its
   presentation fields). `theme_gallery_manifest_matches_all_themes` asserts the
   manifest ids == `ALL_THEMES` names — the site never runs the binary, so this
   bridge test is the only guard that its theme switcher stays in sync. The theme
   stills need no commit: the site's render in CI, the README's regenerate on
   main (`just gen-media` renders them locally to look).
5. Theme roles **may share an RGB** (every bundled theme does). What binds you
   are the `*_for_every_theme` legibility guards in `theme/mod.rs`.
6. Run `just test`; update insta snapshots if the theme list changed.
7. Visually verify with the **beautify-decoration** skill's snapshot loop — a
   palette that passes the legibility guards can still read badly.
8. `just preflight full`, then the **local-review** skill — the regenerated
   stills fire REVIEW.md's local "Generated art / clips" row.
