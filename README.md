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
  <a href="https://pixtuoid.dev/"><strong>🖥&#xFE0E; Live demo ↗</strong></a>
  &nbsp;·&nbsp; <a href="https://pixtuoid.dev/architecture">Architecture</a>
  &nbsp;·&nbsp; <a href="https://pixtuoid.dev/config">Configuration</a>
  &nbsp;·&nbsp; <a href="https://pixtuoid.dev/contributing">Contributing</a>
</p>

---

## Why?

Running several coding agents means alt-tabbing between terminals to find out who's stuck, who's waiting on a permission prompt, and who finished ten minutes ago. **pixtuoid** puts them all in one tiny pixel-art office you can watch from above — every session is a character at a desk: typing while it works, raising a `?` when it needs you, dozing off when it's done.

A little bit *Black Mirror*, a little bit *The Sims* — and the most glanceable multi-agent dashboard you'll ever use.

## Quick Start

Pick one — Homebrew on macOS, or npm on any OS:

<!-- install:start · generated from site/src/install.json by `just gen-readme` — edit the JSON, not this block -->
**Homebrew** (macOS):

```bash
brew install IvanWng97/pixtuoid/pixtuoid
```

**npm** (any OS):

```bash
npm install -g pixtuoid
```
<!-- install:end -->

Then launch:

```bash
pixtuoid
```

Press `s` to open the **Sources** panel and connect your agent CLI (Claude Code, Codex, Antigravity, Reasonix, …) — pixtuoid wires up the integration for you, no separate install step. In another terminal, start that coding agent. A character walks in from the elevator within a second; disconnect in the same panel and it walks back out. The panel also flags a source whose hooks are connected but broken (run `pixtuoid doctor` for the full health report).

**Keyboard shortcuts:** `q` quit · `p` pause · `s` sources (connect / health) · `t` themes · `m` sound (`+`/`-` volume) · `Tab` agent dashboard · `?` help · `↑↓/jk/PgUp/PgDn` floors · click an agent to bring its terminal to the front (`f` in the dashboard)

