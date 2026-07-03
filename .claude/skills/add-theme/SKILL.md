---
name: add-theme
version: 1.0.0
description: "Add a new color theme to pixtuoid (a full ~110-role palette across 9 groups, rendered into the office). Use when the user says 'add a <name> theme', 'new color scheme', or 'port <palette> to pixtuoid'. Orchestrates the Rust registration PLUS the two steps agents miss — the site manifest bridge test and the committed-media regen."
metadata:
  scope: "pixtuoid repo only"
---

# add-theme (v1)

A theme is a `pub static Theme` with ~110 color roles across **9** groups
(surface, office, lighting, furniture, effects, ui, tool_glow, `ApplianceColors`
for corridor appliances, and `SourceColors` for per-CLI dashboard badge hues).
Every field must be supplied — corridor appliances render wrong until each theme
provides its own set.

## When to use

- "Add a `<name>` theme" / "new color scheme" / "port `<palette>`".

## The checklist

Full current steps: **[`.github/prompts/add-theme.prompt.md`](../../../.github/prompts/add-theme.prompt.md)**
+ the theme notes in [`crates/pixtuoid/src/tui/CLAUDE.md`](../../../crates/pixtuoid/src/tui/CLAUDE.md).
Read an existing theme (e.g. `crates/pixtuoid-scene/src/theme/dracula.rs`) for the
full field set, then:

1. Create `crates/pixtuoid-scene/src/theme/<name>.rs` — fill EVERY field; never
   fall back to the normal palette.
2. Register: `mod` in `theme/mod.rs`, append `&<NAME>` to `ALL_THEMES`,
   `theme_by_name()` resolves the kebab-case name.
3. **Each palette key must map to a UNIQUE RGB** — the renderer recolors by RGB
   equality (`recolor_frame`); duplicate keys collide silently.

## The two steps agents miss (both have teeth)

- **`site/src/themes.json` row** (`id` = the kebab-case name) —
  `theme_gallery_manifest_matches_all_themes` (`theme/mod.rs`) asserts the
  manifest ids == `ALL_THEMES` names, so the theme **fails `just test`** until the
  row exists. The site never runs the binary, so this bridge test is the only
  guard that the switcher stays in sync.
- **`just gen-media`** — `themes.json` drives the committed theme stills; a new
  theme drifts them, so regenerate and commit them or the smoke `gen-check` reds
  the PR (the same error the bridge test's message points at).

(Full step list + field details: `add-theme.prompt.md` + the tui `CLAUDE.md` theme
notes — this skill headlines the two teeth steps agents miss.)

## Finish

- `just test` — `appliance_palette_is_legible_for_every_theme` + the snapshot
  tests must pass; update insta snapshots if the theme list changed.
- **Visually verify** — render the `snapshot` example and eyeball the office (see
  the `beautify-decoration` skill); a palette that passes the legibility guard can
  still read badly.
- `just preflight`, then the **two-lens-review** skill (a theme is public-facing —
  add the editorial/film-critic lens for the rendered stills).
