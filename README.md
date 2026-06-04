<p align="center">
  <img src="docs/images/sprite-banner.png" alt="pixtuoid sprites" width="500" />
</p>

<h1 align="center">pixtuoid</h1>

<p align="center">
  <em>Your AI coding agents, visualized as pixel-art coworkers in a terminal office.</em>
</p>

<p align="center">
  <sub><em><b>pix</b>el + <b>tu</b>i + (agent-)<b>oid</b></em></sub>
</p>

<p align="center">
  <a href="https://github.com/IvanWng97/pixtuoid/stargazers"><img src="https://img.shields.io/github/stars/IvanWng97/pixtuoid?style=flat-square" alt="Stars" /></a>
  <a href="https://github.com/IvanWng97/pixtuoid/releases"><img src="https://img.shields.io/github/v/release/IvanWng97/pixtuoid?label=version&style=flat-square" alt="Version" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square" alt="License" /></a>
  <a href="https://github.com/IvanWng97/pixtuoid/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/IvanWng97/pixtuoid/ci.yml?style=flat-square&label=CI" alt="CI" /></a>
  <a href="https://codecov.io/gh/IvanWng97/pixtuoid"><img src="https://img.shields.io/codecov/c/github/IvanWng97/pixtuoid?style=flat-square" alt="Coverage" /></a>
  <a href="https://claude.ai/code"><img src="https://img.shields.io/badge/Built%20with-Claude%20Code-blueviolet?style=flat-square&logo=anthropic" alt="Built with Claude Code" /></a>
  <a href="https://buymeacoffee.com/IvanWng97"><img src="https://img.shields.io/badge/Buy%20Me%20a%20Coffee-ffdd00?style=flat-square&logo=buy-me-a-coffee&logoColor=black" alt="Buy Me a Coffee" /></a>
</p>

<p align="center">
  <img src="docs/images/demo.gif" alt="pixtuoid animated demo" width="800" />
</p>

<p align="center">
  <a href="#quick-start">Quick Start</a> · <a href="#features">Features</a> · <a href="#supported-tools">Supported Tools</a> · <a href="#themes--configuration">Themes & Configuration</a> · <a href="#how-it-works">How It Works</a>
</p>

---

## Why?

Running multiple AI agents in the terminal is like managing a sweatshop you can't see. They type, they wait, they finish — and you have no idea who's doing what unless you scroll through logs like a bureaucrat.

**pixtuoid** puts them all in a tiny pixel-art office you can watch from above. A little bit *Black Mirror*, a little bit *The Sims* — and somehow the most intuitive multi-agent dashboard you'll ever use.

## Who's it for?

- You're running 3+ AI coding agents and losing track of who's stuck on what.
- You want a glanceable activity signal without tailing logs.
- You think coding agents should have a vibe.

## Quick Start

```bash
brew install IvanWng97/pixtuoid/pixtuoid
pixtuoid install-hooks
pixtuoid
```

In another terminal, start a supported coding agent (Claude Code, Codex, Antigravity, …). A character walks in from the elevator within a second.

**Keyboard shortcuts:** `q` quit · `p` pause · `t` themes · `?` help · `↑↓/jk/PgUp/PgDn` floors · click to pin tooltip

<details>
<summary><strong>More install methods</strong></summary>

### Pre-built binaries

