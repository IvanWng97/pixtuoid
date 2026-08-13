# site — agent guide

The **landing page**: a self-contained **Astro** static site (NOT Rust) → GitHub
Pages. It's a *consumer* of the Rust workspace's outputs — the displayed version,
the rendered docs, and the generated demo media all flow in from outside `site/`.
Parent guide: the workspace [`../CLAUDE.md`](../CLAUDE.md). The cross-area
development model: [`../docs/PARALLEL-DELIVERY.md`](../docs/PARALLEL-DELIVERY.md).
Full detail: [`README.md`](README.md).

> **You are in the Astro consumer, not the Rust producer.** The workspace
> `CLAUDE.md` loads above this file, but its Rust house rules (`cargo`/`clippy`,
> `just preflight`, `semver`, `gen-check`) do not apply to a site-only change.
> The gates here are `just site-check` (+ `just site-fmt`).

## Cross-boundary build inputs (the coupling that bites)

`astro build` reads **six files from OUTSIDE `site/`** at build time, and a
rename/move of any of them FAILS the build:

- the workspace `Cargo.toml` → the displayed version's FALLBACK only: the
  primary source is the latest RELEASE tag (`git describe --tags --abbrev=0`
  via `config/released-version.mjs` — main's Cargo.toml runs AHEAD of the
  released version between a mid-cycle bump and its tag, and the site must
  advertise what users can actually install). Both CI workflows checkout
  with `fetch-depth: 0` so the tag path is the one real deploys take;
  unit-tested in `config/released-version.test.mjs`. Note: `pages.yml` deploys on
  push to `main` (+ path filter), NOT on a release-tag push — so a fresh tag's
  version shows only after the next `main` commit (or a manual `workflow_dispatch`).
- `docs/{CONFIGURATION, ARCHITECTURE, CONTRIBUTING,
  KNOWLEDGE-ENGINEERING, PARALLEL-DELIVERY}.md` → rendered as `/config`,
  `/architecture`, `/contributing`, `/knowledge-base`,
  `/parallel-delivery` respectively, via a `glob` loader in
  `src/content.config.ts` + a `src/pages/*.astro` per route. **Adding a rendered
  doc is the inverse of a rename — a new `glob` collection, a `src/pages/*.astro`,
  a `DOCS` entry in `consts.ts` (the `DocId` union + `Docs.astro`'s `current`
  type auto-derive; sidebar + pager), a `Nav.astro` link, and both path filters.**

