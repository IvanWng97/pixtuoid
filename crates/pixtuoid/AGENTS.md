# pixtuoid (binary) — agent guide

The **TUI binary**: wires sources → reducer → renderer; owns the CLI
subcommands, hook installation, config persistence, and multi-floor
orchestration. Its two thin painters over the `pixtuoid-scene` engine
([`../pixtuoid-scene/AGENTS.md`](../pixtuoid-scene/AGENTS.md)) are `src/tui/`
([`src/tui/AGENTS.md`](src/tui/AGENTS.md)) and `floating/` — neither depends
on the other. Module map: `ls src/` — each file's `//!` header is its
annotation. Cross-cutting rules: workspace [`AGENTS.md`](../../AGENTS.md).
