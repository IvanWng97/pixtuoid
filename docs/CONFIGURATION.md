# Configuration

pixtuoid stores its settings in `~/.config/pixtuoid/config.toml` (respecting
`$XDG_CONFIG_HOME`). The file is created on first launch. **Every user setting is
optional** — omit a key to use its default. CLI flags override the file
(e.g. `pixtuoid run --theme dracula`).

## Example

```toml
theme = "cyberpunk"
max-desks = 8
pack-dir = "~/.config/pixtuoid/packs/robot"

# One stanza per pet. Omit the whole section to show all pets with default
# names; use `pets = []` to disable all pets. `name` is optional (shown in
# the pet's hover tooltip). Keep [[pets]] last — it's a table section.
[[pets]]
kind = "cat"
name = "Whiskers"   # optional — omit for "Office Cat"

[[pets]]
kind = "dog"        # name omitted → "Office Dog"
```

## User settings (safe to edit)

| Key | Default | Description |
|-----|---------|-------------|
| `theme` | `"normal"` | Color theme — `normal`, `cyberpunk`, `dracula`, `tokyo-night`, `catppuccin`, `gruvbox`. |
| `max-desks` | auto | Cap desks per floor. If unset, auto-computed from terminal size. Excess agents overflow to additional floors. |
| `pack-dir` | — | Custom sprite pack directory. Supports `~` expansion. See [Custom sprite packs](#custom-sprite-packs). |
| `[[pets]]` | all kinds, default names | One stanza per pet. `kind` (`"cat"`/`"dog"`) is required; `name` is optional (the hover-tooltip label, default `Office Cat`/`Office Dog`). Omit the section for all pets; `pets = []` for none; an unknown `kind` is skipped without affecting other settings. Keep it last (it's a table section). |

## System-managed (don't edit — pixtuoid writes these for you)

| Key | Purpose |
|-----|---------|
| `last-seen-version` | Tracks the highest version you've launched, so the "what's new" popup only fires once per upgrade. Pixtuoid overwrites this on every launch. |

## Themes

Press `t` in the TUI to switch themes with a live preview picker (`j`/`k` or
`↑`/`↓` to navigate); your choice is written back to `config.toml` and persists
across sessions. Override for a single run with `--theme <name>`. Six themes ship
built-in: `normal`, `cyberpunk`, `dracula`, `tokyo-night`, `catppuccin`,
`gruvbox`.

## Custom sprite packs

Create your own character sprites:

```bash
pixtuoid init-pack ./my-pack     # extract skeleton template
# edit the .sprite files in ./my-pack
pixtuoid validate-pack ./my-pack # check for missing animations
pixtuoid run --pack-dir ./my-pack
```

A **robot** pack ships as an example at `crates/pixtuoid/sprites/robot/`. See the
[sprite format docs](../crates/pixtuoid/CLAUDE.md) for palette keys and animation requirements.
