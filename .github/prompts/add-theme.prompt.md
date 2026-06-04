---
mode: agent
description: "Add a new color theme to pixtuoid"
---

# Add a new theme

Add a new color theme named `${input:name}` to pixtuoid.

1. Read an existing theme for the full field set — e.g.
   `crates/pixtuoid/src/tui/theme/dracula.rs`. A theme is a `pub static Theme`
   with ~110 color roles across 8 groups (including `ApplianceColors` for the
   vending machine / printer / coat rack).
2. Create `crates/pixtuoid/src/tui/theme/<name>.rs` defining
   `pub static <NAME>: Theme = Theme { ... }`. Fill **every** field — each
   appliance/UI color must be supplied; never fall back to the normal palette
   (corridor appliances rendered wrong until each theme supplied its own set).
3. Register it: add the `mod` in `theme/mod.rs`, append `&<NAME>` to the
   `ALL_THEMES` slice, and make sure `theme_by_name()` resolves the kebab-case
   name.
4. Each palette key must map to a **unique RGB** — the renderer recolors by RGB
   equality (`recolor_frame`); duplicate keys collide.
5. Run `just test`. The `appliance_palette_is_legible_for_every_theme` guard and
   the theme snapshot tests must pass; update insta snapshots if the theme list
   changed.
6. Visually verify: build and render the `snapshot` example, then eyeball the new
   theme's office (see `.claude/skills/beautify-decoration/SKILL.md`).

Follow `.github/instructions/rust.instructions.md` and the theme notes in
`crates/pixtuoid/src/tui/CLAUDE.md`.
