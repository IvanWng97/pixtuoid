# pixtuoid (binary) — agent guide

The **TUI binary**: wires sources → reducer → renderer; owns the CLI
subcommands, hook installation, config persistence, and multi-floor
orchestration. Its two thin painters over the `pixtuoid-scene` engine
([`../pixtuoid-scene/CLAUDE.md`](../pixtuoid-scene/CLAUDE.md)) are `src/tui/`
([`src/tui/CLAUDE.md`](src/tui/CLAUDE.md)) and `floating/` — neither depends
on the other. Module map: `ls src/` — each file's `//!` header is its
annotation. (`sprites/` holds the robot + skeleton packs, NOT under
pixtuoid-hook.)
