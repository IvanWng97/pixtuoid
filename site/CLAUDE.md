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
Mermaid diagram becomes an inline SVG at build via `rehype-mermaid`, which is
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
(#682). `site.yml` renders fine because it has no cross-run Astro cache. Both
paths are caught by `config/assert-docs-rendered.mjs` (`check:docs`) — run in
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
imperative-warning sniff normalizes back before matching. Elsewhere in the
same arc: `src/faq.json` is a NEW single-sourced content manifest (the pantry
chitchat FAQ copy, every answer citing a repo contract, e.g. the hook-shim
200ms/exit-0 invariant); the lobby tenant-directory board restyle kept
`src/sources.json` untouched (bridge tests stay green); and
[`config/plaque-stars.mjs`](config/plaque-stars.mjs) is the star plaque's
display-line authority (`starText`), unit-tested on its null-stars arm since
`__GH_STARS__` is a build-time `vite.define` the e2e suite always overrides.

## Single-sourced content (don't hand-edit the rendered copy)

- The root `README.md` Features table + install commands are GENERATED from
  `src/features.json` / `src/install.json` (`just gen-readme`); drift is gated by
  the `readme` job (`just gen-readme-check`) on every PR. Edit the JSON, not the
  README prose. `gen-readme.mjs` reads only `icon`/`name`/`desc`/`pix`/`featured` off
  each row, regardless of the partition below.
- **`features.json` is the TOTAL feature collection, partitioned by `channel`**
  (wb-3.1): a row with a video/live demo carries `channel: "<showcase.json id>"`
  and DRIVES the 5F studio's channel dial — the dial no longer hand-lists 7
  channels beside a separately-curated roster, it's `showcase.json` joined
  against every `channel`-bearing `features.json` row (`consts.ts`'s
  `featureForChannel`); the rest (no `channel`) render as the merged 5F band's
  quiet, non-interactive grid BELOW the CRT + dial (`Showcase.astro`'s
  `roster`, falling back to `desc` — stripped of any README-authored backtick
  code spans, e.g. `` `pixtuoid floating` `` — when a row has no `card.blurb`).
  The two manifests' `channel`↔`id` correspondence is a BIJECTION, enforced at
  build time by `astro.config.mjs`'s guard (immediately after the pre-existing
  showcase guard): every showcase.json channel needs exactly one claiming
  features.json row and vice versa, or `astro dev`/`build` fails loud with the
  offending id. **The dial is an accordion**: clicking a channel sets
  `aria-expanded` and swaps `#dial-desc`'s text to that channel's joined
  `desc` — ONE shared slot below the (unchanged, 3-col/2-col) dial grid, not
  a per-row expansion, since the panel already sits shorter than the CRT
  stage's row height (`align-items: start` leaves headroom there for free —
  measured net scroll-budget delta from adding this: zero, see the ≤7.9vh
  pin's test). `ShowcaseChannel.caption` (the stage's diegetic figcaption) is
  now OPTIONAL: a channel whose caption would just restate its joined
  feature's desc (only "pets" today) omits it, and `ChannelStage.astro` falls
  back to the feature desc — one description, not a same-screen repeat;
  channels with a caption that adds real distinct color (agents' swarm-scale
  aside, openclaw's per-state motion detail, meetings' actual dialogue quotes,
  vibing's "you're driving this one") keep it. `card.href` (a per-row
  "tune in →" deep link into the studio) was RETIRED earlier (wb-3): its
  channel mapping was half-fabricated (e.g. Coffee run → vibing, monitor glow
  → spaces) and duplicated the dial one studio-panel-width away — the wb-3.1
  bijection guard is the principled replacement for that ad hoc cross-guard.
