---
mode: agent
description: "Add a new color theme to pixtuoid"
---

# Add a new theme

Add a new color theme named `${input:name}` to pixtuoid.

1. Read an existing theme for the full field set — e.g.
   `crates/pixtuoid-scene/src/theme/dracula.rs`. A theme is a `pub static Theme`
   with ~90 color roles across **9** groups (including `ApplianceColors` for the
   vending machine / printer / coat rack, and `SourceColors` for the per-CLI
   dashboard badge hues).
2. Create `crates/pixtuoid-scene/src/theme/<name>.rs` defining
   `pub static <NAME>: Theme = Theme { ... }`. Fill **every** field — each
   appliance/UI color must be supplied; never fall back to the normal palette
   (corridor appliances rendered wrong until each theme supplied its own set).
3. Register it: add the `mod` in `theme/mod.rs`, append `&<NAME>` to the
   `ALL_THEMES` slice, and make sure `theme_by_name()` resolves the kebab-case
   name.
4. Add a row to `site/src/themes.json` (`id` = the kebab-case `name`, plus its
   presentation fields). `theme_gallery_manifest_matches_all_themes` (`theme/mod.rs`)
   asserts the manifest ids == `ALL_THEMES` names, so the theme **fails `just test`**
   until the row exists; then run `just gen-media` to regenerate the committed
   theme stills (else the smoke `gen-check` reds the PR).
5. Theme roles **may share an RGB** (every bundled theme does) — the unique-RGB
   rule belongs to sprite packs (`RECOLOR_KEYS` B/H/S/P, enforced at pack load by
   `validate_recolor_palette`), not to themes. What binds you are the per-theme
   legibility guards in `theme/mod.rs`.
6. Run `just test`. The `appliance_palette_is_legible_for_every_theme`,
   `source_badges_legible_for_every_theme`,
   `token_paper_is_legible_on_the_desk_for_every_theme` and
   `sun_and_moon_read_warm_and_cool_for_every_theme` guards and
   the theme snapshot tests must pass; update insta snapshots if the theme list
   changed.
7. Visually verify: build and render the `snapshot` example, then eyeball the new
   theme's office (see `.claude/skills/beautify-decoration/SKILL.md`).

Follow `.github/instructions/rust.instructions.md` and the theme notes in
`crates/pixtuoid/src/tui/CLAUDE.md`.
