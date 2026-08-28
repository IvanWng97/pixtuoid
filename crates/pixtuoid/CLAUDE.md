# pixtuoid (binary) — agent guide

The **TUI binary**: `ratatui` + `crossterm` + `winit` + `tokio` + `clap`. Wires
sources → reducer → renderer; owns the CLI subcommands, hook installation,
config persistence, and multi-floor orchestration. Its two thin painters over
the `pixtuoid-scene` engine ([`../pixtuoid-scene/CLAUDE.md`](../pixtuoid-scene/CLAUDE.md))
are `src/tui/` ([`src/tui/CLAUDE.md`](src/tui/CLAUDE.md)) and `floating/` —
neither depends on the other. Cross-cutting rules: workspace
[`CLAUDE.md`](../../CLAUDE.md).

## Layout

Module map: `ls src/` — each file's `//!` header is its annotation.
(`sprites/` holds the robot + skeleton packs, NOT under pixtuoid-hook.)