**More ways to install** — Cargo, prebuilt binaries, and Debian `.deb`s — are on the **[install guide ↗](https://pixtuoid.dev/#install)**.

## Features

<!-- features:start · generated from site/src/features.json by `just gen-readme` — edit the JSON, not this table -->
| | Feature | Description |
|---|---|---|
| <img src="docs/images/pix-icons/multiagent.png" alt="" width="50" height="50"> | **Multi-agent office** | Every agent session gets its own desk — when a floor fills up, a new floor opens automatically |
| <img src="docs/images/pix-icons/multifloor.png" alt="" width="50" height="50"> | **Multi-floor office** | Hop between floors with `PageUp`/`PageDown`, `↑`/`↓`, or `j`/`k` — each switch slides into view |
| <img src="docs/images/pix-icons/spaces.png" alt="" width="50" height="50"> | **Office spaces** | Cubicles, a meeting lounge, and a pantry — the office is laid out in distinct furnished zones, not just a grid of identical desks |
| <img src="docs/images/pix-icons/walk.png" alt="" width="40" height="60"> | **Animated characters** | Coworkers type, wait with a `?`, sleep under little z's, and walk A\*-routed paths between desks |
| <img src="docs/images/pix-icons/palette.png" alt="" width="50" height="50"> | **Team palette** | Shirt and pants take their colors from the working directory — same repo, same colors, so the room reads like an org chart. Hair and skin vary per agent; 16 curated outfits |
| <img src="docs/images/pix-icons/glow.png" alt="" width="50" height="50"> | **Per-tool monitor glow** | Each desk's monitor glows with the tool in use — Edit blue, Bash orange, Read cyan — so you can read the whole room at a glance |
| <img src="docs/images/pix-icons/tokens.png" alt="" width="50" height="50"> | **Token meter** | Paper stacks up on a desk as its session burns tokens — the pile climbs through 250K / 2M / 16M tiers, a big spend drops a fresh sheet, and hovering shows the exact total (Σ) |
| <img src="docs/images/pix-icons/magnify.png" alt="" width="50" height="50"> | **Hover tooltips** | Hover an agent for session duration, tool-call count and active-time %; hover any furniture — desks, sofas, plants, vending machine, printer — for its name |
| <img src="docs/images/pix-icons/tree.png" alt="" width="50" height="50"> | **Agent tree dashboard** | Tab opens a collapsible tree of every floor's agents — each badged with the CLI it runs, color-tinted by what it's doing, with tool-call counts |
| <img src="docs/images/pix-icons/pets.png" alt="" width="50" height="50"> | **Office pets** | A cat or dog (one per floor) roams desks, pantry, sofas; sleeps near idle agents. Click to pet — pixel-art hearts float up |
| <img src="docs/images/pix-icons/lobster.png" alt="" width="70" height="60"> | **OpenClaw gateway mascot** | A live OpenClaw gateway shows up as a wandering lobster — the way it moves shows the gateway's health |
| <img src="docs/images/pix-icons/vibes.png" alt="" width="50" height="50"> | **Office vibes** | The sun and moon cross the skyline as the day goes by, weather rolls past the windows — rain, storm, snow, fog, overcast, windy, smog — and six themes give the office a whole new look |
| <img src="docs/images/pix-icons/note.png" alt="" width="50" height="50"> | **Lofi soundtrack** | A lofi soundtrack synthesized entirely in code — no audio files shipped. Day and night tracks follow the office's clock and weather, typing sounds swell with activity, and the door chime, printer and vending machine play as coworkers come and go. `m` mutes, `+`/`-` volume |
| <img src="docs/images/pix-icons/window.png" alt="" width="50" height="50"> | **Floating desktop window** | `pixtuoid floating` opens the office in a frameless, always-on-top window — on your desktop, not just in your terminal |
| <img src="docs/images/pix-icons/shield.png" alt="" width="50" height="50"> | **Hook-safe** | The tiny hook shim pixtuoid installs always exits 0 — even a stuck office can never block your agent |
<!-- features:end -->

<p align="center">
  <a href="https://pixtuoid.dev/#showcase"><strong>▶ See every feature live — floors, themes, weather, pets, the office tour →</strong></a>
</p>

## Supported Tools

<!-- tools:start · generated from site/src/sources.json by `just gen-readme` — edit the JSON, not this table -->
| Tool | Runs on |
|---|---|
| [Claude Code](https://code.claude.com) | macOS · Linux · Windows\* |
| [Codex CLI](https://github.com/openai/codex) | macOS · Linux · Windows\* |

_Also supported: [Antigravity CLI](https://github.com/antiGravity-AI/antigravity-cli), [DeepSeek-Reasonix](https://github.com/esengine/DeepSeek-Reasonix), [CodeWhale](https://github.com/Hmbown/CodeWhale), [Copilot CLI](https://github.com/github/copilot-cli), [opencode](https://github.com/anomalyco/opencode), [Cursor CLI](https://cursor.com/cli), [Hermes Agent](https://hermes-agent.nousresearch.com), [Oh My Pi](https://omp.sh), [OpenClaw](https://github.com/openclaw/openclaw), [Grok Build](https://github.com/xai-org/grok-build)._

**→ [Full tool × OS support matrix on the site](https://pixtuoid.dev/#tools)**

_\* experimental — limited testing, unsigned binaries._
<!-- tools:end -->

> Adding a new tool? Implement the [`Source` trait](#contributing) — or, for a hook-only CLI, just a hook decoder + an install `Target` — then add a row to [`site/src/sources.json`](site/src/sources.json) (its `supported` set is pinned to the code by a test). One file, one channel, done.

## Configuration

Everything lives in `~/.config/pixtuoid/config.toml` (created on first launch;
every key optional) — theme, desk cap, custom pet names, and sprite packs. CLI
flags override the file (`pixtuoid run --theme dracula`).

The setting you'll reach for most is the **theme** — press `t` in the TUI for a
live-preview picker across six built-in palettes; your pick persists across sessions.

<p align="center">
  <img src="docs/images/themes-composite.png" alt="the six built-in themes side by side" width="800" />
</p>

See **[docs/CONFIGURATION.md](docs/CONFIGURATION.md)** for the full key reference
(defaults, system-managed keys), the custom sprite-pack workflow, and **logging /
troubleshooting** (diagnostics go to `~/.cache/pixtuoid/log`) — or browse it live
at **[/config](https://pixtuoid.dev/config)**.

## How It Works

Agent CLIs emit events two ways — a hook shim (a 200ms fire-and-forget write to a Unix socket, or a named pipe on Windows, that can never block your agent) and JSONL transcript watching. Both feed one channel; a reducer folds events into office state; the renderer draws it as half-block pixel art. Five Rust crates, zero terminal deps in the core.

**[Full architecture with diagrams →](https://pixtuoid.dev/architecture)** · single source: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)

## Privacy & Security

pixtuoid is **local-only and telemetry-free** — it makes no network connections,
ships no analytics or "phone home", and reads your agent transcripts read-only to
animate the office. Your session data never leaves your machine. The dependency
set is audited for advisories daily (`cargo-deny`). For the trust boundaries (the
hook shim, the owner-only socket, and how hook installation edits another tool's
config), see **[SECURITY.md](SECURITY.md)**.

## Contributing

PRs welcome — especially new themes, sprite/decoration polish, and `Source` adapters for agent CLIs we don't support yet (the ten agent CLIs plus the OpenClaw gateway already wired up are in [Supported Tools](#supported-tools)). See **[CONTRIBUTING.md](docs/CONTRIBUTING.md)** for the build/test workflow, conventions, the review process, and how to add a new agent CLI. Architecture and the load-bearing invariants live in [`CLAUDE.md`](CLAUDE.md).

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
