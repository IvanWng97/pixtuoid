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

## Quality gates

```sh
npm run verify     # format:check → lint → astro check → build  (== site CI)
# individually:
npm run format     # prettier --write .
npm run lint       # eslint .
npm run check       # astro check (types + templates)
npm run build      # astro build → dist/
```

From the repo root the same gate is `just site-check` (and `just site-fmt`).

> **Cross-boundary build inputs.** The site reads six files from _outside_ `site/`
> at build time: the workspace `Cargo.toml` (displayed version, via `vite.define` in
> `astro.config.mjs`), `docs/CONFIGURATION.md` (rendered as `/config`),
> `docs/ARCHITECTURE.md` (rendered as `/architecture` — its Mermaid diagram becomes an
> inline SVG at build via rehype-mermaid, which is why CI installs Chromium),
> `docs/CONTRIBUTING.md` (rendered as `/contributing`), `docs/MIGRATION.md`
> (rendered as `/migration`), and `docs/KNOWLEDGE-BASE.md` (rendered as
> `/knowledge-base`).
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
- **The building** — the landing page maps the product's multi-floor metaphor
  onto its own structure: each section is a floor (6F penthouse → 1F front desk),
  `Elevator.astro` is a fixed desktop panel tracking scroll, and the app's
  literal keys work on the page — digits `1–6` ride the elevator, `t` retints
  the decorative palette through the THEMES list (Esc restores). The favicon
  dims with the night theme; 404 is the office at 3 a.m. (`--empty` render).
- **FX** (all `prefers-reduced-motion`-safe) — headline shimmer, CRT power-on,
  pixel-dust, and click-to-retint theme chips. (The old pointer glow + 3D tilt
  were removed deliberately: perspective transforms smear pixel art.)
- **Ticker** — fed at build time with the repo's last merged PRs (the agents
  that built pixtuoid); falls back to a canned reel offline so builds never
  block on the GitHub API.
- **Docs shell** — `layouts/Docs.astro` gives /config, /architecture,
  /contributing, /migration a shared sidebar + build-time mini-TOC + pager.

## Demo art

`site/public/demos/*` (office screenshots, the hero `.mp4`, per-theme shots) is
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
component (`Showcase` → `ChannelStage` / `MonitorWall`). Channel order, labels,
and content type are all defined in **`src/showcase.json`** — the **fifth
single-source manifest** alongside `src/themes.json`, `src/weather.json`,
`src/features.json`, and `src/install.json`.

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
in `astro.config.mjs`, then point DNS at GitHub Pages.

## First deploy (one-time)

In the repo's **Settings → Pages**, set **Source: GitHub Actions**. After that,
every push to `main` that touches `site/**` redeploys automatically.
