# pixtuoid (binary) — agent guide

The **TUI binary**: `ratatui` + `crossterm` + `winit` + `tokio` + `clap`. Wires
sources → reducer → renderer, owns the CLI subcommands, hook installation, config
persistence, and multi-floor orchestration. The backend-agnostic render +
simulation **engine** (layout, pose/motion/pathfinding, the pixel pass, the theme
model, pets, chitchat) is its OWN dependency crate `pixtuoid-scene` (it used to be
an in-binary module) — see [`../pixtuoid-scene/CLAUDE.md`](../pixtuoid-scene/CLAUDE.md);
the DAG is `pixtuoid-core ← pixtuoid-scene ← {pixtuoid, pixtuoid-web}`. This
binary's two thin painters **over the `pixtuoid-scene` crate** are the terminal
renderer `src/tui/` ([`src/tui/CLAUDE.md`](src/tui/CLAUDE.md)) and the
`floating/` desktop window (neither depends on the other); the wasm `<canvas>`
painter is the SIBLING crate `pixtuoid-web`. Cross-cutting rules: workspace
[`CLAUDE.md`](../../CLAUDE.md); headless-lib detail:
[`../pixtuoid-core/CLAUDE.md`](../pixtuoid-core/CLAUDE.md).

## Layout

Annotated tree in [`LAYOUT.md`](LAYOUT.md) — grep it for a filename.

```
src/
├── main.rs             entry point — arg-parse + dispatch + env glue ONLY …
├── crash.rs            install_crash_hook — panic hook → terminal restore …
├── logging.rs          log routing (#157): logging::init installs the ONE …
├── cli.rs              clap subcommands (run / floating / validate-pack / …
├── term.rs             truecolor preflight — does NOT guess from a $TERM name …
├── setup.rs            first-run detection for onboarding: the PURE …
├── sources.rs          the TUI-free source-control CORE …
├── sources_cli.rs      the scriptable sources-CLI PRESENTERS over crate::sources …
├── doctor.rs           `pixtuoid doctor` — read-only source self-diagnosis …
├── focus/              FOCUS-JUMP (click a sprite / dashboard `f` → the agent's …
├── config/             AppConfig persistence (~/.config/pixtuoid/config.toml) …
├── runtime/            mod.rs (RunConfig, boot-capacity math, headless summarize …
├── init_pack.rs        extracts the embedded skeleton pack to a target dir for …
├── validate.rs         the `validate-pack` presenter; pack.name/version are …
├── version.rs          pure version-popup boot logic
├── aa_text.rs          THE anti-aliased text rasterizer — every rasterized text …
├── audio/              ambient office sound (#633) — THE one consumer of …
├── fonts/              MonaspaceNeon-SemiBold.otf + OFL-Monaspace.txt (the ONE …
├── install/            multi-target (Claude + Codex + Reasonix + CodeWhale + …
├── floating/           `pixtuoid floating` — the frameless, always-on-top …
└── tui/                ratatui App + TuiRenderer (inherent `render` flush) — the …

sprites/                character/environment packs (NOT under pixtuoid-hook; the …
├── robot/              proof-of-concept TV-head robot pack (loadable via …
└── skeleton/           template pack for custom sprite creation (embedded via …
```

## Known sharp edges (don't be surprised by these)

Full entries in [`SHARP-EDGES.md`](SHARP-EDGES.md) — grep it for the phrase.

- Windows focus-jump borrows the foreground thread's input state, and the two BETTER-KNOWN bypasses are refused on purpose.
- `--graphics off` answers TWO questions at once, and the second one is the cutaway's — but it is a `doctor` flag, and NOTHING paints the cutaway yet.
- Capacity GROWTH strands an already-allocated overflow agent, and the HUD keeps counting it — render-conservatively, count-completely.
- Terminal cell aspect drives sprite design.
- `--max-desks` has no hard default.
- The floating pipeline boots in `resumed`, NOT in `floating::run` — the window has to exist before the desk capacity can be seeded.
- Re-install is a SEMANTIC no-op, and backups APPEND their suffix.
- Daemon presence is ANNOUNCE-only, so a gateway pixtuoid never heard announce is invisible until its next activity — a documented residual, NOT a bug to "fix" with a poll.
- Connecting OpenClaw is NOT the last step — a RUNNING gateway must restart, and the presenters say so.
- `connect openclaw` binds ONE OpenClaw state dir — and that ONE install covers EVERY gateway of that profile, however many.
- OpenClaw's config is JSON5, so pixtuoid REFUSES to rewrite a non-strict document instead of "fixing" it.
- Two surfaces bind a source, ONE core.
- `OutcomeRow` is `{id, outcome, message?}` — a bare machine token + a SEPARATE optional detail field.
- Code-artifact targets: install writes ⊆ verify checks, CONTENT included (#387).
- `doctor`'s `<cli> --version` probes are NOT side-effect-free on the other side, so they are GATED on presence.

## Where to look

Answers live in [`WHERE-TO-LOOK.md`](WHERE-TO-LOOK.md), so a session
pays for the entry it needs instead of all of them. Grep it for the
question:

- How do hooks get installed?
- How does the default character pack get into the binary?
- How do custom sprite packs work?
- How does the crash log work?
- Where do runtime errors / config warnings surface?
- How does config persistence work?
- How do multi-floor offices work?