- The six-floor anchor vocabulary (`data-floor="6F"`…`"1F"` + `data-floor-label`,
  stamped from `consts.ts`'s `FLOORS`) is what `ElevatorShaft.astro` (mounted
  index-only, a `pix:paused` set member) and the Statusline's scrollspy both
  consume off the one `FLOOR_SPY_ROOT_MARGIN` band. The Showcase/Features
  merge re-keys the 4F `FLOORS` entry from `features` to `amenities`
  (mounting the `section#amenities` shell in `index.astro` — still an empty
  eyebrow-only shell, for future sibling components to fill), keeps a
  `#features` anchor-compat shim atop the merged 5F band so inbound deep
  links still resolve, and adds `data-keys-scope="channels"` + scoped
  channel-tune keys on `Showcase.astro`'s `section#showcase` (the first live
  example of the focused-region carve-out the global digit handler already
  respects — see `README.md`'s cross-component-seams note). The inner
  studio-wall div keeps `id="studio"` — no collision, since the 5F section id
  is `#showcase`. Studio's right column (`.studio__panel`) holds ONLY the
  channel dial — the ONE interactive switcher; the feature roster (`.roster`,
  the `#features` shim's landing spot) sits BELOW the whole stage as a quiet,
  non-interactive, full-width two-column grid (single column ≤760px) —
  reading the room no longer means "which row do I click," just the dial.
- The demo media under `public/demos/` is GENERATED by `scripts/gen-media.py` +
  `scripts/media.json` (the one manifest-driven driver), rendering through the
  REAL `TuiRenderer` (and, for `hero-wide.png` + `vibing-poster.png`, the real
  wasm `Office` via the `pixtuoid-web` `hero_still` example — the latter with
  `--hour 18 --weather clear`) — `just gen` regenerates, `just
  gen-check` pixel-diffs the committed stills. Clips (`.webm`/`.mp4`) are presence-gated, not pixel-gated
  (encoding is non-deterministic). The §3 proof split (demos/proof*) renders via
  snapshot --proof over the committed proof-session fixture
  (`crates/pixtuoid-core/tests/sources/fixtures/claude-code/proof-session/`) — its
  posters are pixel-gated, its encodes presence-gated; retime the fixture, not
  the component. A PR that changes the office's look
  regenerates these in the same change (workspace `CLAUDE.md`).
- `public/wasm/` is the live-office backdrop's engine — a GENERATED, COMMITTED
  artifact built from the `pixtuoid-web` crate by `just gen-wasm` (wasm +
  wasm-bindgen JS glue, size- and pair-gated — a sha256 manifest pins the
  wasm/glue ABI pair — by `gen-wasm-check` in the Rust CI).
  `components/OfficeBackdrop.astro` dynamically `import()`s it at runtime
  (cover-first on the boot path — the canvas covers the baked poster with its
  OWN `var(--bg)` tone, then FLOOR-ROLLS the live office up out of that tone once
  the boot splash clears (`pix:revealed`), so the reveal never cross-dissolves a
  wrong-time still — the day/night flip is gone by construction; the splash in
  turn HOLDS on `window.__pixEngineReady` (Level-2 gate) so it lifts straight into
  the roll. Any failure / no-JS / no-wasm / reduced-motion keeps the still poster).
  Never hand-edit
  (prettier/eslint/knip all ignore it); regenerate from the crate.
  The hero renders at `BUF_H=130`, `SEED=0` (a deliberately closer camera than
  the app's ~180 — roadmap C): at the 64px `bufW` floor (very narrow phones)
  the 11-CLI cast OVERFLOWS the ~8 desks that floor lays out — the cast that
  fits seats, the rest stay unadmitted (invisible), and the install-copy
  `hire()` easter egg is politely refused for lack of a free desk — graceful
  and owner-accepted, not a bug (pinned by pixtuoid-web's
  `a_phone_narrow_office_the_cast_fills_refuses_hires_outright`).
- **The crisp AA caption layer (`#office-overlay`).** The canvas renders a
  ~130px buffer that CSS upscales with `image-rendering: pixelated`, so text
  BAKED into the office pixels blows up blocky. Instead the engine exports the
  name badges + the neon wall board as a MODEL — `pixtuoid-web`'s
  `Office.overlay_json()` (the SAME `pixtuoid_scene::overlay` + `pixtuoid_scene::board`
  the TUI/floating painters use) → `{ labels:[{x,y,text,color,badge?}],
  board:{rect,brand, star,mood,context} }`, coords in OFFICE-BUFFER px, colors
  resolved against the current theme. A label's optional `badge` (#657,
  owner-ratified across ALL THREE painters) is the CLI-identity color: the
  source's per-CLI hue (`SourceColors::by_prefix` — the SAME hue the app's
  dashboard/Sources/tooltip badges use) painting the WHOLE name, while the
  `●` marker keeps the activity tone (`color`) — the status-dot idiom: dot =
  busy/idle, text = identity. An unregistered prefix omits `badge` and the
  label stays tone-only. SHARP EDGE:
  the engine re-derives the prefix by splitting the label on its FIRST `·` —
  a cross-crate echo of core's `source_label_prefix` + `·` join, pinned by
  the web `labels == badges` test, not a shared const.
  `OfficeBackdrop.astro` lays pooled Monaspace Neon (`var(--font-mono)`)
  DOM `<span>`s over the canvas at DISPLAY resolution, positioned by the canvas's
  `object-fit: cover` geometry (`scale = max(disp/buf)`, buffer centered — the
  same math the cover-crop uses), so they stay sharp at any zoom. Each pooled
  label holds TWO fixed child spans — ● dot (tone) / name (badge hue, tone
  when absent) — updated together behind one text+color+badge change key;
  child `textContent`s concatenate to the full label, so text assertions
  and hit-tests read it unchanged. Load-bearing:
  (1) captions update + fade in (`.is-on`) only AFTER the reveal roll settles
  (`rt >= 1`), never over the rolling floors — labels track FINAL sprite
  positions; (2) `JSON.parse` is try/caught so a malformed frame degrades to no
  overlay, never a throw; (3) segment spans use `.textContent`, never
  `innerHTML` — agent cwds are untrusted; (4) reduced motion is `display:none`
  (no live office to caption); (5) caption legibility is HALO-carried (the
  dark text-shadow) by design — raw label/badge hues are deliberately not
  WCAG-gated against the office pixels (the idle/exiting grays never were;
  the flat chips elsewhere on the page are the raw-contrast surfaces).
  Pinned by `smoke.spec.ts` ("crisp AA captions
  overlay the live office" — incl. the 3-span split with a colored prefix —
  + the reduced-motion hide twin). This layer is part
  of the same `is:inline` script the CSP hook hashes — no manual CSP step.
  The backdrop's pause switch (`#office-pause`, WCAG 2.2.2) lives in the same
  component: pause stops the rAF loop (frozen frame stays on the canvas) and
  resume subtracts the paused span from the sim clock (`pauseOffset`) so the
  timeline doesn't lurch-jump. Pause is **page-scoped**: `setPaused` dispatches
  `pix:paused` and every other >5s auto-motion listens (the statusline feed
  ticker, the hero dust) — one control governs the page's *ambient* motion,
  and the statusline reads `❚❚ PAUSED`. The Showcase demo clips ARE in the
  `pix:paused` set (`Showcase.astro` `syncVideos` gates play on `userPaused`):
  in normal motion they auto-loop with NO visible controls, so in-view-gating
  alone did not satisfy 2.2.2 — the page pause button is their pause affordance.
  (Under reduced-motion the clips instead pause and show native `<video>`
  controls, a separate path.) Because that ambient motion is **wasm-independent**,
  `#office-pause` is decoupled from the office canvas (#456): the button is shown
  whenever motion runs = NOT reduced-motion, and its control (`setPaused` / the
  click handler) wires up even on a no-wasm engine or a failed fetch — so a
  non-reduced-motion visitor whose wasm never loads can still pause the ticker /
  dust / clips. Only the office RENDER path (`boot`/`paint`) is gated on `hasWasm`;
  `start`/`stop` are no-ops without a live office, so `setPaused` is safe
  standalone. Reduced-motion hides the button (nothing auto-animates there). The
  wasm fetch is **deferred** off the render-critical window (`load` →
  `requestIdleCallback`) so it doesn't compete with the above-fold poster/fonts;
  a live un-reduce still boots promptly via the mq listener. The dimmer
  controller honours a per-block `data-lit-max`: the hero's `data-lit` block caps
  its darkness at 0.74 (below the shared `DIM_MAX` 0.86) so the LIVE office reads
  above the fold, while downpage statement holds keep `DIM_MAX` for copy
  legibility (the `[data-lit]::before` radial wash still floors local contrast).
- **The ♩ office-sound toggle (`#office-audio`, #633 web-audio).** The hero's
  `Office` runs the real Rust `WebAudioDriver` (`pixtuoid-web`'s `audio.rs` — the
  SAME scene mixer/schedulers/TrackSwitch the desktop app runs); `OfficeBackdrop.astro`
  is dumb WebAudio glue over its per-tick JSON commands (the `overlay_json` split,
  audio edition). **Muted-by-default + gesture-gated** (browser autoplay policy):
  no `AudioContext` until the first ♩ click, which `office.audio_begin()`s, pumps
  the chunked `office.audio_warmup_step()` off `setTimeout(0)` (the multi-second
  bank/rain/beds synth never blocks the main thread), uploads the buffers via the
  zero-copy `audio_loop_ptr/_len` + `audio_oneshot_ptr/_len` getters (COPIED out —
  a view dangles on `memory.grow`), then `office.audio_tick(nowMs)` per frame,
  applying `{gains[6],plays,swapped}` to looping `AudioBufferSourceNode`s + a
  `GainNode` each. Sound rides the SAME pause-shifted `nowMs` as the render, so
  `#office-pause` + `visibilitychange` `suspend()`/`resume()` the context in
  lockstep (a frozen office never drones). The ♩ un-hides only once the office is
  live; a remembered "on" choice (`localStorage`) restores on the FIRST user
  gesture (never auto-plays on load). The button STACKS above `#office-pause`
  (same right edge → the ≤760px container reservation still covers one column).
  Regenerate `public/wasm/` (`just gen-wasm`) whenever the `Office` audio surface
  changes — the glue exports must match. Pinned by `smoke.spec.ts`'s existing
  backdrop contracts (the audio layer adds no throw path). **iOS silent-switch
  (#664, DELIBERATE — don't revert to respecting it):** on the ♩ click the glue
  sets `navigator.audioSession.type = 'playback'` BEFORE constructing the
  `AudioContext`. iOS Safari routes default WebAudio to the ambient channel, so the
  hardware Ring/Silent switch (very commonly left ON) mutes it — a user who
  deliberately taps ♩ then hears nothing, reading as broken. `'playback'` routes to
  the media channel (like a `<video>`), honouring the explicit opt-in even on
  silent. Accepted trade-off: `'playback'` is the only NON-RECORDING AudioSession
  category that bypasses the switch (`play-and-record` also would, but needs a mic
  prompt), and it's non-mixing, so it can pause other apps' audio (Spotify) when ♩
  is tapped — acceptable since ♩ is a deliberate gesture. The API
  is Safari-only; the `'audioSession' in navigator` guard no-ops elsewhere, so it's
  a pure iOS enhancement CI can't exercise (the smoke test mocks the API to pin the
  wiring; the real silent-switch behavior is device-verified).
- **The showcase `VIBING` channel is a SECOND live `Office` (#468).** The CRT
  showcase (`Showcase.astro` + `ChannelStage.astro`, driven by `src/showcase.json`
  → `src/consts.ts`) has one `kind:"live"` channel, `vibing`, whose screen is a
  real `pixtuoid-web::Office` in a `<canvas>` — a time slider + weather chips +
  theme chips let a visitor scrub the office's time-of-day / weather / theme. It
  REPLACED the static `weather` + `themes` channels (their stills retired). Two
  load-bearing facts: (1) it's the SECOND `Office` on the page (the hero backdrop
  is the first), sharing the ONE browser-cached wasm module — and `force_weather`
  is a **thread-local shared by both**, so each `Office::step` re-applies its own
  weather every frame (Rust-side invariant); a naive one-shot set would hijack the
  hero. (2) Its `Showcase.astro` controller runs its rAF loop ONLY when the channel
  is active + in-view + not `userPaused` + not reduced-motion (`syncCanvas`, wired
  into the same 4 sites as `syncVideos` and JOINING the `pix:paused` set — it's a
  consumer, never a dispatcher of `#office-pause`), feeding a synthetic
  `Date.now()`-based `now_ms` for the time scrub. Theme chips call `Office::set_theme`
  AND decorative-retint the page via the shared `retintPage()` (they do NOT dispatch
  `pix:theme` — that would clobber other channels' chip state). The two live-office
  consumers (this backdrop and Showcase's VIBING controller) MUST share ONE `init()`
  call via `window.__pixWasm` — the generated glue's `__wbg_init` guards only the
  already-resolved instance, not an in-flight promise, so independent `mod.default()`
  calls racing before either resolves instantiate two separate wasm instances that
  stomp the single module-global `wasm`. A bounded retry (2×, 400ms backoff) wraps
  that shared init for transient wasm-fetch resilience, but stays INSIDE the one
  `__pixWasm` promise — a per-consumer retry would reintroduce the very
  double-instantiate race above (the retry consts are pinned equal across the two
  boot scripts by `config/wasm-init-consts.test.mjs`). On FINAL exhaustion the
  promise is NULLED so a later-booting consumer re-attempts — safe because it only
  clears a SETTLED (rejected) promise, never an in-flight one (#671). Schema: `kind:"live"`
  + `variantGroups` (per-group `retint`) + `poster` + `timeSlider` in `showcase.json`,
  resolved by `showcaseGroups` in `consts.ts`, validated by the `astro.config.mjs`
  showcase guard's live branch. Fallback (no-JS / no-wasm / reduced-motion): the
  static `vibing-poster.png` (gen'd via `hero_still --hour 18 --weather clear`).
- **Scoped `<style>` does NOT reach `set:html` content.** Astro scopes component
  styles by stamping a `data-astro-*` hash on template elements AND selectors;
  markup injected at runtime via `set:html` (e.g. the SupportedTools per-OS
  pixel-check marks from `MARK()`) carries no hash, so scoped rules silently miss
  it — target it with `:global(...)`. (The tools checks rendered black + mis-sized
  until the `.tools__mark*` rules were `:global`; caught only by rendering, not by
  the static gates.)
- The **on-page nav + footer logo mark IS the favicon** — `public/favicon-32.png`
  / `favicon-32-night.png` (the head-and-collar bust squircle from #379), one
  brand asset in two roles so there's no second file to drift (the old separate
  `char-mark.png` silently diverged from the icon for a month). `Nav.astro` /
  `Footer.astro` render it via the `.js-brand-mark` class, and `Base.astro`'s
  `syncBrand` swaps BOTH the tab favicon and those marks day↔night together
  (night ⇔ any non-day theme). Don't reintroduce a separate mark asset or drop
  the `.js-brand-mark` hook. Size the mark to 32 (1:1) or an integer fraction
  (footer uses 16) so the pixel bust stays device-exact.
- `src/assets/pix-icons/` is GENERATED by `scripts/gen-pix-icons.py` from the
  embedded sprite pack's own palette
  (`crates/pixtuoid-scene/sprites/default/pack.toml`) — an icon may only use
  pack-defined palette keys, so it can never drift off the office's own
  colors. Never hand-edit the PNGs (regenerate via `just gen-icons`, folded
  into `just gen`); drift is gated by `just gen-check`
  (`scripts/gen-pix-icons.py --check`, decode-compared like `gen-media.py`'s
  check, not raw-byte compared — Pillow re-encoding is version-fragile).
  `PixIcon.astro` fails the build on an unknown icon name;
  `config/pix-icons.test.mjs` bridges `features.json`'s `pix` names to the
  generated PNGs.

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

- **`script-src` carries no `'unsafe-inline'`** — every inline script is
  whitelisted by content hash, recomputed on each build. Adding/editing an
  `is:inline` script needs NO manual CSP step.
- **`style-src` keeps `'unsafe-inline'` and must stay hash-free**: Shiki
  spans, the build-time mermaid SVG, and the few `style={}` attributes are
  inline style ATTRIBUTES, which hashes cannot express (one present hash
  disables `unsafe-inline` for the whole directive).
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