All six are in the `site.yml` / `pages.yml` **path filters**, so editing one
re-runs the site CI + redeploys. **Renaming a rendered doc is a multi-point
edit** — the `glob` pattern, the page's `sourcePath`, the nav label, the two
path filters, and the doc itself (the `KNOWLEDGE-BASE → KNOWLEDGE-ENGINEERING`
rename is the worked example: the *route slug* `/knowledge-base` was kept to
avoid link rot while the file + display name changed). `ARCHITECTURE.md`'s
Mermaid diagram becomes an inline SVG via `rehype-mermaid` whenever the content
layer renders it — during `astro check` AS WELL AS `astro build` — which is
**why CI installs Chromium**; break the Mermaid *syntax* and `astro build` fails —
but a Chromium/Playwright *version* mismatch fails DIFFERENTLY: `rehype-mermaid`
collapses the whole `<Content />` to an empty `<article>` WITHOUT erroring the
build, so it can silently ship an empty `/architecture` (the deploy's
`withastro/action` fell back to pnpm → a Playwright unmatched to the installed
Chromium; #680 pins `package-manager: npm`). A SECOND, subtler way it ships empty:
`withastro/action`'s Astro build cache (`cache: true`, `node_modules/.astro`)
uses a bare `astro-cache-<os>-` restore-key, so a deploy restores the *previous*
deploy's cache — and once a broken deploy caches an empty `/architecture`, every
later deploy serves it back as a **+7ms cache HIT** and never re-renders the
`mermaid` block (no Chromium launch, no error, still empty). `pages.yml` sets
`cache: false` so each deploy does a fresh real render; poison can't persist
(#682). `site.yml` renders fine because it has no cross-run Astro cache. A
THIRD, orthogonal cause is INSTALL ORDER: the render also fires during `astro
check`, so Chromium must be installed BEFORE `npm run check` — `pages.yml` once
had the install LAST in the `&&` chain, so check rendered browserless and the
deploy went red (the 2026-07 outage; #776 reorders it before check, mirroring
`site.yml`). All three are caught by `config/assert-docs-rendered.mjs` (`check:docs`) — run in
`verify`/site-check AND the deploy's `build-cmd`, asserting every doc page's
`<article>` has a body + `/architecture` kept its `<svg>` — so a collapsed render
reddens, never deploys.

**wb-5 (Lobby + Docs):** the five doc routes now mount the Statusline **doc
variant** (index-only organs — floor lift, PR feed, env readouts, keys hint —
omitted; the left segment renders `~ pixtuoid docs · /<route>` instead; the
build-time PR-feed fetch is skipped entirely for doc pages, so the doc
pages (the five routes plus 404) don't each re-hit the GitHub API at build). `Docs.astro`'s sidebar is
now an elevator panel (`.hw-panel` + `.led-dot`) with a DOCS wing plus a
building bank of every OTHER floor, both read off the one `FLOORS` manifest in
`consts.ts`. Every top-level blockquote in a rendered doc promotes to a
terminal-window callout via [`config/rehype-callouts.mjs`](config/rehype-callouts.mjs)
— a pure hast transform, unit-tested, registered in `astro.config.mjs`'s
processor AFTER `rehypeRepoLinks` (order matters: it walks the final tree).
Note its smartypants quirk: Astro's remark-smartypants has already turned a
straight `'` into a curly U+2019 by the time the transform runs, so the
imperative-warning sniff normalizes back before matching. **SHARP EDGE — the
callout window is a `--screen` panel dropped inside `.prose`, so every
`.prose <tag>` colour rule is a cascade trap**: a rule that matches the tag
DIRECTLY (`.prose p`, `.prose li`, `.prose a`, `.prose :not(pre) > code`)
always beats the `--chip-ink` the `.callout__body` blockquote hands DOWN —
inheritance loses to any match, whatever the specificity. `Docs.astro`
therefore reclaims each of them at `.docs :global(.callout__body <tag>)`,
which Astro's attribute scoping compiles to
`.docs[data-astro-cid-…] .callout__body <tag>` = **(0,3,1)** — three
class-level components plus one type, beating `.prose <tag>` (0,1,1) by
WEIGHT. `p`/`li` were missed when `a`
and `code` were first noticed, which shipped day's body copy as `--fg` ink on
the near-black screen (1.16:1, three published pages) until the smoke suite's
callout AA sweep pinned it. Add a `.prose` colour rule ⇒ add its callout
twin, and let that sweep prove it — but COUNT the twin: a theme-prefixed
global rule (`:root[data-theme='night'] .prose a`) is ALSO (0,3,1), an exact
TIE that the twin wins only on source order (Astro emits component styles
after `global.css`), and one carrying a second type selector
(`… .prose a > code`, (0,3,2)) OUTWEIGHS a plain twin outright — which is how
dracula's link-wrapped code chip briefly wore `--surface` beside its
`--hw-hover` siblings inside the terminal window. That twin therefore lifts to
`.docs :global(.callout .callout__body a > code)` (0,4,2) and wins on weight.
Elsewhere in the
same arc: `src/faq.json` is a NEW single-sourced content manifest (the pantry
chitchat FAQ copy, every answer citing a repo contract, e.g. the hook-shim
200ms/exit-0 invariant); the lobby tenant-directory board restyle kept
`src/sources.json` untouched (bridge tests stay green); and
[`config/plaque-stars.mjs`](config/plaque-stars.mjs) is the star plaque's
display-line authority (`starText`), unit-tested on its null-stars arm since
`__GH_STARS__` is a build-time `vite.define` the e2e suite always overrides.

## Single-sourced content (don't hand-edit the rendered copy)

Moved to [`SINGLE-SOURCED.md`](SINGLE-SOURCED.md) — read it before touching this area.

## CSP (hash-based, two coordinated halves — both in astro.config.mjs)

The `<meta>` CSP is Astro 7's built-in `security.csp` PLUS the
`cspInlineHashes()` `astro:build:done` hook. The **policy** (`security.csp`
directives) and the **hook registration** stay together in `astro.config.mjs`
(the anti-drift co-location); the pure per-page transform — `rewriteCspMeta(html)`
— lives in [`config/csp-hashes.mjs`](config/csp-hashes.mjs), unit-tested by
`config/csp-hashes.test.mjs` (`npm run test:unit`, in `verify`) so its
quote-aware script-tag scan can't diverge from the HTML tokenizer. Astro emits
the meta into every page's head (404 included) and owns the RESOURCE lists; the
hook then re-derives the hash sets from the **built html**, because (verified
vs 7.0.5) Astro does not hash template-level `is:inline` scripts — the only
script kind this site has — and it appends style hashes unconditionally, which
would make browsers *ignore* `'unsafe-inline'`. Consequences to not "fix":

- **The hook also HOISTS the meta**, directly after `<meta charset>`. A
  `<meta http-equiv>` policy governs only what FOLLOWS it, and Astro emits it at
  its head-injection point — measured on 7.1.3, three `<script>` and one
  `<style>` above it on `/`, including the theme-init that reads localStorage, so
  the policy did not cover the scripts whose hashes it carries. `<meta charset>`
  is the anchor rather than the `<head>` tag because the encoding sniffer only
  reads the first 1024 bytes and the policy is 1-3 KB of hashes. Source-reading
  cannot confirm this one — check `dist/**/*.html`.

- **`script-src` carries no `'unsafe-inline'`** — every inline script is
  whitelisted by content hash, recomputed on each build. Adding/editing an
  `is:inline` script needs NO manual CSP step.
- **A hand-written `public/*.js` module loaded by URL** (`audio-worker.js` via
  `new Worker`, `office-driver.js` via `import()`) rides `script-src 'self'` as
  an EXTERNAL resource — it is not hashed and needs no CSP step; only its
  runtime-loading `is:inline` caller is (auto-)rehashed when its content changes.
- **`style-src` keeps `'unsafe-inline'` and must stay hash-free**: the
  build-time mermaid SVG and the few `style={}` attributes are
  inline style ATTRIBUTES, which hashes cannot express (one present hash
  disables `unsafe-inline` for the whole directive). Markdown code uses
  class-based Prism highlighting; do not switch it back to Shiki's inline
  style attributes, which also makes Astro's CSP build warn.
- **`astro dev` serves NO CSP** (upstream: the feature is build/preview-only).
  CSP regressions surface in `just site-e2e`'s console watchdog against the
  production build, not in dev.

## Dev-server lifecycle (agent-driving)

Foreground `astro dev` quits on stdin EOF — under a PTY an AI agent could not
keep it alive across commands. Astro 7's `--background` mode is the fix:
**`just site-dev-bg`** daemonizes the server (no stdin/TTY tie) and polls the
dev-only `/_astro/status` health endpoint (`{"ok":true}`) until ready;
**`just site-dev-stop`** (= `astro dev stop`) shuts it down and frees the port.
`astro dev status` / `astro dev logs --follow` inspect the daemon; non-TTY runs
auto-emit JSON log lines. Two sharp edges: `/_astro/status` and the
background/stop/status subcommands are **dev-server only** — `astro preview`
404s the endpoint and has no daemon mode (verified vs 7.0.5), so the e2e
webServer keeps its URL-poll readiness; and dev/preview share port 4321, so
**stop the daemon before `just site-e2e`** (its webServer fails loud on a
squatted port, by design). `just site-dev` stays foreground for humans who
want HMR logs.

## Gates

`just site-{setup, dev, dev-bg, dev-stop, check, fmt, e2e}` (see `README.md`). The full-stack gate
is `just verify` = `preflight` + `site-check` + `gen-check`. For a site-only
change, `just site-check` is the relevant one; `just site-e2e` (Playwright vs
the PRODUCTION build via `astro preview` — the official Astro posture) pins the
page's RUNTIME contracts (`__pixLights`/`pix:onair`/`data-lit` seams, the
digit-key scrollspy, the docs-nav variant, reduced-motion) plus a console-error
watchdog, where tsc/knip/build are blind. CI is `site.yml` / `pages.yml` (NOT
the Rust `ci.yml`).

Lighthouse runs every route three times and gates volatile lab metrics by their
median; the one first-visit reveal timing stays pessimistic because all three
visits must clear it. **SHARP EDGE — a `categories:*` assertion cannot fail on
one audit.** `color-contrast` weighs 7 of the accessibility category's 195
points, so a TOTAL contrast failure costs 3.6% against a `minScore: 0.9` floor:
the landing page really did score 0.93 with `color-contrast` at 0 and a hard AA
failure in the footer, green. `lighthouserc.json` therefore asserts
`"color-contrast": ["error", { "minScore": 1, "aggregationMethod":
"pessimistic" }]` alongside the category — a category score is a budget, not a
contract, so anything that must NEVER regress needs its own per-audit
assertion, and a CONTRACT audit needs every run to clear it. The
`aggregationMethod` is not decoration: the runner defaults to `median`
(`scripts/lighthouse-runner.mjs`), and the median of three runs still greens a
binary 0/1 axe audit that failed ONE of them — hence `pessimistic` on the
contract audits. Second surprise: Lighthouse does **not** pick a theme of its own.
`Base.astro`'s init falls back to night off the 7/19 wall clock, so an
unqualified URL audits whatever palette the runner's clock happens to hold —
half of all CI runs on one palette, half on the other, and a hard per-audit
assertion on top of that is a coin flip. The collect URLs therefore carry
`?theme=day` (the `Base.astro` override, ahead of storage/clock/system): ONE
deterministic palette, the default one, and the one whose light ground the
`--screen`-chip inks are hardest on. **Lighthouse is the rendered-DOM axe
backstop on the pinned theme, not the theme matrix** — the theme matrix is
`smoke.spec.ts`, which drives day/night/dracula explicitly; adding a
theme-dependent surface means adding it THERE, not doubling the collect list.
`src/styles/fonts.css` uses Fontsource's own WOFF2 assets
with `font-display: optional`, while `Base.astro` preloads the regular faces.
Do not replace those declarations with Fontsource's default `swap` CSS: an
Ubuntu cold visit lacks the metric-matched Georgia fallback and reflows long doc
pages above the CLS budget. `font-layout.spec.ts` delays every font response and
forces a deliberately mismatched fallback so that platform-specific failure is
reproducible in the production-browser suite; Google's pinned `web-vitals`
package owns the canonical CLS calculation, while the test owns only this
repository's delayed-font scenario and budget.

`site-check` ends with `npm audit --audit-level=low`, and site.yml runs it in the
same last position: the audit resolves advisories LIVE, so at step one someone
else's publish short-circuits every check below it (#847/#849). `pages.yml`
deliberately keeps it FIRST — that workflow ships. The npm generation is part of the
toolchain: `packageManager` pins CI to npm 12.0.1, `engines.npm` +
`engine-strict=true` reject older local clients, and both workflows upgrade the
older npm bundled with Node 26 before install. npm install scripts are
fail-closed (`strict-allow-scripts=true`). `allowScripts` grants only the
exact-version esbuild approval; `fsevents` is explicitly denied because npm's
registry metadata flags an install script even though the installed manifest
needs none. Use `npm install-scripts ls` after dependency changes: an
unreviewed script must fail installation instead of becoming a warning.
`npm run lighthouse` is an in-repo runner (`scripts/lighthouse-runner.mjs`)
driving lighthouse 13 through the official programmatic API (chrome-launcher +
`lighthouse(url, {port})`, per lighthouse `docs/readme.md`), reading the SAME
`lighthouserc.json` budgets `@lhci/cli` did. LHCI was dropped after a year
without a release left it pinning `lighthouse 12`, which dragged the
`@puppeteer/browsers` and `chrome-launcher` overrides — both retired with it
(GHSA-jmr9-qjv8-65gv's extract-zip chain is gone by construction: lighthouse 13
takes `puppeteer-core ≥25`, whose `@puppeteer/browsers` is natively 3.x).
Aggregation follows lighthouse `docs/variability.md`: serial runs, assertions
on median-of-N. The ONE semantic divergence from LHCI: a missing
`aggregationMethod` defaults to MEDIAN, not optimistic — stricter-or-equal,
pinned by `config/lighthouse-runner.test.mjs`, which also pins that a renamed
audit, category, or `user-timings` mark FAILS the run instead of passing
vacuously. `config/lighthouse-budget.test.mjs` keeps gating the budget file's
shape, unchanged.

**Nothing gates a STALE `overrides` entry**, so retiring one is a manual
audit: copy `package.json` to a scratch dir (never the working tree, whose
lockfile is the artifact under test), `npm install --package-lock-only` with
and without the entry, and diff the generated `packages` maps — identical
means inert. The `yaml-language-server` → `yaml 2.8.3` pin was retired this
way once `@astrojs/check` 0.9.10 pulled a language-server whose
`yaml-language-server` declares an exact `yaml 2.8.3` of its own. **`npm
audit` alone cannot clear an entry**: `chrome-launcher`'s guards a deprecated
chain no advisory covers, so audit reads clean while
`rimraf@3`/`glob@7`/`inflight` come back.
