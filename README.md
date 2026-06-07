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
  <a href="https://ivanwng97.github.io/pixtuoid/"><strong>🖥&#xFE0E; Live demo ↗</strong></a>
  &nbsp;·&nbsp; <a href="https://ivanwng97.github.io/pixtuoid/architecture">Architecture</a>
  &nbsp;·&nbsp; <a href="https://ivanwng97.github.io/pixtuoid/config">Configuration</a>
  &nbsp;·&nbsp; <a href="https://ivanwng97.github.io/pixtuoid/contributing">Contributing</a>
</p>

---

## Why?

Running several coding agents means alt-tabbing between terminals to find out who's stuck, who's waiting on a permission prompt, and who finished ten minutes ago. **pixtuoid** puts them all in one tiny pixel-art office you can watch from above — every session is a character at a desk: typing while it works, raising a `?` when it needs you, dozing off when it's done.

A little bit *Black Mirror*, a little bit *The Sims* — and the most glanceable multi-agent dashboard you'll ever use.

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

**Both packages are required** — `pixtuoid` (the visualizer) and `pixtuoid-hook` (the shim your agent invokes).

### Pre-built binaries

Download from [GitHub Releases](https://github.com/IvanWng97/pixtuoid/releases/latest):

| Platform | Tarball |
|---|---|
| macOS (Apple Silicon) | `pixtuoid-v*-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `pixtuoid-v*-x86_64-apple-darwin.tar.gz` |
| Linux (x86_64, static) | `pixtuoid-v*-x86_64-unknown-linux-musl.tar.gz` |
| Linux (ARM64) | `pixtuoid-v*-aarch64-unknown-linux-gnu.tar.gz` |

Debian/Ubuntu `.deb`s (amd64/arm64) are on the same page — install the binary
**and** the hook shim:

```bash
sudo dpkg -i pixtuoid_*.deb pixtuoid-hook_*.deb
```

### Cargo

```bash
cargo install pixtuoid pixtuoid-hook
```

### Windows (experimental)

Requires [Windows Terminal](https://aka.ms/terminal) (the Windows 11 default).
Claude Code only for now — Codex support is coming.

1. Download `pixtuoid-<version>-x86_64-pc-windows-msvc.zip` from the
   [latest release](https://github.com/IvanWng97/pixtuoid/releases)
2. Unblock before extracting (the exes are unsigned — SmartScreen marks
   downloads): right-click the zip → Properties → Unblock, or
   `Unblock-File .\pixtuoid-*.zip` in PowerShell
3. Extract anywhere and add the folder to your PATH
4. `pixtuoid install-hooks` then `pixtuoid run`

ARM64 Windows: use the `aarch64-pc-windows-msvc` zip.

### From source

```bash
git clone https://github.com/IvanWng97/pixtuoid && cd pixtuoid
just build --release
```

Upgrading from `ascii-agents` v0.3.x? See [docs/MIGRATION.md](docs/MIGRATION.md).

</details>

## Features

<!-- features:start · generated from site/src/features.json by `just gen-readme` — edit the JSON, not this table -->
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
<!-- features:end -->

<p align="center">
  <a href="https://ivanwng97.github.io/pixtuoid/#showcase"><strong>▶ See every feature live — floors, themes, weather, pets, the office tour →</strong></a>
</p>

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

Press `t` to cycle the built-in themes with live preview. Your choice persists across sessions:

<p align="center">
  <img src="docs/images/themes-composite.png" alt="built-in themes side by side" width="800" />
</p>

Settings live in `~/.config/pixtuoid/config.toml` — theme, desk cap, custom pet
names, and sprite packs. CLI flags override the file (`pixtuoid run --theme dracula`).
See **[docs/CONFIGURATION.md](docs/CONFIGURATION.md)** for the full key reference
(defaults, system-managed keys), the custom sprite-pack workflow, and **logging /
troubleshooting** (the TUI writes warnings to `~/.cache/pixtuoid/log`) — or browse it
live at **[/config](https://ivanwng97.github.io/pixtuoid/config)**.

## How It Works

Agent CLIs emit events two ways — a hook shim (a 200ms fire-and-forget Unix-socket write that can never block your agent) and JSONL transcript watching. Both feed one channel; a reducer folds events into office state; the renderer draws it as half-block pixel art. Three Rust crates, zero terminal deps in the core.

**[Full architecture with diagrams →](https://ivanwng97.github.io/pixtuoid/architecture)** · single source: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)

## Contributing

PRs welcome — especially new themes and `Source` adapters for other agent CLIs (Copilot, Cursor, OpenCode). See **[CONTRIBUTING.md](docs/CONTRIBUTING.md)** for the build/test workflow, conventions, the review process, and how to add a new agent CLI. Architecture and the load-bearing invariants live in [`CLAUDE.md`](CLAUDE.md).

## Acknowledgments

Inspired by [`pixel-agents`](https://github.com/pablodelucca/pixel-agents) (VS Code), [`clawd-on-desk`](https://github.com/rullerzhou-afk/clawd-on-desk) (desktop pet), and Claude Code's [Buddy](https://dev.to/picklepixel/how-i-reverse-engineered-claude-codes-hidden-pet-system-8l7).

## License

[MIT](LICENSE)

## Star History

<p align="center">
  <a href="https://www.star-history.com/?repos=IvanWng97%2Fpixtuoid&type=date&legend=top-left">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=IvanWng97/pixtuoid&type=Date&theme=dark" />
      <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=IvanWng97/pixtuoid&type=Date" />
      <img alt="star history chart for IvanWng97/pixtuoid" src="https://api.star-history.com/svg?repos=IvanWng97/pixtuoid&type=Date" width="640" />
    </picture>
  </a>
</p>

<p align="center">
  <sub>Enjoying the little office? <a href="https://buymeacoffee.com/IvanWng97">☕ Buy me a coffee</a> · <a href="https://github.com/IvanWng97/pixtuoid">⭐ Star the repo</a></sub>
</p>
