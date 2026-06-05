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

> **Cross-boundary build inputs.** The site reads two files from _outside_ `site/`
> at build time: `docs/CONFIGURATION.md` (rendered as the `/config` page via the
> Astro content layer in `src/content.config.ts`) and the workspace `Cargo.toml`
> (the displayed version, injected via `vite.define` in `astro.config.mjs`).
> Renaming or moving either breaks `astro build` — both are listed in the
> `site.yml` / `pages.yml` path filters so a change to them re-runs CI + redeploys.

## Design

- **Layout/type** — "Cozy Terminal": Pixelify Sans (display) · JetBrains Mono
  (UI/code) · Lora (body); ASCII dividers, blinking cursor, CRT scanlines.
- **Palette** — warm "Coworking" (cream lifted from the office carpet + Claude
  coral). Day = cream, night = after-hours. `dracula` is a hidden easter-egg
  theme (type it, or `?theme=dracula`).
- **FX** (all `prefers-reduced-motion`-safe) — pointer glow, 3D tilt, headline
  shimmer, CRT power-on, pixel-dust, and click-to-retint theme chips.

## Demo art

`site/public/demos/*` (office screenshots, the hero `.mp4`, per-theme shots) is
**generated**, never hand-placed. Regenerate from the pixtuoid binary:

```sh
./scripts/gen-demos.sh      # or: just site-demos
```

It reads the theme list from `src/themes.json`, so it stays in lock-step with the
switcher. (Pixel art lives in `public/` on purpose — Astro's `src/assets/`
optimizer would resize/blur it.)

## Add a theme

When pixtuoid ships a new in-app theme, the site is a **one-line** update:

1. Add `{ "id": "...", "name": "...", "blurb": "...", "accent": "#...", "accent2": "#..." }`
   to [`src/themes.json`](src/themes.json).
2. Run `./scripts/gen-demos.sh` to render its screenshot.

The switcher chips, the live "N built-in themes" count, the page retint, and the
render script all pick it up automatically — no component edits.

## Custom domain

Project page today (`base: '/pixtuoid'`). To move to e.g. `pixtuoid.dev`: add
`public/CNAME` with the domain, set `base: '/'` and `site: 'https://pixtuoid.dev'`
in `astro.config.mjs`, then point DNS at GitHub Pages.

## First deploy (one-time)

In the repo's **Settings → Pages**, set **Source: GitHub Actions**. After that,
every push to `main` that touches `site/**` redeploys automatically.
