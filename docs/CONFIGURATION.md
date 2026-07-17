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
| `max-desks` | auto | Cap desks per floor (≥ 1; `0` is ignored with a warning). If unset, auto-computed from terminal size. Excess agents overflow to additional floors. Applies to the `run` TUI; `pixtuoid floating` sizes its floors from the window. |
| `pack-dir` | — | Custom sprite pack directory. Supports `~` expansion. See [Custom sprite packs](#custom-sprite-packs). |
| `[[pets]]` | all kinds, default names | One stanza per pet. `kind` (`"cat"`/`"dog"`) is required; `name` is optional (the hover-tooltip label, default `Office Cat`/`Office Dog`). Omit the section for all pets; `pets = []` for none; an unknown `kind` is skipped without affecting other settings. Keep it last (it's a table section). |

## System-managed (don't edit — pixtuoid writes these for you)

| Key | Purpose |
|-----|---------|
| `last-seen-version` | Tracks the last version whose "what's new" popup you've seen, so the popup only fires once per upgrade. Pixtuoid rewrites it when the popup fires, on first launch, or to repair an unparseable value — not on every launch. |
| `[sources]` | Per-agent-CLI connection state (`source-id = true/false`), written when you connect/disconnect a source in the in-TUI **Sources panel** (`s`) or via the scriptable CLI (`pixtuoid connect`/`disconnect`/`sources set`/`setup --yes`). When a source has no entry it is simply not connected (since 0.12.0; on a first run — no `[sources]` yet — the onboarding wizard offers the detected CLIs to connect). A disconnected source's characters are hidden even if its hooks/transcripts are still present. |
| `[floating]` | Geometry of the `pixtuoid floating` desktop window (`width`/`height`/`x`/`y`), rewritten when the window closes. Sizes below 240×160 clamp up on load; `x`/`y` are dropped when the OS can't report the position (the next launch is OS-placed). A user-set `opacity` is accepted (clamped 0.2–1.0) and preserved across the rewrite, but isn't applied yet. |
| `[audio]` | Ambient office sound — **starts muted** (`muted = true` is the default; a terminal app never speaks uninvited). Press `m` — in the TUI or the floating window — to turn it on; the toggle persists here, so the office boots exactly as you left it, and the sound system only spins up on the first unmute (muted costs nothing). A ♩ in the TUI footer means you'd hear sound right now. `volume` (0.0–1.0, clamped, default 1.0) scales everything on a perceptual curve — low percents get genuinely quiet, and the whole bus sits under your real work audio by design; `+`/`-` nudge it live (±0.05, persisted, a `♩ N%` readout flashes in the footer — the floating window flashes the same readout in its bottom-right corner; `+` from muted also unmutes). Unmuted: a lofi office soundscape — after dark (the same sundown the office lights follow) or in rain it crossfades to a slower night take — mixed live from how busy **the floor you're viewing** is (a warm band layers up with active agents; typing density tracks them; other floors are silent until you ride to them), gentle rain when the office weather rains (weather is global — it's outside the windows), plus one-shots — a door chime on walk-ins and printer/vending moments. Every sound is synthesized at startup (no audio files). Prebuilt **Linux** binaries ship without audio (ALSA can't link into the static/cross builds); building from source on Linux enables it when the ALSA headers (`libasound2-dev`) are present. |

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
[binary guide](../crates/pixtuoid/CLAUDE.md) for pack loading, and the
[scene engine guide](../crates/pixtuoid-scene/CLAUDE.md) for the recolor palette keys.

## Logging & troubleshooting

The TUI owns your terminal (alternate screen), so runtime diagnostics go to a
**log file** instead of stderr:

| | |
|-----|-----|
| Default path | `~/.cache/pixtuoid/log` (or `$XDG_STATE_HOME/pixtuoid/log` if set) |
| Custom path | set `$PIXTUOID_LOG=/path/to/file` |
| Level | `warn` and above by default; `--log-level debug` or `trace` (or `$RUST_LOG`) raises it |
| Rotation | one generation: past 5 MB the file rotates to `<name>.old` at startup |

Warnings about a misconfigured `config.toml` (unknown theme, bad `[[pets]]`
kind, malformed TOML) are also printed to stderr **before** the office takes
over the screen — scroll back after quitting to see them. If a data source
dies mid-run (e.g. the hook listener), the footer shows a persistent ⚠ warning
and the full error is in the log file.

Crashes are reported separately to `~/.cache/pixtuoid/crash.log`.

Non-TUI commands (`--headless`, `validate-pack`, …) log to stderr directly.

### Truecolor preflight

The pixel-art office renders in 24-bit color. On launch, `pixtuoid run`
**asks your terminal** whether it supports truecolor — it sets an unlikely
24-bit color and queries it back (a `DECRQSS` probe) — rather than guessing from
the terminal's name. If the terminal doesn't confirm, it prints a one-line
stderr warning. It's **warn-only** (never blocks) and scrolls away once the
office takes over. (`$COLORTERM=truecolor` is taken as a yes and skips the query;
the query runs only otherwise.) Run `pixtuoid doctor` for the detected
`terminal:` verdict.

A terminal that's genuinely truecolor but doesn't answer the query (rare) may
still get warned. If you know your terminal is fine, silence the warning with
`$PIXTUOID_NO_TRUECOLOR_WARN=1` (any of `1`/`true`/`yes`/`on`). Note: `tmux`
doesn't implement the `DECRQSS` query, so a truecolor tmux session can trip this
warning — set `$PIXTUOID_NO_TRUECOLOR_WARN=1` (tmux usually advertises
`$COLORTERM`, which skips the query, so most setups never see it).

### When color is disabled (`$NO_COLOR`, `$TERM=dumb`)

The office has **no legible monochrome mode** — it's color end to end. So rather
than render unreadable blocks, `pixtuoid run` refuses to launch the canvas and
explains why when color is turned off:

- **`$NO_COLOR`** (the [no-color.org](https://no-color.org) convention; any
  non-empty value): color output is disabled, so the office can't render. Unset
  `NO_COLOR`, or override per the standard precedence with **`$CLICOLOR_FORCE=1`**
  (forces color on despite `$NO_COLOR`; a `0` value does not force). An *empty*
  `$NO_COLOR` is ignored (it doesn't actually disable color).
- **`$TERM=dumb`**: the terminal can't render escape sequences or color at all.

In both cases use a graphical terminal, or `pixtuoid run --headless` for a plain
text summary (which works fine without color). `pixtuoid doctor` reports the
active color status. This gate applies only to the terminal `run` TUI —
`--headless`, `doctor`, `sources`, and the `floating` window are unaffected.