Download from [GitHub Releases](https://github.com/IvanWng97/pixtuoid/releases/latest):

| Platform | Tarball |
|---|---|
| macOS (Apple Silicon) | `pixtuoid-v*-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `pixtuoid-v*-x86_64-apple-darwin.tar.gz` |
| Linux (x86_64, static) | `pixtuoid-v*-x86_64-unknown-linux-musl.tar.gz` |
| Linux (ARM64) | `pixtuoid-v*-aarch64-unknown-linux-gnu.tar.gz` |

### Cargo

```bash
cargo install pixtuoid pixtuoid-hook
```

### From source

```bash
git clone https://github.com/IvanWng97/pixtuoid && cd pixtuoid
just build --release
```

</details>

## Features

| | Feature | Description |
|---|---|---|
| 🏢 | **Multi-agent office** | Each agent session gets a desk; overflow agents auto-fill new floors |
| 🛗 | **Multi-floor office** | PageUp/PageDown/↑↓/jk to navigate floors with slide transition |
| 🎭 | **Animated characters** | Typing, waiting (`?`), sleeping (z's), walking with A\*-routed pathfinding |
| 💡 | **Per-tool monitor glow** | Edit = blue, Bash = orange, Read = cyan — scannable at a glance |
| 🎨 | **Per-agent identity** | Deterministic shirt/hair/skin palette from session hash, 16 curated outfits |
| 🌧️ | **Weather effects** | Rain, storm, snow, fog, overcast, windy — cycles every 10 min + sunset golden hour |
| 📊 | **Tooltip stats** | Hover any agent to see session duration, tool call count, and active time % |
| 🏷️ | **Furniture tooltips** | Hover any item — desks, sofas, plants, vending machine, printer — to see its name |
| 🐾 | **Office pets** | A cat or dog (one per floor) roams desks, pantry, sofas; sleeps near idle agents. Click to pet — pixel-art hearts float up |
| ☕ | **Coffee run** | Idle agents visit the pantry, carry a cup back to their desk. Cup stays while you work; taken on exit |
| 💬 | **Pantry chitchat** | 2+ idle agents at the same waypoint trigger speech bubbles with dev-humor snippets |
| 🛡️ | **Hook-safe** | The shim always exits 0 — a stuck visualizer can never block your agent |

## Supported Tools

| Tool | Status | Notes |
|---|---|---|
| [**Claude Code**](https://code.claude.com) | ✅ Supported | Hook shim + JSONL watcher |
| [**Antigravity CLI**](https://github.com/antiGravity-AI/antigravity-cli) | ✅ Supported | JSONL watcher |
| [**Codex CLI**](https://github.com/openai/codex) | ✅ Supported | Hook shim + JSONL watcher (hook/JSONL coalesce on session UUID) |
| [**Copilot CLI**](https://github.com/github/copilot-cli) | 🔜 Planned | Identical event names |
| [**OpenCode**](https://github.com/anomalyco/opencode) | 🔜 Planned | Any LLM (DeepSeek / GPT / Claude / Gemini) |
| [**Cursor CLI**](https://cursor.com/cli) | 🔜 Planned | NDJSON stream |

> Adding a new tool? Implement the [`Source` trait](#contributing) — one file, one channel, done.

## Themes & Configuration

Press `t` to switch themes with live preview. Your choice persists across sessions. 6 built-in:

<p align="center">
  <img src="docs/images/themes-composite.png" alt="6 themes: Normal, Cyberpunk, Dracula, Tokyo Night, Catppuccin, Gruvbox" width="800" />
</p>

Settings live in `~/.config/pixtuoid/config.toml` — theme, desk cap, custom pet
names, and sprite packs. CLI flags override the file (`pixtuoid run --theme dracula`).
See **[docs/CONFIGURATION.md](docs/CONFIGURATION.md)** for the full key reference
(defaults, system-managed keys) and the custom sprite-pack workflow.

## How It Works

<details>
<summary><strong>Architecture</strong></summary>

```
agent tool call ──► agent fires hook ──► pixtuoid-hook (shim)
                                         │ JSON over Unix socket
                                         ▼
                                  /tmp/pixtuoid-{uid}.sock
                                         │
                       HookSocketListener ─────► ┐
                                                 │ (Transport, AgentEvent)
                       JsonlWatcher       ─────► ┤ shared mpsc channel
                                                 ▼
                       Reducer ──► SceneState (watch channel)
                                         │
                       TuiRenderer ──► draw_scene @ ~30fps
                       (pose → pixel_painter → RgbBuffer → half-block → ratatui)
```

Three Rust crates:

| Crate | Role |
|---|---|
| **pixtuoid-core** | Headless library — no terminal deps. Source trait, reducer, pose, layout, sprites. |
| **pixtuoid** | TUI binary — ratatui + crossterm + tokio. Half-block rendering + theme system. |
| **pixtuoid-hook** | Tiny shim the agent CLI invokes from hooks. 200ms timeout, always exits 0. |

</details>

<details>
<summary><strong>Migrating from <code>ascii-agents</code> (v0.3.x → v0.4.0)</strong> — rename, hooks, config paths</summary>

**v0.4.0 renamed the project from `ascii-agents` to `pixtuoid`.**

### What changed

| Before (v0.3.x) | After (v0.4.0) |
|---|---|
| `ascii-agents` binary | `pixtuoid` |
| `ascii-agents-hook` shim | `pixtuoid-hook` |
| `~/.config/ascii-agents/` | `~/.config/pixtuoid/` |
| `~/.cache/ascii-agents/` | `~/.cache/pixtuoid/` |
| `/tmp/ascii-agents-{uid}.sock` | `/tmp/pixtuoid-{uid}.sock` |
| `_ascii_agents` hook key in `settings.json` | `_pixtuoid` |

### Upgrade steps

1. **Install the new version:**
   ```bash
   brew untap IvanWng97/ascii-agents 2>/dev/null
   brew install IvanWng97/pixtuoid/pixtuoid
   # or: cargo install pixtuoid pixtuoid-hook
   ```

2. **Re-register hooks** (replaces old `ascii-agents-hook` entries automatically):
   ```bash
   pixtuoid install-hooks
   ```

3. **Migrate config** (optional — only if you customized `config.toml`):
   ```bash
   mkdir -p ~/.config/pixtuoid
   mv ~/.config/ascii-agents/config.toml ~/.config/pixtuoid/config.toml
   ```

> **GitHub links:** The old `IvanWng97/ascii-agents` URL automatically redirects to `IvanWng97/pixtuoid`. Existing bookmarks and stars carry over.

</details>

## Contributing

PRs welcome — especially new themes and `Source` adapters for other agent CLIs (Copilot, Cursor, OpenCode). See **[CONTRIBUTING.md](CONTRIBUTING.md)** for the build/test workflow, conventions, the review process, and how to add a new agent CLI. Architecture and the load-bearing invariants live in [`CLAUDE.md`](CLAUDE.md).

## Acknowledgments

Inspired by [`pixel-agents`](https://github.com/pablodelucca/pixel-agents) (VS Code), [`clawd-on-desk`](https://github.com/rullerzhou-afk/clawd-on-desk) (desktop pet), and Claude Code's [Buddy](https://dev.to/picklepixel/how-i-reverse-engineered-claude-codes-hidden-pet-system-8l7).

## Support

If you enjoy pixtuoid, consider [buying me a coffee](https://buymeacoffee.com/IvanWng97) :)

## License

[MIT](LICENSE)
