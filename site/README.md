# pixtuoid — website

The marketing landing page for [pixtuoid](https://github.com/IvanWng97/pixtuoid),
built with [Astro](https://astro.build). Deploys to GitHub Pages at
**https://ivanwng97.github.io/pixtuoid/**.

Self-contained: this is a Node project that lives in `site/` and is independent
of the Rust workspace. Its CI (`.github/workflows/site.yml`) runs the same checks
as `npm run verify`; deploys run via `.github/workflows/pages.yml`.

## Develop

```sh
npm install        # or: just site-setup   (from the repo root)
npm run dev        # http://localhost:4321/pixtuoid/   ·  just site-dev
```

**Agent-driven (non-TTY) dev server** — foreground `astro dev` exits on stdin
EOF, so it dies between an agent's shell commands. Astro 7's background mode
fixes this (dev-server only; `astro preview` has none of it):

```sh
just site-dev-bg       # astro dev --background, then polls /_astro/status until ready
just site-dev-stop     # astro dev stop (frees port 4321; no-op if not running)
npx astro dev status   # from site/: is the daemon up? · logs: npx astro dev logs --follow
```

Readiness/liveness is the dev-only `/_astro/status` endpoint (`{"ok":true}`,
served at the root and under the base path). In a non-TTY context astro
auto-switches to JSON log lines — no logger config needed. Stop the daemon
before `just site-e2e`: dev and preview share port 4321, and the e2e webServer
fails loud on a squatted port by design.

## Quality gates

```sh
npm run verify     # format:check → lint → astro check → knip → build  (== site CI)
# individually:
npm run format     # prettier --write .
npm run lint       # eslint .
npm run check       # astro check (types + templates)
npm run knip       # unused files / exports / dependencies
npm run build      # astro build → dist/
npm run e2e        # Playwright smoke suite (tests/e2e/) vs the PRODUCTION build
```

From the repo root the same gate is `just site-check` (and `just site-fmt`);
`just site-e2e` builds then runs the Playwright suite — the runtime-contract
tier (window seams, scrollspy keys, dimmer, reduced-motion) that the static
gates can't see. CI runs it in `site.yml` after the build step.

> **Cross-boundary build inputs.** The site reads six files from _outside_ `site/`
> at build time: the workspace `Cargo.toml` (displayed version, via `vite.define` in
> `astro.config.mjs`), `docs/CONFIGURATION.md` (rendered as `/config`),
> `docs/ARCHITECTURE.md` (rendered as `/architecture` — its Mermaid diagram becomes an
> inline SVG at build via rehype-mermaid, which is why CI installs Chromium),
> `docs/CONTRIBUTING.md` (rendered as `/contributing`), `docs/KNOWLEDGE-ENGINEERING.md` (rendered as
> `/knowledge-base` — the route slug kept from its `KNOWLEDGE-BASE.md` days, no link
> rot), and `docs/PARALLEL-DELIVERY.md` (rendered as `/parallel-delivery`).
> Renaming/moving any of them — or breaking the diagram's Mermaid syntax — fails
> `astro build`; all six are in the `site.yml` / `pages.yml` path filters so a
> change re-runs CI + redeploys. The root `README.md`'s Features table and install
> commands are sourced from `src/features.json` / `src/install.json` (see below);
> drift is gated by the `readme` job in `.github/workflows/ci.yml` (`just gen-readme-check`)
> on every PR, not by `site.yml`.

> **Generated README sections.** `src/features.json` (feature inventory — also
> drives the Features bento), `src/sources.json` (supported tools — also drives the
> tool × OS matrix), and `src/install.json` (install methods — also drives the
> Install tabs) are single sources shared with the **root README**:
> `scripts/gen-readme.mjs` regenerates the Features table, the supported-tools
> glimpse, and the install block between their markers. The install block shows
> only methods flagged `"readme": true` (Homebrew, npm); the rest (Cargo, GitHub
> Releases) stay site-only. Edit the JSON → run `just gen-readme`; CI's `readme`
> job runs `just gen-readme-check` and fails on drift.

## Design

- **Layout/type** — "Cozy Terminal": Jersey 10 (pixel display) · JetBrains Mono
  (UI/code) · Lora (body); ASCII dividers, blinking cursor, CRT scanlines.
- **Palette** — warm "Coworking" (cream lifted from the office carpet + Claude
  coral). Day = cream, night = after-hours. Until the visitor picks a theme the
  site follows their wall clock like the app does (19:00–07:00 → night; the
  clock only ever turns night ON — a dark-preference system keeps night at noon
  via the `prefers-color-scheme` fallback). `dracula` is a hidden easter-egg
  theme (type it, or `?theme=dracula`).
- **The building (OPEN FLOOR)** — the page is one continuous camera hold on the
  REAL office: `OfficeBackdrop.astro` runs the `pixtuoid-web` wasm engine as a
  fixed full-viewport canvas (poster-first; reduced-motion / no-JS / any
  failure stays on the still), and scrolling is the light switch — a `#dimmer`
  sheet darkens toward statements and releases to 0 in full-viewport
  "office gaps" (the hero caps its dim lower via `data-lit-max` so the live
  office reads above the fold). Each section is a floor (6F penthouse → 1F front desk);
  `Statusline.astro` is the one piece of fixed chrome (floor readout +
  scrollspy, the build-time merged-PR feed with a canned-reel fallback,
  `lights %` / clock / `● LIVE` · `❚❚ PAUSED`), and the app's literal keys work
  on the page — digits `1–6` ride between floors, `t` retints the decorative
  palette (Esc restores). Those bare single-char shortcuts are **focus-gated**
  (WCAG 2.1.4): they fire only from a neutral focus context (page body / a
  jumped-to section / the statusline), never a focused control — the shared
  `window.__pixKeys.shortcutContext()` gate. The favicon dims with the night
  theme; 404 is the office at 3 a.m. (`--empty` render).
- **Cross-component seams** (what a new section must wire): sections declare
  `data-lit` (dimmer target; `data-lit="fade"` also rises with the darkness)
  and `data-floor` (scrollspy) — a new floor is a `FLOORS` row in `consts.ts`
  (the ONE source the statusline lift AND each section's `data-floor`/id/eyebrow
  derive from), then `data-lit` on the section. The backdrop publishes
  `window.__pixLights` (per-frame dim value; the statusline polls it), `pix:onair`
  - the `.backdrop.is-live` class (discrete live flip; event for changes, class
    for late-attach seeding), and `pix:paused` (the office pause switch — see FX)
    — every >5s auto-motion listens, so ONE control governs page motion. Plus
    `window.__pixHire()` (walk one extra sprite in; the install Copy buttons call
    it). The boot handshake: `Base.astro` publishes `window.__pixRevealed` (+ the
    `pix:revealed` event) when the terminal boot splash lifts — that RELEASES the
    office's floor-roll reveal; `OfficeBackdrop` publishes `window.__pixEngineReady`
    when the engine resolves (live / failed / unsupported), which the splash's
    Level-2 gate polls so it lifts STRAIGHT into the reveal instead of a bg gap.
    `window.__pixNight()` and `window.__pixTheme` / `window.__pixKeys`
    (defined parse-first in `Base.astro`'s head) are the ONE day/night boundary /
    theme-constants source / key-shortcut helpers — never re-derive 19:00–07:00,
    the theme BG map, or the typing guard inline.
- **FX** (all `prefers-reduced-motion`-safe) — CRT power-on, hero pixel-dust,
  and the dimmer itself. (The old pointer glow + 3D tilt were removed
  deliberately: perspective transforms smear pixel art.) The live office has
  a pixel-style pause switch (`#office-pause`, bottom-right above the
  statusline — WCAG 2.2.2): pause freezes the office frame in place AND, via the
  `pix:paused` event, stops every other >5s auto-motion (the statusline feed
  ticker, the hero dust) — the statusline reads `❚❚ PAUSED`; resume picks the
  timeline up where it stopped. Hidden only under reduced motion (nothing
  auto-animates); on a no-wasm / fetch-failure poster it stays visible, since the
  ticker / dust / clips still run and it governs them wasm-independently (#456).
- **Docs shell** — `layouts/Docs.astro` gives /config, /architecture,
  /contributing, /knowledge-base, and /parallel-delivery a shared sidebar + build-time mini-TOC + pager,
  driven by the one `DOCS` manifest in `consts.ts` (the Nav dropdown reads the
  same source).

## Demo art

`site/public/demos/*` (office screenshots, demo clips, per-theme shots) is
**generated**, never hand-placed. Regenerate from the pixtuoid binary:

```sh
just gen-media              # from the repo root (or: just gen for README + all images)
```

`scripts/gen-media.py` (driven by `scripts/media.json`) reads `src/themes.json` and
`src/weather.json` (via `@`-refs in the manifest), keeping their variant-set channels
in lock-step with their manifests. It also renders the four animated clips via the
snapshot example's `--gif`/`--navigate-at`/`--agents`/`--pets`/`--meeting`
flags (the multi-floor clip uses `--agents 22 --navigate-at 3:1 --navigate-at 7:0`
to drive the real TuiRenderer across floors; the pets clip uses `--pets cat`; the
meetings clip uses `--meeting 3` to stage three agents converging on one meeting
room, with an auto-computed warmup pre-roll so the clip opens just before the
first agent rises — no screen recording). Each `.gif` is re-encoded through
`encode_clip` into `.mp4` + `.webm` + a poster frame (a clip job's optional
`poster` field picks the poster's second, e.g. mid-meeting for `meetings`) so
`ChannelStage` can emit a `<video>` with both sources.

(Pixel art lives in `public/` on purpose — Astro's `src/assets/` optimizer would
resize/blur it.)

## Showcase (Studio Wall)

The landing page's interactive demo section is a single, manifest-driven
component (`Showcase` → `ChannelStage`; the channel dial is inline mono text —
`MonitorWall.astro` is the kept-but-unmounted fallback, see `knip.jsonc`).
Channel order, labels, and content type are all defined in
**`src/showcase.json`** — the **fifth single-source manifest** alongside
`src/themes.json`, `src/weather.json`, `src/features.json`, and
`src/install.json`.

Channel kinds:

- **`clip`** — mp4 + webm + poster rendered by `just gen-media`. Requires `asset`,
  `w`, `h`, and the three files in `public/demos/` (`<asset>.mp4`, `.webm`,
  `-poster.png`).
- **`variant-set`** — static screenshot grid (themes / weather / day-night).
  References `variantsRef` (a sibling manifest) or an inline `variants` array.
- **`soon`** (`"status": "soon"`) — placeholder monitor, no assets needed.

`astro.config.mjs` enforces the invariants at build time: exactly one `default`
live channel, no duplicate ids, and all live clip assets present on disk.

**Adding a demo channel:** add one entry to `showcase.json` + run `just gen-media`
for the assets. No component edits. For a `clip` channel, also add a render call
and an `encode_clip` block in `scripts/gen-media.py`; `variant-set` channels only
need the manifest entry and whatever static screenshots the manifest references.

## Add a theme

When pixtuoid ships a new in-app theme, the site is a **one-line** update:

1. Add `{ "id": "...", "name": "...", "blurb": "...", "accent": "#...", "accent2": "#..." }`
   to [`src/themes.json`](src/themes.json).
2. Run `just gen-media` to render its screenshot.

The switcher chips, the live "N built-in themes" count, the page retint, and the
render script all pick it up automatically — no component edits.

## Custom domain

Project page today (`base: '/pixtuoid'`). To move to e.g. `pixtuoid.dev`: add
`public/CNAME` with the domain, set `base: '/'` and `site: 'https://pixtuoid.dev'`
in `astro.config.mjs`, then point DNS at GitHub Pages. `robots.txt` and its
sitemap URL (`src/pages/robots.txt.ts`) derive from that config automatically —
note that on the project page crawlers never fetch it (they only read the
ORIGIN root's robots.txt), so it only becomes authoritative on a custom domain.

## First deploy (one-time)

In the repo's **Settings → Pages**, set **Source: GitHub Actions**. After that,
every push to `main` that touches `site/**` redeploys automatically.
