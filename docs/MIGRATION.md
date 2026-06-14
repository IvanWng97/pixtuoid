# Migration

Migrating from `ascii-agents` (v0.3.x → v0.4.0) — rename, hooks, config paths.

**v0.4.0 renamed the project from `ascii-agents` to `pixtuoid`.**

## What changed

| Before (v0.3.x) | After (v0.4.0) |
|---|---|
| `ascii-agents` binary | `pixtuoid` |
| `ascii-agents-hook` shim | `pixtuoid-hook` |
| `~/.config/ascii-agents/` | `~/.config/pixtuoid/` |
| `~/.cache/ascii-agents/` | `~/.cache/pixtuoid/` |
| `/tmp/ascii-agents-{uid}.sock` | `/tmp/pixtuoid-{uid}.sock` |
| `_ascii_agents` hook key in `settings.json` | `_pixtuoid` |

## Upgrade steps

1. **Install the new version:**
   ```bash
   brew untap IvanWng97/ascii-agents 2>/dev/null
   brew install IvanWng97/pixtuoid/pixtuoid
   # or: cargo install pixtuoid pixtuoid-hook
   ```

2. **Re-register hooks**: launch `pixtuoid`, press `c` to open the Connection
   panel, and connect your agent CLI (this replaces any old `ascii-agents-hook`
   entries automatically). The old `pixtuoid install-hooks` subcommand is gone —
   binding a source is now done live inside the TUI.

3. **Migrate config** (optional — only if you customized `config.toml`):
   ```bash
   mkdir -p ~/.config/pixtuoid
   mv ~/.config/ascii-agents/config.toml ~/.config/pixtuoid/config.toml
   ```

> **GitHub links:** The old `IvanWng97/ascii-agents` URL automatically redirects to `IvanWng97/pixtuoid`. Existing bookmarks and stars carry over.
