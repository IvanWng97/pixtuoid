# site — agent guide

The **landing page**: a self-contained **Astro** static site (NOT Rust) →
GitHub Pages. A *consumer* of the Rust workspace's outputs — version, rendered
docs, demo media all flow in from outside `site/`. Parent guide: the workspace
[`../CLAUDE.md`](../CLAUDE.md). Human detail: [`README.md`](README.md).

> **You are in the Astro consumer, not the Rust producer.** Rust house rules
> (`cargo`/`clippy`, `just preflight`, `semver`, `gen-check`) do not apply to a
> site-only change. The gates here are `just site-check` (+ `just site-fmt`)
> and `just site-e2e`.

## Cross-boundary build inputs (the coupling that bites)

`astro build` reads **six files from OUTSIDE `site/`**; a rename/move of any
FAILS the build, and all six sit in the `site.yml` / `pages.yml` path filters:

- workspace `Cargo.toml` → displayed-version FALLBACK only; the primary source
  is the latest release tag (`config/released-version.mjs`, unit-tested — main
  runs ahead of the released version mid-cycle). Both workflows checkout with
  `fetch-depth: 0`. `pages.yml` deploys on push to `main`, not on tag push, so
  a fresh tag shows after the next `main` commit or a `workflow_dispatch`.
- `docs/{CONFIGURATION, ARCHITECTURE, CONTRIBUTING, PARALLEL-DELIVERY}.md`
  → rendered routes via glob loaders in `src/content.config.ts`. **Adding,
  renaming, or REMOVING a rendered doc is a multi-point edit**: glob
  pattern, `src/pages/*.astro`, the `DOCS` entry in `consts.ts` (Nav and
  `assert-docs-rendered` derive from it), both workflow path filters,
  `lighthouserc.json`, and the smoke viewport table.

**Mermaid renders at build AND during `astro check`** (why CI installs
Chromium, BEFORE `npm run check`). A version-mismatched Chromium/Playwright
collapses `<Content />` to an empty article WITHOUT failing the build, and
`withastro/action`'s cross-run Astro cache can then serve the empty page back
forever — hence `pages.yml` pins `package-manager: npm` (#680) and
`cache: false` (#682). All three historical causes are gated by
`config/assert-docs-rendered.mjs` (`check:docs`, in `verify` AND the deploy
build-cmd): every doc `<article>` has a body and `/architecture` keeps its
`<svg>` — a collapsed render reddens, never deploys.

**Docs shell**: the five doc routes mount the Statusline doc variant (no PR
feed fetch); `Docs.astro`'s sidebar reads the one `FLOORS` manifest.
Blockquotes promote to terminal callouts (`config/rehype-callouts.mjs`,
unit-tested, registered AFTER `rehypeRepoLinks`). **SHARP EDGE — the callout
window is a `--screen` panel inside `.prose`, so every `.prose <tag>` colour
rule beats the ink the callout hands down** (direct match beats inheritance).
Each such rule needs a twin at `.docs :global(.callout__body <tag>)`, and the
twin must OUTWEIGH the prose rule — count the specificity, including
theme-prefixed globals that tie (source order then decides) and second-type
selectors that outweigh. The smoke suite's callout AA sweep is the proof;
add a `.prose` colour rule ⇒ add its twin and let the sweep grade it.

## Single-sourced content

Every generated artifact, manifest seam, and rendered-copy sharp edge:
[`SINGLE-SOURCED.md`](SINGLE-SOURCED.md) — read it before touching this area.

## CSP (hash-based, two coordinated halves in astro.config.mjs)

Astro 7's `security.csp` owns policy + resource lists; the `cspInlineHashes()`
`astro:build:done` hook re-derives inline-script hashes from the BUILT html
(Astro doesn't hash template `is:inline` scripts — verified vs 7.0.5) and
HOISTS the `<meta>` directly after `<meta charset>` (a meta CSP governs only
what follows it; Astro emits it below the scripts it hashes). The pure
transform `rewriteCspMeta` lives in `config/csp-hashes.mjs`, unit-tested.
Rules: adding/editing an `is:inline` script needs NO manual CSP step;
hand-written `public/*.js` loaded by URL rides `script-src 'self'`;
`style-src` keeps `'unsafe-inline'` and must stay hash-free (inline style
ATTRIBUTES — mermaid SVG, `style={}` — can't be hashed; one hash disables
unsafe-inline for the directive; keep Prism class-based highlighting, not
Shiki). `astro dev` serves NO CSP — regressions surface in `just site-e2e`'s
console watchdog, not dev.

## Dev server (agent-driving)

`just site-dev-bg` daemonizes (`astro dev --background`, polls the dev-only
`/_astro/status` endpoint); `just site-dev-stop` frees the port. Two edges:
the status endpoint + daemon subcommands are dev-server only (`astro preview`
has neither — verified vs 7.0.5), and dev/preview share port 4321 — stop the
daemon before `just site-e2e` (its webServer fails loud on a squatted port).

## Gates

`just site-check` = `npm run verify` (format:check → lint → check → knip →
test:unit → build → check:docs → audit). `just site-e2e` = Playwright vs the
PRODUCTION build — the runtime-contract tier (`__pixLights`/`pix:onair`/
`data-lit` seams, scrollspy keys, docs-nav variant, reduced-motion, console
watchdog) that tsc/knip/build are blind to. CI: `site.yml` / `pages.yml`.

- **Lighthouse** (`npm run lighthouse`, in-repo runner
  `config/lighthouse-runner.mjs`, lighthouse 13 programmatic API, median of
  three serial runs; runner-semantics pinned by its test — a renamed audit
  FAILS instead of passing vacuously). **A category score is a budget, not a
  contract**: `color-contrast` is 7/195 of the a11y category, so a total
  contrast failure still scored 0.93 — anything that must never regress gets
  its own per-audit assertion with `aggregationMethod: pessimistic` (median
  greens a binary audit that failed one run). Collect URLs pin `?theme=day`
  (otherwise the wall clock picks the palette — half of CI runs would audit
  each). **The theme matrix is `smoke.spec.ts`** (day/night/dracula), not a
  doubled collect list.
- **Fonts**: Fontsource WOFF2 with `font-display: optional` + preloads — do
  not switch to the default `swap` (Ubuntu cold visits reflow past the CLS
  budget; `font-layout.spec.ts` reproduces it deliberately).
- **npm**: `audit` runs LAST in `verify` (a live advisory would short-circuit
  the checks below it — #847/#849) but FIRST in `pages.yml` (that one ships).
  npm 12 pinned (`packageManager` + `engine-strict`); install scripts
  fail-closed (`strict-allow-scripts`, exact-version `allowScripts`, explicit
  `fsevents` denial) — review with `npm install-scripts ls`.
- **Nothing gates a STALE `overrides` entry**: to retire one, fresh-resolve
  `--package-lock-only` in a scratch dir with and without it and diff the
  `packages` maps — `npm audit` cannot clear an entry (the retired
  `chrome-launcher` pin guarded a chain no advisory covered).
