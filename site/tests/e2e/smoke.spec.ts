import { expect, test, type Page } from '@playwright/test';
import sourcesData from '../../src/sources.json' with { type: 'json' };

// read the manifest directly (same idiom as floors.spec.ts's features.json
// import) so the hero-badge bridge test below can't drift from a hand-copied
// expected count.
type SourceRow = { badge: string; badge_color: string; name: string; status: string };
const supportedSources = (sourcesData as SourceRow[]).filter((s) => s.status === 'supported');

// The smoke suite: one assertion per cross-component CONTRACT of the OPEN
// FLOOR page — the seams that only exist at runtime (window globals, custom
// events, data-attribute wiring) where tsc/eslint/knip/astro-build are blind.
// The first seven tests are regression pins for bug classes a human review
// actually caught on this site:
//   - the missed one-shot `pix:onair` event (statusline read STATIC forever)
//   - the `is:inline` parse-position trap (scrollspy frozen on floor 6)
//   - the floating-nav variant leaking onto the docs pages
//   - a wasm/glue ABI mismatch throwing at runtime under the hero
// Runs against the PRODUCTION build (see playwright.config.ts).

/**
 * WCAG 2.1 relative luminance + contrast ratio (per the spec's definitions),
 * plus the minimal alpha-compositing needed to pin `.text-scrim`'s worst case:
 * the scrim is painted over the dimmer, which is itself translucent over the
 * live office. Kept fn-local (used by one test) rather than a shared util.
 */
function relLuminance([r, g, b]: [number, number, number]): number {
  const lin = (c: number) => {
    const s = c / 255;
    return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}
function contrastRatio(a: [number, number, number], b: [number, number, number]): number {
  const [la, lb] = [relLuminance(a), relLuminance(b)];
  const [hi, lo] = la > lb ? [la, lb] : [lb, la];
  return (hi + 0.05) / (lo + 0.05);
}
function compositeOver(
  [r, g, b, a]: [number, number, number, number],
  under: [number, number, number]
): [number, number, number] {
  const [ur, ug, ub] = under;
  return [r * a + ur * (1 - a), g * a + ug * (1 - a), b * a + ub * (1 - a)];
}
function parseRgb(css: string): [number, number, number, number] {
  const rgb = css.match(/rgba?\(([^)]+)\)/);
  if (rgb) {
    const [r, g, b, a] = rgb[1].split(',').map((s) => parseFloat(s));
    return [r, g, b, a ?? 1];
  }
  // Chromium resolves a color-mix() result to the `color(srgb r g b [/ a])`
  // functional form (0–1 components), not rgb() — the hero badge-code hue
  // (color-mix toward white/black) hits this path; every other caller still
  // sees plain rgb()/rgba() and takes the branch above.
  const srgb = css.match(/color\(srgb\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)(?:\s*\/\s*([\d.]+))?\)/);
  if (srgb) {
    const [r, g, b, a] = srgb.slice(1, 5).map((s) => (s === undefined ? undefined : parseFloat(s)));
    return [r! * 255, g! * 255, b! * 255, a ?? 1];
  }
  throw new Error(`unparseable color: ${css}`);
}
/**
 * Fail the calling test if the page logs an uncaught error or console.error.
 * Attached once per DISTINCT code path (index live boot, copy/hire, docs
 * shell, reduced-motion) rather than every test — keeps failures pointed.
 */
function watchErrors(page: Page): () => string[] {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
  page.on('console', (msg) => {
    if (msg.type() === 'error') errors.push(`console.error: ${msg.text()}`);
  });
  return () => errors;
}

/**
 * Scroll a section to viewport center and expect its head to reveal (`in`).
 * The scroll is INSIDE the retry: a one-shot scrollIntoView races the two
 * things that keep moving the page under a slow (CI-throttled) load —
 * Chromium's async scroll restoration after reload() (clamped retries while
 * the document grows) and late layout settling — either can park the viewport
 * where the head never intersects the 0.12 observer threshold. Re-scrolling
 * per retry pins the geometry the assert depends on. (Reproduced identically
 * on the Astro 6 build under 10x CPU throttle — a test-timing hazard, not a
 * product one: the observer fires whenever the head actually intersects.)
 */
async function expectSectionReveal(page: Page, sectionId: string): Promise<void> {
  await expect(async () => {
    await page.evaluate(
      (id) => document.getElementById(id)!.scrollIntoView({ block: 'center', behavior: 'instant' }),
      sectionId
    );
    await expect(page.locator(`#${sectionId} .section-head.reveal`)).toHaveClass(/\bin\b/, {
      timeout: 500,
    });
  }).toPass({ timeout: 10_000 });
}

/** Load the landing page with the boot intro pre-skipped and the office live. */
async function gotoLive(page: Page): Promise<void> {
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  // The wasm office must come up: poster → live canvas. 15s is generous — a
  // timeout here is the ABI-mismatch / loader-regression signal.
  await expect(page.locator('.backdrop.is-live')).toBeAttached({ timeout: 15_000 });
}

test('the office goes live and the statusline truth-light agrees', async ({ page }) => {
  const errors = watchErrors(page);
  await gotoLive(page);
  // The on-air readout must say LIVE — covers BOTH orderings of the one-shot
  // pix:onair event vs the statusline's listener (the seed-from-class fix).
  await expect(page.locator('[data-sl-onair]')).toHaveText('● LIVE', { timeout: 10_000 });
  // Resize re-aspects the render buffer (rAF-throttled sizeBuffer): the buffer
  // height is fixed at 130, so width = min(640, max(64, round(w/h · 130))) —
  // 231 at the 1280×720 default, 72 at a 500×900 portrait.
  const bufW = () =>
    page.evaluate(() => (document.getElementById('office-live') as HTMLCanvasElement).width);
  expect(await bufW()).toBe(231);
  await page.setViewportSize({ width: 500, height: 900 });
  await expect.poll(bufW).toBe(72);
  expect(errors()).toEqual([]);
});

test('the cross-component window contracts exist', async ({ page }) => {
  await gotoLive(page);
  // The runtime seams every component wires against (documented in
  // site/README.md "Cross-component seams") — a rename breaks consumers
  // silently, so pin their existence + shapes here.
  await expect
    .poll(async () =>
      page.evaluate(() => ({
        night: typeof window.__pixNight === 'function' && typeof window.__pixNight() === 'boolean',
        hire: typeof window.__pixHire === 'function',
        lights: typeof window.__pixLights,
        // the office-reveal boot handshake (PR #462): Base publishes __pixRevealed
        // (splash lifted) to release the roll; OfficeBackdrop publishes
        // __pixEngineReady (engine resolved) to release the Level-2 splash gate.
        revealed: window.__pixRevealed === true,
        engineReady: window.__pixEngineReady === true,
      }))
    )
    .toEqual({ night: true, hire: true, lights: 'number', revealed: true, engineReady: true });
});

test('digit keys ride between floors (scrollspy round-trip)', async ({ page }) => {
  await gotoLive(page);
  // Key "3" → the machine-room floor. Covers the is:inline parse-position
  // trap (an observer wired before <main> parses sees zero [data-floor]
  // sections and the readout freezes on 6F).
  await page.keyboard.press('3');
  await expect(page.locator('[data-lift-digit]')).toHaveText('3F', { timeout: 10_000 });
  await page.keyboard.press('1');
  await expect(page.locator('[data-lift-digit]')).toHaveText('1F', { timeout: 10_000 });
});

test('scrolled to the true page bottom, the statusline clamps to the last floor', async ({
  page,
}) => {
  // 1F (install) + the footer rarely fill the observer's -45%/-45% middle
  // band, so without a bottom clamp the readout can freeze one floor short
  // while the visitor reads the very end of the page. Force actual max scroll
  // (not a fixed pixel guess — page height varies by content/viewport); retry
  // a few times since late layout settling can still grow the page after the
  // first scrollTo lands.
  await gotoLive(page);
  await expect(async () => {
    await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
    await expect(page.locator('[data-lift-digit]')).toHaveText('1F', { timeout: 500 });
  }).toPass({ timeout: 10_000 });
});

test('the dimmer darkens statements and releases in office gaps', async ({ page }) => {
  await gotoLive(page);
  const dim = () =>
    page.evaluate(() => parseFloat(document.getElementById('dimmer')!.style.opacity || '0'));
  // A statement at viewport center pulls the darkness in…
  await page.evaluate(() =>
    document.getElementById('install')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  await expect.poll(dim).toBeGreaterThan(0.5);
  // …and the first observation gap releases it (the office IS the content).
  await page.evaluate(() =>
    document.querySelector('.office-gap')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  await expect.poll(dim).toBeLessThan(0.15);
  // The hero is a data-lit="fade" block: while a statement owns the viewport
  // center it parks at 0.001 (the invisible-headline class), and rises back
  // when the office scrolls up again.
  const heroOp = () =>
    page.evaluate(() =>
      parseFloat((document.querySelector('.hero__copy') as HTMLElement).style.opacity || '1')
    );
  await page.evaluate(() =>
    document.getElementById('install')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  await expect.poll(heroOp).toBeLessThan(0.01);
  await page.evaluate(() => window.scrollTo({ top: 0, behavior: 'instant' }));
  await expect.poll(heroOp).toBeGreaterThan(0.5);
});

// Regression pin for a real (if brief) dimmer glitch found while investigating
// a reported "dimmer jumps/strobes near the floating-window gap" bug: a
// Showcase channel swap changes content height ABOVE the viewport (every live
// channel renders a different height — measured ~8224px to ~8306px across the
// 7 channels), and the browser's OWN scroll anchoring adjusts window.scrollY
// to compensate *in the same task* as the swap — before OfficeBackdrop's
// ResizeObserver-triggered re-measure ever runs. The gap-2 image itself turned
// out NOT to be the cause (its box is pinned by `aspect-ratio: 16/10`
// regardless of load state — see the sibling test below); the real mechanism
// is this content-reflow-above-the-viewport race, and gap-2 sits directly
// downstream of Showcase so it's exactly where a visitor would see it.
// OfficeBackdrop.astro's recompute() now runs synchronously from the
// ResizeObserver callback (no extra rAF hop) so the corrected opacity lands
// in the same task as the reflow, before paint.
test('the dimmer tracks live geometry across a Showcase channel swap', async ({ page }) => {
  await gotoLive(page);
  // Straddle the gap-2 observation hold (between Features and HowItWorks) —
  // the reported bug's location.
  await page.evaluate(() => window.scrollTo({ top: 3763, behavior: 'instant' }));

  // Ground truth: replicate the dimmer's own ease()/cap/best-block formula
  // against LIVE getBoundingClientRect() rects and the LIVE (possibly
  // scroll-anchor-adjusted) scrollY — independent of whatever the controller
  // has cached.
  const liveTruth = () =>
    page.evaluate(() => {
      const y = window.scrollY;
      const innerH = window.innerHeight;
      const center = y + innerH / 2;
      const reach = innerH * 0.55;
      let best = 0;
      let bestCap = 0.86;
      document.querySelectorAll<HTMLElement>('[data-lit]').forEach((el) => {
        const r = el.getBoundingClientRect();
        const top = r.top + y;
        const bottom = r.bottom + y;
        const d = center < top ? top - center : center > bottom ? center - bottom : 0;
        const p = d >= reach ? 0 : 1 - d / reach;
        if (p > best) {
          best = p;
          bestCap = el.dataset.litMax ? parseFloat(el.dataset.litMax) : 0.86;
        }
      });
      const ease = (t: number) => t * t * (3 - 2 * t);
      return bestCap * ease(best);
    });
  const pageOp = () =>
    page.evaluate(() => parseFloat(document.getElementById('dimmer')!.style.opacity || '0'));

  // Cycle through every live channel (each a genuine reflow above the
  // viewport) and confirm the dimmer converges to the live ground truth —
  // not a value cached from before the swap.
  const channels = ['agents', 'openclaw', 'dashboard', 'meetings', 'pets', 'spaces', 'vibing'];
  for (const ch of channels) {
    await page.evaluate(
      (id) => (document.querySelector(`.dial__ch[data-ch="${id}"]`) as HTMLElement | null)?.click(),
      ch
    );
    await expect
      .poll(async () => Math.abs((await pageOp()) - (await liveTruth())), {
        message: `dimmer opacity vs live ground truth after switching to "${ch}"`,
      })
      .toBeLessThan(0.01);
  }
});

test('the hero pause switch freezes the office and resumes it seamlessly', async ({ page }) => {
  // WCAG 2.2.2: the auto-playing office backdrop can be paused. Pause must
  // STOP the rAF loop dead (a frozen canvas, byte-identical snapshots — not
  // merely a hidden button), and resume must paint new frames again.
  const errors = watchErrors(page);
  await gotoLive(page);
  const btn = page.locator('#office-pause');
  await expect(btn).toBeVisible(); // shown at init for any non-reduced-motion visitor (syncPauseBtn), independent of the office going live
  await expect(btn).toHaveAttribute('aria-pressed', 'false');
  const shot = () =>
    page.evaluate(() => (document.getElementById('office-live') as HTMLCanvasElement).toDataURL());
  const bufW = () =>
    page.evaluate(() => (document.getElementById('office-live') as HTMLCanvasElement).width);
  await btn.click();
  await expect(btn).toHaveAttribute('aria-pressed', 'true');
  const frozen = await shot();
  await page.waitForTimeout(400); // >10 would-be frames at the 33ms cap
  expect(await shot()).toBe(frozen); // not one new frame painted
  // Pause-unify (WCAG 2.2.2 covers the whole page): the statusline reflects the
  // paused office — PAUSED, not '● LIVE'.
  await expect(page.locator('[data-sl-onair]')).toHaveText('❚❚ PAUSED');
  // Resize while paused: sizeBuffer() wipes the bitmap and no rAF will repaint
  // it, so the resize handler must re-render the ONE frozen frame — a blank
  // var(--bg) void here is the exact regression this branch prevents.
  await page.setViewportSize({ width: 500, height: 900 });
  await expect.poll(bufW).toBe(72); // re-aspected
  expect(await btn.getAttribute('aria-pressed')).toBe('true'); // still paused
  const painted = await page.evaluate(() => {
    const c = document.getElementById('office-live') as HTMLCanvasElement;
    const d = c.getContext('2d')!.getImageData(0, 0, c.width, c.height).data;
    return d.some((v) => v !== 0);
  });
  expect(painted).toBe(true); // the frozen frame, not a void
  const frozen2 = await shot(); // frozen at the new aspect
  await page.waitForTimeout(400);
  expect(await shot()).toBe(frozen2); // pause survives the resize
  // Keyboard operability: the switch is a real button — Enter resumes.
  await btn.focus();
  await page.keyboard.press('Enter');
  await expect(btn).toHaveAttribute('aria-pressed', 'false');
  await expect.poll(shot, { timeout: 10_000 }).not.toBe(frozen2); // animating again
  await expect(page.locator('[data-sl-onair]')).toHaveText('● LIVE'); // back to live
  expect(errors()).toEqual([]);
});

test('the hero ♩ sound toggle: muted by default, gesture-gated, no AudioContext until clicked', async ({
  page,
}) => {
  // #633 web-audio. Muted-by-default + browser autoplay policy: NO AudioContext
  // may be constructed before a user gesture. Instrument the constructor (the
  // only observable proof) before the page's own script runs.
  await page.addInitScript(() => {
    (window as unknown as { __acCount: number }).__acCount = 0;
    const Real =
      window.AudioContext ||
      (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!Real) return;
    const Wrapped = function (this: unknown, ...args: unknown[]) {
      (window as unknown as { __acCount: number }).__acCount++;
      return new (Real as new (..._a: unknown[]) => AudioContext)(...args);
    } as unknown as typeof AudioContext;
    Wrapped.prototype = Real.prototype;
    window.AudioContext = Wrapped;
    (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext = Wrapped;
  });
  const errors = watchErrors(page);
  await gotoLive(page);
  const btn = page.locator('#office-audio');
  const acCount = () => page.evaluate(() => (window as unknown as { __acCount: number }).__acCount);
  // un-hidden only once a live office exists to sound
  await expect(btn).toBeVisible({ timeout: 15_000 });
  // default: muted + ZERO AudioContexts — the autoplay-policy contract (no
  // sound machinery may spin up before a user gesture).
  await expect(btn).toHaveAttribute('aria-pressed', 'false');
  expect(await acCount()).toBe(0);
  // one click = one gesture = exactly one AudioContext constructed. Then wait
  // through the warmup + several ticks: on a real audio device it plays; in
  // headless (no backend) createBuffer throws and the ♩ degrades GRACEFULLY
  // (audioDisable → hidden), never an uncaught page error. Either way: no throw.
  await btn.click();
  await expect.poll(acCount).toBe(1);
  await page.waitForTimeout(3000); // beds synth (~1-2s) + live ticks
  // playing (audio available) OR gracefully hidden (no backend) — never a
  // second context, never a crash.
  expect(await acCount()).toBe(1);
  expect(errors()).toEqual([]);
});

test('a remembered ♩ choice never inverts a direct first click on the button', async ({ page }) => {
  // Review HIGH: the remembered-"on" restore installs capture-phase
  // pointerdown/keydown listeners; if the visitor's FIRST gesture is a direct
  // click on ♩, a naive restore fires (→on) then the button's own click toggles
  // (→off), silently muting despite the click. The restore must ignore a gesture
  // ON the button and let its click own the toggle. The bug's signature is the
  // button left VISIBLE-AND-MUTED after a direct click; the fix leaves it
  // playing (aria-pressed true) — or, where WebAudio has no backend (headless),
  // gracefully hidden. What must NEVER happen: visible + aria-pressed false.
  const errors = watchErrors(page);
  await gotoLive(page);
  await page.evaluate(() => localStorage.setItem('pix:audio', '1'));
  await page.reload();
  await expect(page.locator('.backdrop.is-live')).toBeAttached({ timeout: 15_000 });
  const btn = page.locator('#office-audio');
  await expect(btn).toBeVisible({ timeout: 15_000 });
  await btn.click();
  await expect
    .poll(async () => {
      const pressed = await btn.getAttribute('aria-pressed');
      const hidden = await btn.evaluate((el) => (el as HTMLElement).hidden);
      return (pressed === 'true' && !hidden) || (hidden && pressed === 'false');
    })
    .toBe(true);
  // the inversion bug's exact signature — visible AND muted after the click:
  const invertedBug =
    (await btn.getAttribute('aria-pressed')) === 'false' &&
    !(await btn.evaluate((el) => (el as HTMLElement).hidden));
  expect(invertedBug).toBe(false);
  expect(errors()).toEqual([]);
});

test('crisp AA captions overlay the live office (name badges + neon board)', async ({ page }) => {
  // The office canvas is a ~130px buffer CSS-upscaled with image-rendering:
  // pixelated, so text baked into it pixelates. Instead the engine exports the
  // name badges + neon wall-board (Office.overlay_json) and OfficeBackdrop lays
  // crisp Monaspace Neon DOM spans over the canvas at display resolution. Pin
  // that the layer comes up, carries real text, and is actually the mono face.
  const errors = watchErrors(page);
  await gotoLive(page);
  // The caption layer fades in only AFTER the reveal roll settles (labels track
  // the FINAL sprite positions), so wait on is-on — not merely is-live.
  await expect(page.locator('#office-overlay.is-on')).toBeAttached({ timeout: 10_000 });
  // At least one name badge, laid over a seated/walking-in agent, non-empty and
  // in Monaspace Neon (10s covers the cast's staggered walk-in at loop start).
  const label = page.locator('#office-overlay .ov-label').first();
  await expect(label).toHaveText(/\S/, { timeout: 10_000 });
  const labelFont = await label.evaluate((el) => getComputedStyle(el).fontFamily);
  expect(labelFont).toContain('Monaspace Neon');
  // The label splits into ●-dot/name child spans — the dot carries the
  // activity tone, the name the CLI's badge hue (#657): the same per-CLI
  // color the app's dashboard uses.
  const parts = await label.evaluate((el) =>
    Array.from(el.children).map((c) => ({
      text: c.textContent,
      color: (c as HTMLElement).style.color,
    }))
  );
  expect(parts).toHaveLength(2);
  expect(parts[0].text).toBe('●');
  expect(parts[1].text).toMatch(/^[a-z]{2}·/);
  expect(parts[1].color).not.toEqual('');
  // The neon wall board renders its brand row (● / ★) from the same model.
  const brand = page.locator('#office-overlay .ov-board .ov-brow--top span').first();
  await expect(brand).toHaveText(/\S/, { timeout: 10_000 });
  const brandFont = await brand.evaluate((el) => getComputedStyle(el).fontFamily);
  expect(brandFont).toContain('Monaspace Neon');
  expect(errors()).toEqual([]);
});

test('reduced motion hides the caption overlay (still poster, no captions)', async ({
  browser,
}) => {
  // Reduced motion is the still-poster design: no live office, so no captions.
  // The overlay is display:none there — assert it never becomes visible.
  const context = await browser.newContext({ reducedMotion: 'reduce' });
  const page = await context.newPage();
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  await expect(page.locator('.backdrop.is-live')).not.toBeAttached();
  await expect(page.locator('#office-overlay.is-on')).not.toBeAttached();
  await expect(page.locator('#office-overlay')).toBeHidden();
  await context.close();
});

test('the install Copy click hires without breaking the page', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-write']);
  const errors = watchErrors(page);
  await gotoLive(page);
  await page.evaluate(() =>
    document.getElementById('install')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  const copy = page.locator('.install__panel.is-active .install__copy');
  await copy.click();
  // The copy flash proves the click handler ran to completion — i.e. the
  // post-copy pix:install-copy dispatch (OfficeBackdrop's hire listener) didn't throw.
  await expect(copy).toHaveText(/Copied|Select & copy/);
  expect(errors()).toEqual([]);
});

test('the hire cap stops the receipt at 3 but keeps hiring every time', async ({
  page,
  context,
}) => {
  // The engine's own bool return is now the ONE admission signal (see
  // `Office::hire`'s contract, pixtuoid-web/src/lib.rs) — no JS-side mirror of
  // `VisitorHires::MAX_LIVE` to drift out of lockstep. This test pins BOTH
  // halves: the cap VALUE (3, via the receipts) and the keep-attempting
  // BEHAVIOR (the clipboard/copy path must never look broken even once the
  // engine has quietly refused a hire past its cap — the 4th call still runs,
  // it just returns false). Drives the REAL Install-section copy control
  // (wb-2: the statusline chip that used to drive this is now a plain jump
  // link — Install.astro's own tabs are the surviving install-copy surface).
  await context.grantPermissions(['clipboard-write']);
  const errors = watchErrors(page);
  await gotoLive(page); // hire needs the LIVE office (__pixHire exists)
  await page.evaluate(() =>
    document.getElementById('install')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  await page.evaluate(() => {
    (window as unknown as { __hired: string[] }).__hired = [];
    document.addEventListener('pix:hired', (e) =>
      (window as unknown as { __hired: string[] }).__hired.push(
        (e as CustomEvent<{ name: string }>).detail.name
      )
    );
    // Instrument the REAL Office.hire() call BEFORE firing any copies — must
    // forward its bool return, or the admission signal the listener gates
    // pix:hired on goes missing.
    const real = window.__pixHire!;
    (window as unknown as { __hireResults: boolean[] }).__hireResults = [];
    window.__pixHire = function () {
      const admitted = real();
      (window as unknown as { __hireResults: boolean[] }).__hireResults.push(admitted);
      return admitted;
    };
  });
  const copy = page.locator('.install__panel.is-active .install__copy');
  for (let i = 0; i < 4; i++) {
    await copy.click();
    // wait for THIS click's hire() result to land before firing the next —
    // each click's clipboard-write → pix:install-copy → hire() chain is async.
    await expect
      .poll(() =>
        page.evaluate(
          () => (window as unknown as { __hireResults: boolean[] }).__hireResults.length
        )
      )
      .toBe(i + 1);
  }
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __hired: string[] }).__hired))
    .toEqual(['cc·yours', 'cc·yours', 'cc·yours']); // receipt caps at MAX_LIVE (3), not 4
  expect(
    await page.evaluate(() => (window as unknown as { __hireResults: boolean[] }).__hireResults)
  ).toEqual([true, true, true, false]); // hire() runs every time; only the 4th is refused
  expect(errors()).toEqual([]);
});

test('reduced motion: an install copy writes the clipboard but hires nobody', async ({
  browser,
}) => {
  // The no-wasm strand of the same finding: under reduced motion the wasm
  // fetch never runs, so window.__pixHire is never published. Install.astro's
  // own copy button (the surviving install-copy control — the statusline
  // chip is a plain jump link) must still succeed writing the clipboard (that
  // path is independent of the office) and OfficeBackdrop's
  // `if (!window.__pixHire) return;` guard must make the hire side a true
  // no-op — no throw, no pix:hired receipt.
  const context = await browser.newContext({
    reducedMotion: 'reduce',
    permissions: ['clipboard-read', 'clipboard-write'],
  });
  const page = await context.newPage();
  const errors = watchErrors(page);
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  await expect(page.locator('.backdrop.is-live')).not.toBeAttached();
  await page.evaluate(() => {
    (window as unknown as { __hired: string[] }).__hired = [];
    document.addEventListener('pix:hired', (e) =>
      (window as unknown as { __hired: string[] }).__hired.push(
        (e as CustomEvent<{ name: string }>).detail.name
      )
    );
  });
  await page.evaluate(() =>
    document.getElementById('install')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  const copy = page.locator('.install__panel.is-active .install__copy');
  await copy.click();
  await expect(copy).toHaveText(/Copied|Select & copy/);
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    'brew install IvanWng97/pixtuoid/pixtuoid'
  );
  await page.waitForTimeout(500); // settle window: no late/async hire lands
  expect(await page.evaluate(() => (window as unknown as { __hired: string[] }).__hired)).toEqual(
    []
  );
  expect(errors()).toEqual([]);
  await context.close();
});

test('docs pages keep the sticky nav with section links', async ({ page }) => {
  // The floating-nav treatment is index-ONLY; the docs pages have no office
  // backdrop (they DO mount the statusline since wb-5), so they keep the sticky bar (the #426-review
  // regression: `nav--floating` leaked here — absolute, transparent, links
  // hidden — and every scroll offset went stale).
  const errors = watchErrors(page);
  await page.goto('./config');
  const nav = page.locator('.nav');
  await expect(nav).not.toHaveClass(/nav--floating/);
  await expect
    .poll(() => page.evaluate(() => getComputedStyle(document.querySelector('.nav')!).position))
    .toBe('sticky');
  await expect(page.locator('.nav__section-link').first()).toBeVisible();
  // The docs shell has its own script surface (sidebar scrollspy, pager,
  // inline mermaid SVG) the index tests never visit — keep it error-free too.
  expect(errors()).toEqual([]);
});

test('reduced motion stays on the still poster without errors', async ({ browser }) => {
  // A complete parallel design: no wasm fetch, the poster is the office, the
  // dimmer holds a constant CSS level. Must be error-free — reduced-motion
  // visitors see this forever.
  const context = await browser.newContext({ reducedMotion: 'reduce' });
  const page = await context.newPage();
  const errors = watchErrors(page);
  const wasmRequests: string[] = [];
  page.on('request', (r) => {
    if (r.url().includes('/wasm/')) wasmRequests.push(r.url());
  });
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  await expect(page.locator('.backdrop__poster')).toBeVisible();
  // Deterministic (no fixed wait): by network-idle a would-be boot would have
  // fetched the wasm glue and published __pixHire — assert neither happened.
  await page.waitForLoadState('networkidle');
  expect(wasmRequests).toEqual([]);
  await expect(page.locator('.backdrop.is-live')).not.toBeAttached();
  // Reduced motion is the ONLY path that hides the pause switch: nothing
  // auto-animates here (the wasm-fail poster keeps it visible — ticker/dust/clips
  // still run there, see the wasm-failure test).
  await expect(page.locator('#office-pause')).toBeHidden();
  // Reduced motion also strips the showcase clip's autoplay: native controls
  // appear and the video stays paused (WCAG 2.2.2).
  const video = page.locator('[data-stage="agents"] video');
  await expect(video).toHaveAttribute('controls', '');
  await expect.poll(() => video.evaluate((v) => (v as HTMLVideoElement).paused)).toBe(true);
  // The proof clip never hydrates under reduced motion: poster only (§3).
  const proofVid = page.locator('.proof__video--wide');
  expect(await proofVid.evaluate((v) => v.querySelectorAll('source').length)).toBe(0);
  await expect(proofVid).toHaveAttribute('poster', /proof-poster/);
  await expect.poll(() => proofVid.evaluate((v) => (v as HTMLVideoElement).paused)).toBe(true);
  expect(errors()).toEqual([]);
  await context.close();
});

test('wasm fetch failure keeps the still poster without an uncaught error', async ({ browser }) => {
  // The third documented boot path (live / reduced-motion / FAILURE): abort every
  // wasm request so the dynamic import rejects — the empty .catch must keep the
  // poster (graceful degradation) and never throw. The pause control stays present
  // though: it governs the wasm-independent ambient motion (ticker/dust/clips), so
  // a failed office must NOT strand that motion uncontrollable (#456).
  const context = await browser.newContext();
  const page = await context.newPage();
  const errors = watchErrors(page);
  await page.route('**/wasm/**', (r) => r.abort());
  const wasmTried: string[] = [];
  page.on('request', (r) => {
    if (r.url().includes('/wasm/')) wasmTried.push(r.url());
  });
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  // the boot is deferred to load+idle — wait until it actually attempted the fetch
  await expect.poll(() => wasmTried.length, { timeout: 15_000 }).toBeGreaterThan(0);
  await page.waitForLoadState('networkidle');
  await expect(page.locator('.backdrop__poster')).toBeVisible();
  await expect(page.locator('.backdrop.is-live')).not.toBeAttached();
  await expect(page.locator('[data-sl-onair]')).toHaveText('○ STATIC');
  // #456: the office canvas never went live, but the statusline ticker / hero dust
  // / showcase clips still auto-animate — so the pause control must be VISIBLE and
  // actually govern them (WCAG 2.2.2), not hidden as if nothing were animating.
  // Clicking it fires the page-wide pix:paused even with no live office.
  const pauseBtn = page.locator('#office-pause');
  await expect(pauseBtn).toBeVisible();
  const paused = page.evaluate(
    () =>
      new Promise<boolean>((resolve) => {
        document.addEventListener('pix:paused', (e) => resolve((e as CustomEvent).detail.paused), {
          once: true,
        });
      })
  );
  await pauseBtn.click();
  expect(await paused).toBe(true);
  await expect(pauseBtn).toHaveAttribute('aria-pressed', 'true');
  // the aborted request logs a resource error; the import rejection must stay
  // handled — no uncaught pageerror / console.error beyond that one line.
  expect(errors().filter((e) => !e.includes('Failed to load resource'))).toEqual([]);
  await context.close();
});

test('key vocabulary: digits ride globally, typing surfaces stay guarded, t keeps its gate', async ({
  page,
}) => {
  await gotoLive(page);
  await page.keyboard.press('3');
  await expect(page.locator('[data-lift-digit]')).toHaveText('3F', { timeout: 10_000 });
  // The audit's dead-digit-keys bug, pinned FIXED (§4): focus parked on a real
  // control no longer kills the floor keys — digits are document-global now.
  await page.locator('#office-pause').focus();
  await page.keyboard.press('1');
  await expect(page.locator('[data-lift-digit]')).toHaveText('1F', { timeout: 10_000 });
  // …but a typing surface still swallows them (no teleport mid-input).
  await page.evaluate(() => {
    const inp = document.createElement('input');
    inp.id = 'e2e-typing-probe';
    document.body.appendChild(inp);
    inp.focus();
  });
  await page.keyboard.press('3');
  await expect(page.locator('[data-lift-digit]')).toHaveText('1F'); // unchanged
  await page.evaluate(() => document.getElementById('e2e-typing-probe')!.remove());
  // `t` (decorative retint) KEEPS the old WCAG 2.1.4 focus gate.
  await page.locator('#office-pause').focus();
  await page.evaluate(() => document.documentElement.style.removeProperty('--coral'));
  await page.keyboard.press('t');
  expect(
    await page.evaluate(() => document.documentElement.style.getPropertyValue('--coral'))
  ).toBe('');
});

test('statusline install chip is a link that jumps to Install (href, scroll, keyboard)', async ({
  page,
}) => {
  const errors = watchErrors(page);
  await gotoLive(page);
  const link = page.locator('#sl-install [data-sl-install-link]');
  expect(await link.evaluate((el) => el.tagName)).toBe('A');
  await expect(link).toHaveAttribute('href', '#install');
  await expect(link).toHaveAttribute('aria-label', 'Jump to the install section');
  await expect(page.locator('#sl-install .sl__copy-label')).toHaveText('install');
  // the ★ star count is unaffected by wb-2 (still the chip's sibling)
  await expect(page.locator('#sl-install .sl__stars')).toBeVisible();

  await link.click();
  await expect
    .poll(() =>
      page.evaluate(() => document.getElementById('install')!.getBoundingClientRect().top)
    )
    .toBeLessThan(50);
  expect(await page.evaluate(() => document.activeElement && document.activeElement.id)).toBe(
    'install'
  );

  // keyboard activation: a real <a> answers Enter without any extra wiring
  await page.evaluate(() => window.scrollTo({ top: 0, behavior: 'instant' }));
  await link.focus();
  await page.keyboard.press('Enter');
  await expect
    .poll(() =>
      page.evaluate(() => document.getElementById('install')!.getBoundingClientRect().top)
    )
    .toBeLessThan(50);
  expect(errors()).toEqual([]);
});

test('statusline install chip: reduced motion jumps instantly (no smooth scroll)', async ({
  browser,
}) => {
  const context = await browser.newContext({ reducedMotion: 'reduce' });
  const page = await context.newPage();
  const errors = watchErrors(page);
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  await page.locator('#sl-install [data-sl-install-link]').click();
  await expect
    .poll(() =>
      page.evaluate(() => document.getElementById('install')!.getBoundingClientRect().top)
    )
    .toBeLessThan(50);
  expect(errors()).toEqual([]);
  await context.close();
});

test('statusline install chip on mobile: label stays readable at rest, flash swaps to the glyph', async ({
  page,
  context,
}) => {
  // ≤760px KEEPS the one-word 'install' label (a bare arrow means nothing to
  // a first-time visitor — user-caught regression); only the hire-receipt
  // flash swaps to the ✓ glyph + pulse, because the receipt TEXT is too long
  // for the narrow bar. Post-wb-2 the chip no longer copies anything itself
  // (it's a jump link) — the hire fires from the Install section's OWN copy
  // control.
  await context.grantPermissions(['clipboard-write']);
  const errors = watchErrors(page);
  await page.addInitScript(() => {
    (window as unknown as { __chipPulses: number }).__chipPulses = 0;
    document.addEventListener('animationstart', (e) => {
      if ((e as AnimationEvent).animationName === 'chip-pulse') {
        (window as unknown as { __chipPulses: number }).__chipPulses++;
      }
    });
  });
  await gotoLive(page); // live office → the Install copy also hires → the receipt
  await page.setViewportSize({ width: 375, height: 800 });
  const chip = page.locator('#sl-install .sl__copy');
  const label = page.locator('#sl-install .sl__copy-label');
  const flashIcon = page.locator('#sl-install .sl__copy-icon-flash');
  await expect(chip).not.toHaveClass(/is-flash/);
  await expect(flashIcon).toBeHidden();
  // the rest state must READ on mobile: arrow + the word, not a bare glyph
  await expect(label).toBeVisible();
  await expect(label).toHaveText('install');

  await page.evaluate(() =>
    document.getElementById('install')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  await page.locator('.install__panel.is-active .install__copy').click();

  await expect(chip).toHaveClass(/is-flash/);
  await expect(flashIcon).toBeVisible();
  await expect(label).toBeHidden(); // the long receipt text never overflows the narrow bar
  // …and once the hire-receipt sequence settles, it reverts
  await expect(page.locator('#sl-install .sl__copy-label')).toHaveText('install', {
    timeout: 8_000,
  });
  await expect(chip).not.toHaveClass(/is-flash/);
  await expect(flashIcon).toBeHidden();
  // ONE start: only the hire-receipt flash fires now (there's no more
  // separate copy flash on the chip itself to queue behind).
  expect(
    await page.evaluate(() => (window as unknown as { __chipPulses: number }).__chipPulses)
  ).toBe(1);
  expect(errors()).toEqual([]);
});

test('statusline install chip: the ★ star segment renders the overridden count, never a literal null/undefined', async ({
  page,
}) => {
  // __GH_STARS__ is a build-time GitHub API fetch (astro.config.mjs calls
  // fetchStarCount()); `just site-e2e` / CI's site.yml e2e build both set
  // GH_STARS_OVERRIDE=842 (config/gh-stars.mjs) so this suite's single shared
  // webServer/dist gets a deterministic count instead of racing an
  // unauthenticated, rate-limited GitHub API call. A dev running bare
  // `npx playwright test` against a stale build made WITHOUT that override may
  // see this fail (chip absent or a different count) — rebuild with the env
  // var set first. The shape guard stays broad so a regression to the
  // stringified-null/undefined defect class (`★null`/`★undefined`) still fails
  // even if the override value above ever changes.
  await gotoLive(page);
  const stars = page.locator('#sl-install .sl__stars');
  await expect(stars).toBeVisible();
  await expect(stars).toHaveText('★ 842');
  await expect(stars).toHaveText(/^\s*★\s*\d+\s*$/);
});

test('WCAG 2.1.4: the statusline keys toggle turns the digit shortcuts off, then back on', async ({
  page,
}) => {
  await gotoLive(page);
  // digits ride by default
  await page.keyboard.press('2');
  await expect(page.locator('[data-lift-digit]')).toHaveText('2F', { timeout: 10_000 });
  // open the floor popover and flip the shortcuts OFF
  await page.locator('[data-floor-toggle]').click();
  const keysToggle = page.locator('[data-keys-toggle]');
  await keysToggle.click();
  await expect(keysToggle).toHaveAttribute('aria-checked', 'false');
  // OFF: a floor digit is inert — the lift readout does not move
  await page.keyboard.press('3');
  await expect(page.locator('[data-lift-digit]')).toHaveText('2F');
  // …and the choice is persisted (single-char shortcuts have a real off-switch)
  expect(await page.evaluate(() => localStorage.getItem('pix-keys'))).toBe('off');
  // flip it back ON — the digit rides again
  await keysToggle.click();
  await expect(keysToggle).toHaveAttribute('aria-checked', 'true');
  await page.keyboard.press('3');
  await expect(page.locator('[data-lift-digit]')).toHaveText('3F', { timeout: 10_000 });
});

test('the clock forces night after hours and clears on an explicit theme act', async ({ page }) => {
  // The only theme-init path every other test routes around. Playwright's clock
  // makes it deterministic (fixes Date; timers/rAF stay real).
  await page.clock.setFixedTime(new Date('2026-01-01T23:00:00'));
  await page.emulateMedia({ colorScheme: 'light' }); // the clock must win over a light OS
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  await page.evaluate(() => localStorage.removeItem('pix-theme'));
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'night');
  await expect(page.locator('html')).toHaveAttribute('data-clock-night', '1');
  // an explicit theme act ends the clock's authority (and its footer explainer)
  await page.locator('#theme-toggle').click();
  await expect(page.locator('html')).not.toHaveAttribute('data-clock-night', '1');
  // …and the clock NEVER forces day: noon + a light OS lands day, not night.
  await page.clock.setFixedTime(new Date('2026-01-01T12:00:00'));
  await page.evaluate(() => localStorage.removeItem('pix-theme'));
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'day');
});

// ---------------------------------------------------------------------------
// The tests below came out of the sitewide interaction audit (91 catalogued
// listeners/globals/observers → these six): every runtime contract with
// med+ user impact and low flake risk that the tests above didn't already pin.

test('first visit: boot intro auto-runs, reveals the page, seeds the gate', async ({ page }) => {
  await page.goto('./'); // NO pix-booted seed — the real first visit
  await expect(page.locator('#boot')).toBeVisible();
  // Splash log displays 4 lines (~1.7s) then holds for engine (~4s MAX_ENGINE_WAITS) + settle fade (460ms) ≈ 6.3s.
  await expect(page.locator('html')).not.toHaveAttribute('data-booting', '1', { timeout: 10_000 });
  await expect.poll(() => page.evaluate(() => sessionStorage.getItem('pix-booted'))).toBe('1');
  expect(await page.evaluate(() => document.getElementById('main')!.hasAttribute('inert'))).toBe(
    false
  );
  // finish() dispatched pix:revealed, arming the reveal-on-scroll observer —
  // opacity:0 still counts as "visible" to Playwright, so assert the CLASS.
  await expectSectionReveal(page, 'install');
  // Gate round-trip: a seeded session skips the overlay, and the IMMEDIATE
  // pix:revealed path must arm the reveal observer just the same.
  await page.reload();
  await expect(page.locator('#boot')).not.toBeVisible();
  await expectSectionReveal(page, 'install');
});

test('a keypress during the Level-2 engine hold force-settles the splash immediately', async ({
  page,
}) => {
  // Regression pin: skip() used to just call finish() — on the index page
  // (#office-live present) that re-enters the SAME waitForEngine() hold an
  // unforced finish() would, so a user gesture mid-hold did NOTHING but relight
  // already-lit log lines; the page stayed inert up to the ~4s MAX_ENGINE_WAITS
  // cap regardless of the keypress. A user gesture must always win over the
  // engine hold.
  const errors = watchErrors(page);
  // Hang the wasm fetch forever (never fulfilled/aborted): window.__pixEngineReady
  // then never resolves via the office's own first-frame path, so an unforced
  // finish() would hold the full MAX_ENGINE_WAITS cap.
  await page.route('**/wasm/**', () => {});
  await page.goto('./'); // real first visit — no pix-booted seed
  await expect(page.locator('#boot')).toBeVisible();
  // Wait for the log to finish (last line lit) — the exact moment finish()
  // runs and, since #office-live exists and the engine will never resolve,
  // enters the waitForEngine hold. Bounded generously above the ~1.7s nominal
  // log time for CI slack.
  await expect(page.locator('.boot__line').last()).toHaveClass(/\bin\b/, { timeout: 5_000 });
  await page.keyboard.press('Space');
  // Must clear almost immediately — nowhere near the ~4s MAX_ENGINE_WAITS cap.
  await expect(page.locator('html')).not.toHaveAttribute('data-booting', '1', { timeout: 700 });
  expect(await page.evaluate(() => document.getElementById('main')!.hasAttribute('inert'))).toBe(
    false
  );
  expect(errors()).toEqual([]);
});

test('first visit on an office-less page lifts the splash promptly (no engine-gate hang)', async ({
  page,
}) => {
  // The Level-2 boot gate holds the splash for window.__pixEngineReady, set ONLY by
  // OfficeBackdrop — which is index-only. Docs/404 share Base.astro's splash but
  // have NO office, so the gate MUST fall back to the flat delay there; else the page
  // stays inert the full ~4s cap. Regression pin for PR #462's docs-page hang.
  const errors = watchErrors(page);
  await page.goto('./architecture/'); // real first visit (no pix-booted), no OfficeBackdrop
  await expect(page.locator('#boot')).toBeVisible();
  await expect(page.locator('#office-live')).toHaveCount(0); // confirm: no office on this page
  // Splash clears data-booting in ~1.7s (4×390ms line dwell) + 460ms fade ≈ 2.1s; the unguarded gate hangs to ~5.9s. 3s separates.
  await expect(page.locator('html')).not.toHaveAttribute('data-booting', '1', { timeout: 3_000 });
  expect(errors()).toEqual([]);
});

test('first visit: splash displays 4-line log with per-line dwell (~390ms)', async ({ page }) => {
  const errors = watchErrors(page);
  // Test on docs page (no office, no engine wait) for pure splash-timing measurement.
  await page.goto('./config/'); // NO pix-booted seed — the real first visit
  await expect(page.locator('#boot')).toBeVisible();
  // The splash displays 4 log lines: version, booting, themes, CLI count.
  await expect(page.locator('#boot .boot__log')).toContainText('pixtuoid');
  await expect(page.locator('#boot .boot__log')).toContainText('booting office');
  await expect(page.locator('#boot .boot__log')).toContainText('loading themes');
  // Count derived from the SAME manifest Base.astro's CLI_COUNT reads, so a
  // new source can't drift this literal (the PR-520 lesson).
  await expect(page.locator('#boot .boot__log')).toContainText(
    `${sourcesData.length} CLIs connected`
  );
  // Splash clears data-booting in ~1.7s (4×390ms line dwell) + 460ms fade ≈ 2.1s — the
  // §1 budget (user decision 2026-07-06: retimed from ~450ms/line, which measured
  // ~2.5s end-to-end here — noticeably slower than production's felt pace).
  await expect(page.locator('html')).not.toHaveAttribute('data-booting', '1', {
    timeout: 3_000,
  });
  // Whole-viewport skip still seeds the session gate.
  await expect.poll(() => page.evaluate(() => sessionStorage.getItem('pix-booted'))).toBe('1');
  expect(errors()).toEqual([]);
});

test('theme chain: saved choice, URL override, toggle persist, Escape restore, system dark', async ({
  page,
}) => {
  // Only the boot gate goes in addInitScript — an init-script THEME seed would
  // re-run on every navigation and clobber the later steps' seeds; theme
  // choices are planted via localStorage + reload instead.
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  await page.evaluate(() => localStorage.setItem('pix-theme', 'dracula'));
  await page.reload(); // the saved-choice branch — never consults the clock
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dracula');
  // The theme-color meta syncs from the same init read (mobile chrome tint).
  await expect(page.locator('meta[name="theme-color"]')).toHaveAttribute('content', '#282a36');
  // A ?theme= URL override outranks the saved choice.
  await page.goto('./?theme=night');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'night');
  // Toggle round-trip (seed 'day' so the flip lands 'night' — wall-clock-proof):
  // flip + persist + the pix:theme dispatch → listener → sync() icon/aria chain.
  await page.evaluate(() => localStorage.setItem('pix-theme', 'day'));
  await page.goto('./');
  // the brand mark (nav + footer) IS the tab favicon asset, swapped by the same
  // theme sync — day shows the lit mark, the toggle flips it to the night mark.
  await expect(page.locator('.nav__mark')).toHaveAttribute('src', /favicon-32\.png$/);
  await page.locator('#theme-toggle').click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'night');
  await expect(page.locator('.nav__mark')).toHaveAttribute('src', /favicon-32-night\.png$/);
  await expect(page.locator('.footer__mark')).toHaveAttribute('src', /favicon-32-night\.png$/);
  // the swap must also run in reverse — toggle back to day and the marks return
  // to the lit favicon (the night filename only appears if syncBrand ran, so this
  // proves the day path with teeth, not just the authored default), then restore
  // night for the persistence checks below.
  await page.locator('#theme-toggle').click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'day');
  await expect(page.locator('.nav__mark')).toHaveAttribute('src', /favicon-32\.png$/);
  await expect(page.locator('.footer__mark')).toHaveAttribute('src', /favicon-32\.png$/);
  await page.locator('#theme-toggle').click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'night');
  expect(await page.evaluate(() => localStorage.getItem('pix-theme'))).toBe('night');
  await expect(page.locator('#theme-toggle .nav__toggle-icon')).toHaveText('☀️');
  await expect(page.locator('#theme-toggle')).toHaveAttribute('aria-label', 'Switch to day');
  await page.reload(); // persistence read-back + the parse-time sync() seed
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'night');
  await expect(page.locator('#theme-toggle .nav__toggle-icon')).toHaveText('☀️');
  // Escape restore: t retints inline, Escape clears it and restores the SAVED
  // theme (validated read — never the clock).
  await page.evaluate(() => localStorage.setItem('pix-theme', 'dracula'));
  await page.reload();
  await page.keyboard.press('t');
  await expect
    .poll(() => page.evaluate(() => document.documentElement.style.getPropertyValue('--coral')))
    .not.toBe('');
  await page.keyboard.press('Escape');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dracula');
  await expect
    .poll(() => page.evaluate(() => document.documentElement.style.getPropertyValue('--coral')))
    .toBe('');
  // System-dark fallback: no saved pick + a dark scheme lands 'night' — and
  // after-hours wall clocks ALSO land night, so this is TZ-proof.
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.evaluate(() => localStorage.removeItem('pix-theme'));
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'night');
});

test('install: tabs swap panels and both clipboard branches deliver', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./'); // no live-office wait — tabs/copy are wasm-independent
  await page.locator('.install__tab[data-tab="cargo"]').click();
  await expect(page.locator('.install__tab[data-tab="cargo"]')).toHaveAttribute(
    'aria-pressed',
    'true'
  );
  await expect(page.locator('#install-panel-cargo')).toBeVisible();
  await expect(page.locator('#install-panel-brew')).toBeHidden(); // really swapped out
  // The happy path SPECIFICALLY (the hire test's regex tolerates the fallback):
  // the flash label AND the clipboard payload round-trip.
  const copy = page.locator('.install__panel.is-active .install__copy');
  await copy.click();
  await expect(copy).toHaveText('Copied ✓');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    await copy.getAttribute('data-copy')
  );
  // Force the manual branch on a fresh load (brew is the default active panel):
  // no Clipboard API → the <code> contents get SELECTED for a manual ⌘C.
  await page.addInitScript(() =>
    Object.defineProperty(navigator, 'clipboard', { value: undefined })
  );
  await page.reload();
  const brewCopy = page.locator('.install__panel.is-active .install__copy');
  await brewCopy.click();
  await expect(brewCopy).toHaveText('Select & copy');
  expect(await page.evaluate(() => String(getSelection()))).toContain('brew install');
});

test('showcase studio: deep-links tune, dial and chips swap hydrated stages, the clip plays', async ({
  page,
}) => {
  const errors = watchErrors(page);
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./#showcase-spaces'); // the canonical deep link (the legacy #themes map was dropped in 0.12.0)
  await expect(page.locator('[data-stage="spaces"]')).toBeVisible();
  await expect(page.locator('button.mon[data-ch="spaces"]')).toHaveAttribute(
    'aria-pressed',
    'true'
  );
  // First tune hydrated the stage: data-src promoted to a real src.
  await expect(page.locator('[data-stage="spaces"] img.terminal__screen')).toHaveAttribute(
    'src',
    /space_/
  );
  // An in-page hashchange re-tunes.
  await page.evaluate(() => {
    location.hash = '#showcase-dashboard';
  });
  await expect(page.locator('[data-stage="dashboard"]')).toBeVisible();
  // Dial click: exactly-one-visible-stage swap + aria radio + URL tracking.
  await page.locator('button.mon[data-ch="spaces"]').click();
  await expect(page.locator('[data-stage="spaces"]')).toBeVisible();
  await expect(page.locator('[data-stage="dashboard"]')).toBeHidden();
  await expect(page.locator('button.mon[data-ch="spaces"]')).toHaveAttribute(
    'aria-pressed',
    'true'
  );
  await expect(page).toHaveURL(/#showcase-spaces$/);
  // OSD chip: variant swap inside the stage.
  const chip = page.locator('[data-stage="spaces"] .osd__chip', { hasText: 'Pantry' });
  await chip.click();
  await expect(chip).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('[data-stage="spaces"] img.terminal__screen')).toHaveAttribute(
    'src',
    /space_pantry\.png/
  );
  // Play policy: back on the default channel with #studio in view, the clip
  // plays inline (muted autoplay is gesture-free in chromium) — no controls.
  await page.locator('button.mon[data-ch="agents"]').click();
  await page.evaluate(() =>
    document.getElementById('studio')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  await expect
    .poll(() =>
      page
        .locator('[data-stage="agents"] video')
        .evaluate((v) => !(v as HTMLVideoElement).paused && !v.hasAttribute('controls'))
    )
    .toBe(true);
  // WCAG 2.2.2: the page pause governs the clip too (it has no controls of its
  // own in normal motion). Drive the same pix:paused signal #office-pause fires
  // and assert the clip stops, then resumes.
  const clipPaused = () =>
    page.locator('[data-stage="agents"] video').evaluate((v) => (v as HTMLVideoElement).paused);
  await page.evaluate(() =>
    document.dispatchEvent(new CustomEvent('pix:paused', { detail: { paused: true } }))
  );
  await expect.poll(clipPaused).toBe(true);
  await page.evaluate(() =>
    document.dispatchEvent(new CustomEvent('pix:paused', { detail: { paused: false } }))
  );
  await expect.poll(clipPaused).toBe(false);
  expect(errors()).toEqual([]);
});

test('VIBING channel: live office paints, is pause-gated, chips drive it', async ({ page }) => {
  const errors = watchErrors(page);
  await gotoLive(page);
  // VIBING is the default channel — no dial/hash tune needed to see it.
  const stage = page.locator('[data-stage="vibing"]');
  await expect(stage).toBeVisible();
  await expect(page.locator('[data-vibing-canvas]')).toBeAttached();
  // The VIBING office is a SECOND wasm Office, whose rAF loop is gated on the
  // studio actually scrolling into view (IntersectionObserver) — bring it in.
  await page.evaluate(() =>
    document.getElementById('studio')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  const vibingShot = () =>
    page.evaluate(() =>
      (document.querySelector('[data-vibing-canvas]') as HTMLCanvasElement).toDataURL()
    );
  const vibingPainted = () =>
    page.evaluate(() => {
      const c = document.querySelector('[data-vibing-canvas]') as HTMLCanvasElement;
      const d = c.getContext('2d')!.getImageData(0, 0, c.width, c.height).data;
      return d.some((v) => v !== 0);
    });
  // Paints: the second Office actually rendered a frame (wasm cold-boot budget).
  await expect.poll(vibingPainted, { timeout: 15_000 }).toBe(true);

  // Weather chip: click storm — the office keeps live-painting through it.
  const beforeWeather = await vibingShot();
  const stormChip = page.locator('[data-stage="vibing"] .osd__chip[data-weather="storm"]');
  await stormChip.click();
  // Deterministic teeth: the click handler ran + moved the active state (a
  // frame-changed poll alone passes on ambient sprite motion regardless).
  await expect(stormChip).toHaveClass(/is-active/);
  await expect(stormChip).toHaveAttribute('aria-pressed', 'true');
  await expect.poll(vibingShot, { timeout: 5_000 }).not.toBe(beforeWeather);

  // Theme chip: cyberpunk activates + retints the page, and does NOT touch
  // the weather group's own active chip (the per-group-retint guard).
  const coralBefore = await page.evaluate(() =>
    getComputedStyle(document.documentElement).getPropertyValue('--coral')
  );
  const themeChip = page.locator('[data-stage="vibing"] .osd__chip[data-theme="cyberpunk"]');
  await themeChip.click();
  await expect(themeChip).toHaveClass(/is-active/);
  await expect(themeChip).toHaveAttribute('aria-pressed', 'true');
  await expect
    .poll(() =>
      page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--coral'))
    )
    .not.toBe(coralBefore);
  await expect(stormChip).toHaveClass(/is-active/); // weather group untouched by the theme retint

  // Slider: scrubbing the time updates the readout + aria-valuetext, flips the
  // sun/moon `data-phase` via the ENGINE's `Office.is_day` boundary (the [5,20)
  // sun window), and repaints the office. Exercises BOTH the day and the night
  // branch — the drift-fix payload the sky-scrubber added.
  const timeInput = stage.locator('[data-vibing-time]');
  const timeWrap = stage.locator('.vibing__time');
  const setHour = (h: number) =>
    timeInput.evaluate((el, v) => {
      (el as HTMLInputElement).value = String(v);
      el.dispatchEvent(new Event('input', { bubbles: true }));
    }, h);
  const beforeSlider = await vibingShot();
  await setHour(6); // 06:00 — inside the engine's [5,20) sun window → day
  await expect(stage.locator('[data-vibing-time-label]')).toHaveText('06:00');
  await expect(timeInput).toHaveAttribute('aria-valuetext', '06:00'); // SR hears "06:00", not "6"
  await expect(timeWrap).toHaveAttribute('data-phase', 'day');
  await expect.poll(vibingShot, { timeout: 5_000 }).not.toBe(beforeSlider);
  await setHour(22); // 22:00 — past sunset (≥ 20) → the moon branch
  await expect(stage.locator('[data-vibing-time-label]')).toHaveText('22:00');
  await expect(timeInput).toHaveAttribute('aria-valuetext', '22:00');
  await expect(timeWrap).toHaveAttribute('data-phase', 'night');

  // Pause gate (WCAG 2.2.2, page-scoped): #office-pause freezes this SECOND
  // office too — a frozen canvas, byte-identical snapshots — and unpausing
  // repaints it.
  const pauseBtn = page.locator('#office-pause');
  await pauseBtn.click();
  await expect(pauseBtn).toHaveAttribute('aria-pressed', 'true');
  const frozen = await vibingShot();
  await page.waitForTimeout(400); // >12 would-be frames at the 33ms cap (CI-throttle margin, matches the hero-pause test)
  expect(await vibingShot()).toBe(frozen); // not one new frame painted
  await pauseBtn.click();
  await expect(pauseBtn).toHaveAttribute('aria-pressed', 'false');
  await expect.poll(vibingShot, { timeout: 5_000 }).not.toBe(frozen); // animating again
  expect(errors()).toEqual([]);
});

test('nav menus + docs: dropdown, TOC scrollspy, 404, mobile burger', async ({ page, browser }) => {
  const errors = watchErrors(page);
  await page.goto('./config#themes'); // arrival-by-hash: the rail lights unscrolled
  await expect(page.locator('[data-toc-link="themes"]')).toHaveAttribute(
    'aria-current',
    'location'
  );
  // The Docs dropdown is the ONLY route to the five doc pages.
  const btn = page.locator('#docs-btn');
  await btn.click();
  await expect(page.locator('#docs-menu')).toHaveClass(/is-open/);
  await expect(btn).toHaveAttribute('aria-expanded', 'true');
  await page.locator('#docs-menu a').first().focus(); // focus INSIDE, or the return branch is skipped
  await page.keyboard.press('Escape');
  await expect(page.locator('#docs-menu')).not.toHaveClass(/is-open/);
  await expect(btn).toBeFocused();
  // TOC click sync + the anchored heading clears the 60px sticky nav.
  await page.locator('[data-toc-link="custom-sprite-packs"]').click();
  await expect(page.locator('[data-toc-link="custom-sprite-packs"]')).toHaveAttribute(
    'aria-current',
    'location'
  );
  await expect
    .poll(() =>
      page.evaluate(
        () => document.getElementById('custom-sprite-packs')!.getBoundingClientRect().top
      )
    )
    .toBeGreaterThan(60);
  // Scrollspy proper: park a heading at 20% viewport — inside the -15%/-75%
  // reading band — and the rail follows.
  await page.evaluate(() => {
    const h = document.getElementById('themes')!;
    window.scrollTo({
      top: h.getBoundingClientRect().top + window.scrollY - window.innerHeight * 0.2,
      behavior: 'instant',
    });
  });
  await expect(page.locator('[data-toc-link="themes"]')).toHaveAttribute(
    'aria-current',
    'location'
  );
  // Unknown routes land on the office at 3 a.m. with a way home. The document
  // request itself logs a resource-404 console error — filter that one line;
  // everything else must stay clean.
  await page.goto('./no-such-desk');
  await expect(page.locator('.lost h1')).toContainText('Session not');
  await expect
    .poll(() =>
      page
        .locator('.lost__scene .terminal__screen')
        .evaluate((img) => (img as HTMLImageElement).naturalWidth)
    )
    .toBeGreaterThan(0);
  await expect(page.locator('.lost__cta .btn-primary')).toHaveAttribute('href', '/');
  expect(errors().filter((e) => !e.includes('Failed to load resource'))).toEqual([]);
  // Mobile burger: below 760px the link panel is display:none until .is-open —
  // a dead burger means no navigation at all on phones. Same Esc-focus-return
  // contract as the Docs dropdown (WCAG 2.4.3).
  const ctx = await browser.newContext({ viewport: { width: 480, height: 800 } });
  const m = await ctx.newPage();
  await m.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await m.goto('./config');
  await m.locator('#nav-burger').click();
  await expect(m.locator('#nav-links')).toHaveClass(/is-open/);
  await expect(m.locator('#nav-burger')).toHaveAttribute('aria-expanded', 'true');
  await m.locator('#nav-links a').first().focus();
  await m.keyboard.press('Escape');
  await expect(m.locator('#nav-links')).not.toHaveClass(/is-open/);
  await expect(m.locator('#nav-burger')).toBeFocused();
  await ctx.close();
});

test('landing fixed chrome: floating nav, statusline readouts, floor popover', async ({ page }) => {
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./'); // no live-office wait — everything here is wasm-independent
  // The load-bearing half of the floating variant: no live blur filter over a
  // 30fps canvas (the compositor-flicker class).
  await expect(page.locator('.nav')).toHaveClass(/nav--floating/);
  expect(
    await page.evaluate(() => getComputedStyle(document.querySelector('.nav')!).backdropFilter)
  ).toBe('none');
  // The statusline consumes the globals (the 250ms poll shows the 0.55
  // fallback pre-wasm, so no live wait is needed); clock is format-only — TZ-agnostic.
  await expect(page.locator('[data-sl-lights]')).toHaveText(/lights \d+%/);
  await expect(page.locator('[data-sl-clock]')).toHaveText(/^\d{2}:\d{2} (day|night)$/);
  // Floor popover: toggle → Esc closes → reopen → a floor jump closes AND
  // rides the lift (the same scrollspy round-trip as the digit-keys test).
  const toggle = page.locator('[data-floor-toggle]');
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');
  await expect(page.locator('#sl-floors')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.locator('#sl-floors')).toBeHidden();
  await toggle.click();
  await page.locator('[data-floor-btn="1F"]').click();
  await expect(page.locator('#sl-floors')).toBeHidden();
  await expect(page.locator('[data-lift-digit]')).toHaveText('1F', { timeout: 10_000 });
});

test('no horizontal overflow at phone widths (mobile pan guard)', async ({ browser }) => {
  // `body { overflow-x: hidden }` masks the desktop scrollbar, so a full-width
  // block whose ::before glow (or any child) pokes past the viewport is
  // INVISIBLE on desktop yet PANS the visual viewport on mobile — the
  // [data-lit]::before -8% overflow class (fixed by overflow-x: clip). A
  // pseudo-element dodges every querySelectorAll('*') element scan, so only a
  // documentElement scrollWidth<=clientWidth guard catches it. This whole class
  // slipped the #453 whole-site audit (desktop-eyeballed, no such assertion);
  // pin index + a docs page at real phone widths so it can't silently regress.
  // #503: the missed coverage was the ROUTE MATRIX — /parallel-delivery (the
  // one page with a wide ASCII pre + long unbreakable links) was never in the
  // list, and the old scrollW<=clientW assertion catches its overflow fine
  // once it is (measured pre-fix: scrollW 526 vs clientW 320 → red 206px).
  // The innerWidth assertion below is a second, DIFFERENT tripwire: under
  // mobile emulation window.innerWidth expands with over-wide content (526 at
  // a 375 device — review-measured) while documentElement.clientWidth stays
  // pinned to the device width, so it trips even if a future layout mode
  // absorbs the overflow out of scrollWidth's view.
  for (const [path, width] of [
    ['./', 320], // iPhone SE — the narrowest supported
    ['./', 360], // Android
    ['./', 390], // iPhone 12–14
    ['./', 430], // iPhone Pro Max
    ['./', 768], // tablet
    ['./config', 390], // docs shell: code blocks / mermaid can overflow too
    ['./config', 768],
    ['./architecture', 375], // build-time mermaid SVG
    ['./contributing', 375],
    ['./knowledge-base', 375],
    ['./parallel-delivery', 320], // the #503 repro: wide ASCII pre + long links
    ['./parallel-delivery', 375],
    ['./parallel-delivery', 768],
  ] as const) {
    const context = await browser.newContext({
      viewport: { width, height: 820 },
      isMobile: true,
      hasTouch: true,
    });
    const page = await context.newPage();
    await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
    await page.goto(path);
    // The reported symptom is a left-right drag at the BOTTOM — measure there,
    // after any late layout settles.
    await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
    const { scrollW, clientW, innerW } = await page.evaluate(() => ({
      scrollW: document.documentElement.scrollWidth,
      clientW: document.documentElement.clientWidth,
      innerW: window.innerWidth,
    }));
    expect(
      scrollW,
      `${path} at ${width}px is ${scrollW - clientW}px wider than the viewport (horizontal pan)`
    ).toBeLessThanOrEqual(clientW);
    expect(
      innerW,
      `${path} at ${width}px: window.innerWidth expanded to ${innerW}px (${innerW - width}px past the device width — over-wide content grew the emulated viewport)`
    ).toBeLessThanOrEqual(width);
    await context.close();
  }
});

test('the hero copy clears the floating nav at phone viewports (vertical overlap guard)', async ({
  browser,
}) => {
  // The hero is min-height:100svh with the copy BOTTOM-anchored (flex-end), so
  // when the copy outgrows the viewport (narrow phones; iOS text metrics), its
  // TOP is what gives way — with no top reservation the eyebrow slid to page
  // y=0, straight under the index's floating (absolute) nav logo (measured
  // pre-fix: eyebrow.top 0 vs nav bottom 60 at 402×700; a 1206×2622 iPhone
  // field report). The fix reserves --nav-h + a breath as .hero padding-top.
  // The pan guard above is width-only — this is the VERTICAL twin. 402×700 ≈
  // iOS Safari's visible viewport on a 874pt phone; 360×640 = small Android.
  // reducedMotion pins the copy's `rise` entry animation (translateY 22px →
  // none) to its SETTLED position — the steady-state clearance is only ~12px,
  // so a mid-animation read could mask a partial regression (false green).
  for (const [width, height] of [
    [402, 700],
    [360, 640],
    [402, 874],
  ] as const) {
    const context = await browser.newContext({
      viewport: { width, height },
      isMobile: true,
      hasTouch: true,
      reducedMotion: 'reduce',
    });
    const page = await context.newPage();
    await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
    await page.goto('./');
    const { navBottom, eyebrowTop } = await page.evaluate(() => ({
      navBottom: document.querySelector('.nav .nav__inner')!.getBoundingClientRect().bottom,
      eyebrowTop: document.querySelector('.hero__copy .eyebrow')!.getBoundingClientRect().top,
    }));
    expect(
      eyebrowTop,
      `at ${width}x${height} the hero eyebrow (top ${eyebrowTop}px) sits under the floating nav (bottom ${navBottom}px)`
    ).toBeGreaterThanOrEqual(navBottom);
    await context.close();
  }
});

test('docs-table code cells render single-line (column-collapse guard)', async ({ browser }) => {
  // `.prose :not(pre) > code`'s overflow-wrap:anywhere feeds its soft-wrap
  // opportunities into MIN-CONTENT intrinsic sizing (unlike break-word), so
  // table auto-layout crushed the /config Key column to ~1ch and wrapped
  // `theme` letter-by-letter. The pan guard above is blind to it — a column
  // collapse never widens the page — so pin the `.prose table th/td code`
  // exemption directly: every table code token renders as ONE line box.
  const context = await browser.newContext({
    viewport: { width: 390, height: 820 },
    isMobile: true,
    hasTouch: true,
  });
  const page = await context.newPage();
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./config');
  const cells = await page.evaluate(() => {
    const code = [...document.querySelectorAll('.prose table th code, .prose table td code')];
    return {
      total: code.length,
      wrapped: code.filter((c) => c.getClientRects().length > 1).map((c) => c.textContent),
    };
  });
  expect(
    cells.total,
    'the /config tables rendered no code cells — selector drifted?'
  ).toBeGreaterThan(0);
  expect(cells.wrapped, 'code tokens inside table cells wrapped mid-token').toEqual([]);
  await context.close();
});

test('text over the live office carries its own scrim (.text-scrim)', async ({ page }) => {
  await gotoLive(page);
  // wb-2 C9: the hero copy (eyebrow/subcopy/CTA/platform-line) is now BARE,
  // tools-table style — legibility comes from --office-ink/--office-ink-accent
  // tokens tuned against the real office composite, not a plate (see the WCAG
  // test below + global.css's doc comment). The install note still carries an
  // actual scrim/plate; the standalone Features ledger was retired — its rows
  // now live inside the merged 5F studio band as the roster, which carries
  // the same local-scrim legibility guarantee the old ledger rows had.
  const heroBg = await page.evaluate(
    () => getComputedStyle(document.querySelector('.hero .statement-sub')!).backgroundColor
  );
  expect(heroBg).toBe('rgba(0, 0, 0, 0)');
  const ghostBg = await page.evaluate(
    () => getComputedStyle(document.querySelector('.hero__ghost')!).backgroundColor
  );
  expect(ghostBg).toBe('rgba(0, 0, 0, 0)');

  // The install note ("Also on crates.io...") floated unplated over the
  // skyline — give it the same crisp plate (it's NOT hero, so the install
  // card idiom applies, unlike the hero's bare treatment above).
  expect(await page.locator('.install__note.text-scrim').count()).toBe(1);
  // The roster rows are BARE over the section's DIM_MAX backing (user verdict,
  // same rule as the hero + tools table) — the plate must never come back.
  expect(await page.locator('#showcase .roster__row.text-scrim').count()).toBe(0);
  // …and INSIDE the card the plate's visible edge aligns with the tabs/command
  // column: text-scrim's negative office-margin is zeroed there (user-caught —
  // the plate hung ~11px left of its siblings).
  const [noteX, tabsX] = await page.evaluate(() => [
    document.querySelector('.install__note')!.getBoundingClientRect().x,
    document.querySelector('.install__tabs')!.getBoundingClientRect().x,
  ]);
  expect(Math.abs(noteX - tabsX)).toBeLessThan(1);
});

test('bare hero text clears WCAG AA at the real office composite (day + night)', async ({
  page,
}) => {
  // wb-2 C6/C9: the hero eyebrow/subcopy/platform-line read directly over the
  // live office (no plate — the SupportedTools-table look). The 4F Features
  // intro paragraph this test also covered was retired along with the
  // standalone floor (its rows are now scrimmed roster entries in the merged
  // 5F band — see the .text-scrim test above — so the bare-ink-token
  // legibility path no longer applies to them). Legibility now depends
  // entirely on the ink token clearing contrast against whatever the office
  // ACTUALLY renders behind it, so this samples the REAL canvas pixels (not a --screen proxy
  // — with no opaque plate the underlying office pixel is no longer
  // "immaterial") across the sampled element's bounding box, finds the
  // brightest AND darkest pixel in it (day's dimmer LIGHTENS the composite
  // toward --paper, night's DARKENS it toward --bg — opposite directions, so
  // the true worst case can be either extreme depending on theme), composites
  // each with the live dimmer, and checks both against the element's real
  // computed ink color.
  async function worstCaseRatio(selector: string): Promise<{ ratio: number; theme: string }> {
    const theme = await page.evaluate(() => document.documentElement.dataset.theme || 'day');
    const measured = await page.evaluate((sel) => {
      const canvas = document.getElementById('office-live') as HTMLCanvasElement;
      const el = document.querySelector(sel)!;
      const r = el.getBoundingClientRect();
      const cr = canvas.getBoundingClientRect();
      const sx = canvas.width / cr.width;
      const sy = canvas.height / cr.height;
      const ctx = canvas.getContext('2d', { willReadFrequently: true })!;
      const x0 = Math.max(0, Math.floor((r.left - cr.left) * sx));
      const y0 = Math.max(0, Math.floor((r.top - cr.top) * sy));
      const w = Math.max(1, Math.ceil(r.width * sx));
      const h = Math.max(1, Math.ceil(r.height * sy));
      const data = ctx.getImageData(
        x0,
        y0,
        Math.min(w, canvas.width - x0),
        Math.min(h, canvas.height - y0)
      ).data;
      let maxLum = -1,
        maxPx = [0, 0, 0];
      let minLum = 2,
        minPx = [0, 0, 0];
      const relLum = ([rr, gg, bb]: number[]) => {
        const lin = (c: number) => {
          const s = c / 255;
          return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
        };
        return 0.2126 * lin(rr) + 0.7152 * lin(gg) + 0.0722 * lin(bb);
      };
      for (let i = 0; i < data.length; i += 4) {
        const px = [data[i], data[i + 1], data[i + 2]];
        const lum = relLum(px);
        if (lum > maxLum) {
          maxLum = lum;
          maxPx = px;
        }
        if (lum < minLum) {
          minLum = lum;
          minPx = px;
        }
      }
      return {
        maxPx,
        minPx,
        dimmerBg: getComputedStyle(document.getElementById('dimmer')!).backgroundColor,
        dimmerOpacity: parseFloat(
          (document.getElementById('dimmer') as HTMLElement).style.opacity || '0'
        ),
        textColor: getComputedStyle(el).color,
      };
    }, selector);

    const dim = parseRgb(measured.dimmerBg).slice(0, 3) as [number, number, number];
    const textRgb = parseRgb(measured.textColor).slice(0, 3) as [number, number, number];
    const afterMax = compositeOver(
      [...dim, measured.dimmerOpacity] as [number, number, number, number],
      measured.maxPx as [number, number, number]
    );
    const afterMin = compositeOver(
      [...dim, measured.dimmerOpacity] as [number, number, number, number],
      measured.minPx as [number, number, number]
    );
    const ratio = Math.min(contrastRatio(textRgb, afterMax), contrastRatio(textRgb, afterMin));
    return { ratio, theme };
  }

  for (const theme of ['day', 'night'] as const) {
    await page.addInitScript((t) => {
      sessionStorage.setItem('pix-booted', '1');
      localStorage.setItem('pix-theme', t);
    }, theme);
    await page.goto('./');
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);

    // Every BARE muted-text surface over the office, not just the hero's
    // (lens B measured #showcase's lead + roster body at 3.79-3.96:1 day —
    // the sibling sweep adds the other sections' leads and eyebrows too):
    for (const selector of [
      '.hero .eyebrow',
      '.hero .statement-sub',
      '.hero__avail',
      '#showcase .section-head .lead',
      '#showcase .eyebrow',
      '.roster__body',
      '#how .eyebrow',
      '#tools .section-head .lead',
      '#install .section-head .lead',
      '#amenities .eyebrow',
      '.pantry__cite',
    ]) {
      const { ratio } = await worstCaseRatio(selector);
      expect(
        ratio,
        `${theme} ${selector}: WCAG AA floor is 4.5:1; measured ${ratio.toFixed(2)}:1`
      ).toBeGreaterThanOrEqual(4.5);
    }
  }
});

test('hero badge row: one chip per registered source, matching the tools-table row count', async ({
  page,
}) => {
  // One manifest (sources.json), two consumers (Hero's chip row, the tools
  // table on #tools) — a bridge, not two independently-maintained counts, so
  // adding/removing a source can't silently desync them.
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  const chips = page.locator('.hero__badges .hero__badge');
  await expect(chips).toHaveCount(supportedSources.length);
  await expect(chips.first()).toHaveText(
    `${supportedSources[0].badge.replace('·', '')} ${supportedSources[0].name}`
  );

  const tableRows = page.locator('.tools tbody:not(.tools__planned) tr');
  await expect(tableRows).toHaveCount(supportedSources.length);
});

test('hero badge hues clear WCAG AA against their theme-aware chip surface (day + night)', async ({
  page,
}) => {
  // The badge-code hue is a cross-boundary copy of the app's per-source color
  // (pinned to theme::NORMAL.source by the Rust bridge test), painted on a
  // chip surface that FLIPS per theme — day's light --surface chip darkens
  // the hue toward black, night's dark --screen chip lightens it toward
  // white (global.css's .hero__badge-code doc comment) — so this sweeps
  // every rendered hue in both themes rather than trusting one spot check.
  for (const theme of ['day', 'night'] as const) {
    await page.addInitScript((t) => {
      sessionStorage.setItem('pix-booted', '1');
      localStorage.setItem('pix-theme', t);
    }, theme);
    await page.goto('./');
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);

    const chips = await page.evaluate(() =>
      [...document.querySelectorAll('.hero__badge')].map((li) => ({
        codeColor: getComputedStyle(li.querySelector('.hero__badge-code')!).color,
        chipBg: getComputedStyle(li).backgroundColor,
        label: li.textContent?.trim(),
      }))
    );
    expect(chips.length, `${theme}: no .hero__badge chips rendered`).toBe(supportedSources.length);
    for (const { codeColor, chipBg, label } of chips) {
      const fg = parseRgb(codeColor).slice(0, 3) as [number, number, number];
      const bg = parseRgb(chipBg).slice(0, 3) as [number, number, number];
      const ratio = contrastRatio(fg, bg);
      expect(
        ratio,
        `${theme} "${label}": WCAG AA floor is 4.5:1; code ${codeColor} on chip ${chipBg} measured ${ratio.toFixed(2)}:1`
      ).toBeGreaterThanOrEqual(4.5);
    }
  }
});

test('tenant board text (badges, legend, planned rows, soon marks, star plaque) clears WCAG AA against the dark board ground (day + night)', async ({
  page,
}) => {
  // The tenant board (#tools) is a .hw-panel — its --screen ground is a
  // THEME-INDEPENDENT literal (global.css never redefines --screen per
  // theme), unlike the hero's day/night chip split. This sweep runs both
  // page themes anyway as a defense-in-depth pin, not because the ratio is
  // expected to move.
  for (const theme of ['day', 'night'] as const) {
    await page.addInitScript((t) => {
      sessionStorage.setItem('pix-booted', '1');
      localStorage.setItem('pix-theme', t);
    }, theme);
    await page.goto('./');
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await page.evaluate(() =>
      document.getElementById('tools')!.scrollIntoView({ block: 'center', behavior: 'instant' })
    );

    const boardBg = await page.evaluate(
      () => getComputedStyle(document.querySelector('.tools__board')!).backgroundColor
    );
    const bg = parseRgb(boardBg).slice(0, 3) as [number, number, number];
    const assertAA = (codeColor: string, label: string) => {
      const fg = parseRgb(codeColor).slice(0, 3) as [number, number, number];
      const ratio = contrastRatio(fg, bg);
      expect(
        ratio,
        `${theme} "${label}": WCAG AA floor is 4.5:1; color ${codeColor} on board ${boardBg} measured ${ratio.toFixed(2)}:1`
      ).toBeGreaterThanOrEqual(4.5);
    };

    const badges = await page.evaluate(() =>
      [...document.querySelectorAll('.tools__board .tools__badge')].map((b) => ({
        codeColor: getComputedStyle(b).color,
        label: b.textContent?.trim(),
      }))
    );
    expect(badges.length, `${theme}: no .tools__badge chips rendered`).toBe(
      supportedSources.length
    );
    for (const { codeColor, label } of badges) assertAA(codeColor, `badge ${label}`);

    // the legend is the sole decode key for the LED marks — always renders,
    // since the current manifest always has both 'yes' and 'experimental'
    // states on screen.
    const legendColor = await page.evaluate(
      () => getComputedStyle(document.querySelector('.tools__legend')!).color
    );
    assertAA(legendColor, 'legend');

    // the star plaque is its OWN .hw-panel beside the board (same --screen
    // ground); assert its own bg rather than reuse boardBg, so a future
    // divergence between the two panels' grounds can't go unnoticed.
    const plaqueBg = await page.evaluate(
      () => getComputedStyle(document.querySelector('.tools__plaque')!).backgroundColor
    );
    const assertPlaqueAA = (codeColor: string, label: string) => {
      const fg = parseRgb(codeColor).slice(0, 3) as [number, number, number];
      const ratio = contrastRatio(fg, parseRgb(plaqueBg).slice(0, 3) as [number, number, number]);
      expect(
        ratio,
        `${theme} "${label}": WCAG AA floor is 4.5:1; color ${codeColor} on plaque ${plaqueBg} measured ${ratio.toFixed(2)}:1`
      ).toBeGreaterThanOrEqual(4.5);
    };
    const plaqueColors = await page.evaluate(() => ({
      stars: getComputedStyle(document.querySelector('.tools__plaque-stars')!).color,
      engraving: getComputedStyle(document.querySelector('.tools__plaque-engraving')!).color,
      link: getComputedStyle(document.querySelector('.tools__plaque-link')!).color,
    }));
    assertPlaqueAA(plaqueColors.stars, 'plaque stars');
    assertPlaqueAA(plaqueColors.engraving, 'plaque engraving');
    assertPlaqueAA(plaqueColors.link, 'plaque link');

    // sources.json currently has zero "planned" rows, so the planned tbody
    // and its "soon" mark never render on the live page. Probe the SAME
    // markup SupportedTools.astro's template emits for a planned row
    // (MARK('planned')), injected into the real table so it picks up the
    // live cascade — pins the rule even with an empty planned set.
    // PAIRED-COPY PIN: MARK() is inline in SupportedTools.astro (unexported —
    // Playwright can't call Astro frontmatter), so this literal MUST track
    // MARK's 'planned'/'soon' markup by hand; edit both or the probe asserts
    // stale markup. (The matching note sits on MARK() itself.)
    const planned = await page.evaluate(() => {
      const table = document.querySelector('.tools__board table')!;
      const tbody = document.createElement('tbody');
      tbody.className = 'tools__planned';
      tbody.innerHTML =
        '<tr><th scope="row">Probe Tool</th><td class="tools__cell" data-state="planned">' +
        '<span class="tools__mark tools__soon" aria-hidden="true">soon</span></td></tr>';
      table.appendChild(tbody);
      const result = {
        rowColor: getComputedStyle(tbody.querySelector('th')!).color,
        soonColor: getComputedStyle(tbody.querySelector('.tools__soon')!).color,
      };
      tbody.remove();
      return result;
    });
    assertAA(planned.rowColor, 'planned row');
    assertAA(planned.soonColor, 'soon mark');
  }
});

test('pantry chitchat bubble text clears WCAG AA against its own dark ground (day + night)', async ({
  page,
}) => {
  // Same pattern as the tenant-board sweep above: .pantry__bubble paints its
  // OWN opaque --screen background (not a shared board), so read fg/bg off
  // the bubble element directly rather than a separate panel ancestor. Both
  // tokens are THEME-INDEPENDENT (global.css never redefines --screen/
  // --chip-bright per theme, same reasoning as the board/plaque), so this
  // sweep is defense-in-depth, not an expected-to-move ratio.
  for (const theme of ['day', 'night'] as const) {
    await page.addInitScript((t) => {
      sessionStorage.setItem('pix-booted', '1');
      localStorage.setItem('pix-theme', t);
    }, theme);
    await page.goto('./');
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await page.evaluate(() =>
      document.querySelector('.pantry')!.scrollIntoView({ block: 'center', behavior: 'instant' })
    );
    const bubbles = await page.evaluate(() =>
      [...document.querySelectorAll('.pantry__bubble')].map((b) => {
        const cs = getComputedStyle(b);
        return { color: cs.color, bg: cs.backgroundColor, label: b.textContent?.trim() };
      })
    );
    expect(bubbles.length, `${theme}: no .pantry__bubble rendered`).toBeGreaterThan(0);
    for (const { color, bg, label } of bubbles) {
      const fg = parseRgb(color).slice(0, 3) as [number, number, number];
      const bgRgb = parseRgb(bg).slice(0, 3) as [number, number, number];
      const ratio = contrastRatio(fg, bgRgb);
      expect(
        ratio,
        `${theme} "${label}": WCAG AA floor is 4.5:1; ${color} on ${bg} measured ${ratio.toFixed(2)}:1`
      ).toBeGreaterThanOrEqual(4.5);
    }
  }
});

test('the statusline feed ellipsizes on the wrapping text span, not the flex row', async ({
  page,
}) => {
  // A flex container's own overflow/text-overflow never applies to its
  // children (the badge `<b>` and the raw " · {what}" run are separate
  // anonymous flex items) — regression pin for the mid-glyph clip: ellipsis
  // must live on `.sl__text`, and it must actually be clipping SOMETHING
  // (i.e. the item's content is wider than its box) for this pin to mean
  // anything at a viewport wide enough to show the feed at all.
  await page.setViewportSize({ width: 1280, height: 720 });
  await gotoLive(page);
  // The feed's real content is a build-time GH API fetch (real PR titles) —
  // length varies build to build, so don't rely on live content happening to
  // overflow. Force it deterministically instead: the CSS behavior under test
  // is on `.sl__text` itself, independent of what text it holds.
  const info = await page.evaluate(() => {
    const text = document.querySelector('.sl__item .sl__text') as HTMLElement;
    text.textContent =
      'cc·pixtuoid · merged #999 · this is a deliberately very long line of feed text to force an overflow';
    const cs = getComputedStyle(text);
    return {
      overflow: cs.overflow,
      textOverflow: cs.textOverflow,
      whiteSpace: cs.whiteSpace,
      scrollWidth: text.scrollWidth,
      clientWidth: text.clientWidth,
    };
  });
  expect(info.overflow).toBe('hidden');
  expect(info.textOverflow).toBe('ellipsis');
  expect(info.whiteSpace).toBe('nowrap');
  expect(info.scrollWidth).toBeGreaterThan(info.clientWidth);
});

test('the feed hides itself, rather than show an unreadably short fragment, at a squeezed width', async ({
  page,
}) => {
  // 768-860px: .sl__text's available width drops to a sliver ("cc·pixtuoid ·
  // mer…") even with a clean ellipsis — hiding reads better than a fragment
  // too short to convey anything. Above/below that band it should show.
  await page.setViewportSize({ width: 800, height: 720 });
  await gotoLive(page);
  await expect(page.locator('.sl__feed')).toBeHidden();
  await page.setViewportSize({ width: 1024, height: 720 });
  await expect(page.locator('.sl__feed')).toBeVisible();
});

test('the feed pauses while the tab is hidden — no ghosted double-exposure on refocus', async ({
  page,
}) => {
  // A hidden tab freezes CSS transitions but NOT setInterval/setTimeout. If the
  // crossfading feed kept rotating while hidden, its queued out→in `is-on` class
  // swaps would replay their opacity fades AT ONCE on refocus — two lines
  // double-exposed (the ghosting bug). The rotation must stop while hidden and
  // resync on return; at every point exactly one item is lit.
  await page.setViewportSize({ width: 1280, height: 720 });
  await gotoLive(page);
  await expect(page.locator('.sl__item.is-on')).toHaveCount(1);

  const litIndex = () =>
    page.evaluate(() =>
      Array.from(document.querySelectorAll('.sl__item')).findIndex((el) =>
        el.classList.contains('is-on')
      )
    );

  // Force the tab "hidden" (the handler reads document.hidden) AND read the lit
  // line in the SAME synchronous evaluate: dispatching visibilitychange runs
  // stopFeed → showOnlyFeed, so exactly one item is lit and no rotation timer is
  // pending. Reading here (not before the stop) is deterministic — it can't land
  // in the fade gap (findIndex → -1) or race a free-running 6s tick.
  const before = await page.evaluate(() => {
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
    document.dispatchEvent(new Event('visibilitychange'));
    return Array.from(document.querySelectorAll('.sl__item')).findIndex((el) =>
      el.classList.contains('is-on')
    );
  });
  expect(before).toBeGreaterThanOrEqual(0);

  // A pix:paused{false} can fire WHILE hidden (OfficeBackdrop dispatches it on a
  // reduced-motion toggle) — startFeed's document.hidden guard must refuse to
  // re-arm the rotation behind a hidden tab. Then wait past one 6s rotation:
  // with the fix nothing advances; with the bug the interval keeps ticking.
  await page.evaluate(() =>
    document.dispatchEvent(new CustomEvent('pix:paused', { detail: { paused: false } }))
  );
  await page.waitForTimeout(6500);
  await expect(page.locator('.sl__item.is-on')).toHaveCount(1);
  expect(await litIndex()).toBe(before); // rotation truly paused while hidden

  // Refocus: still exactly one lit, no double-exposure.
  await page.evaluate(() => {
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => false });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await expect(page.locator('.sl__item.is-on')).toHaveCount(1);

  // stopFeed collapses any accidental multi-lit state to exactly one line: force
  // an illegal 2+-lit state, then a stop (user pause) must snap it back to one.
  await page.evaluate(() => {
    document.querySelectorAll('.sl__item').forEach((el) => el.classList.add('is-on'));
    document.dispatchEvent(new CustomEvent('pix:paused', { detail: { paused: true } }));
  });
  await expect(page.locator('.sl__item.is-on')).toHaveCount(1);
});

test('footer separators never strand alone at a wrap boundary', async ({ page }) => {
  // Each "·" is grouped with the item it introduces into ONE flex item
  // (.footer__grp), so flex-wrap can only break BETWEEN groups. Pin the
  // structure directly rather than pixel-measuring a wrap (viewport-fragile):
  // every .footer__sep's parent must be a .footer__grp.
  await gotoLive(page);
  const seps = await page.locator('.footer .footer__sep').all();
  expect(seps.length).toBeGreaterThan(0);
  for (const sep of seps) {
    await expect(sep.locator('xpath=..')).toHaveClass(/\bfooter__grp\b/);
  }
});

test('no footer line begins or ends with a separator dot once the row wraps', async ({ page }) => {
  // R3 (wb-3 matrix sweep): the sibling test above only proves a "·" can't be
  // stranded ALONE mid-row — but each dot introduces its FOLLOWING item
  // (.footer__grp), so a group that itself wraps to a new line still leads
  // that line with its own dot (#768-day-08: "· built in Rust" opening
  // line 2). Below the width where the row visibly wraps, Footer.astro now
  // hides the dots entirely and lets the row's flex gap carry the
  // separation. Check actual RENDERED rows (grouped by top position), not
  // raw textContent — a display:none dot still shows up in textContent even
  // though nothing paints, which would false-positive this check.
  await gotoLive(page);
  await page.setViewportSize({ width: 768, height: 900 });
  await page.evaluate(() =>
    window.scrollTo({ top: document.documentElement.scrollHeight, behavior: 'instant' })
  );
  const bad = await page.evaluate(() => {
    const line = document.querySelector('.footer__line')!;
    const items = Array.from(line.children).filter(
      (el) => (el as HTMLElement).offsetParent !== null
    );
    const rows = new Map<number, Element[]>();
    for (const el of items) {
      const top = Math.round(el.getBoundingClientRect().top);
      (rows.get(top) ?? rows.set(top, []).get(top)!).push(el);
    }
    const findings: string[] = [];
    for (const rowEls of rows.values()) {
      rowEls.sort((a, b) => a.getBoundingClientRect().left - b.getBoundingClientRect().left);
      const leadingSep = rowEls[0].querySelector('.footer__sep');
      if (leadingSep && getComputedStyle(leadingSep).display !== 'none') {
        findings.push(`leading: "${(rowEls[0].textContent || '').trim()}"`);
      }
      const lastText = (rowEls[rowEls.length - 1].textContent || '').trim();
      if (lastText.endsWith('·')) {
        findings.push(`trailing: "${lastText}"`);
      }
    }
    return findings;
  });
  expect(bad).toEqual([]);
});

test('the pause control never overlaps a footer link across the mobile wrap range', async ({
  page,
}) => {
  // C8 clamped the pause button's ≤960px clearance with a flat +84px, sized
  // for an assumed 2-line footer wrap — but the wrap count is non-monotonic
  // across viewport widths (measured 3 lines in the 360-460px band on real
  // device sizes: iPhone 12/13/14/15, Pixel 7), so a flat offset either
  // overlaps a taller wrap or overshoots a shorter one. Sweep the whole range
  // (the --footer-h fix) instead of spot-checking one breakpoint.
  await gotoLive(page);
  const widths = [360, 375, 390, 393, 412, 460, 480, 768, 960];
  for (const width of widths) {
    await page.setViewportSize({ width, height: 844 });
    // expect.poll tolerates the async ResizeObserver round-trip that updates
    // --footer-h after the reflow, and re-settles scroll-bottom on each retry
    // (a resize can change the page's total scroll height via the footer).
    // behavior:'instant' is load-bearing: global.css sets scroll-behavior:
    // smooth, so the 2-arg scrollTo(x, y) form (equivalent to behavior:'auto',
    // which DEFERS to that CSS) would still be animating when the rect read
    // below runs — landing this poll's first read on a pre-scroll snapshot
    // (footer off-screen, trivially zero overlap) and passing for the wrong
    // reason regardless of the real bottom-of-page geometry.
    await expect
      .poll(() =>
        page.evaluate((w) => {
          window.scrollTo({
            top: document.documentElement.scrollHeight,
            left: 0,
            behavior: 'instant',
          });
          const btn = document.getElementById('office-pause');
          if (!btn || btn.hidden) return [];
          const b = btn.getBoundingClientRect();
          return Array.from(document.querySelectorAll<HTMLAnchorElement>('.footer a'))
            .filter((a) => {
              const r = a.getBoundingClientRect();
              return !(
                r.right <= b.left ||
                r.left >= b.right ||
                r.bottom <= b.top ||
                r.top >= b.bottom
              );
            })
            .map((a) => `${w}px: ${(a.textContent || '').trim()}`);
        }, width)
      )
      .toEqual([]);
  }
});

test('the pause control never occludes in-page copy at mobile widths', async ({ page }) => {
  // R1 (wb-3 matrix sweep): .office-ctl is position:fixed, so its on-screen
  // band is CONSTANT across the whole scroll — every section's copy passes
  // under that same band at some scroll offset, not just the footer's (the
  // sibling test above only ever guarded the footer). OfficeBackdrop.astro
  // now widens .container's end-padding and caps the two office-gap
  // captions' width (they render outside .container) by the button's own
  // footprint at ≤760px. Prove it at the four convicted spots: for each,
  // scroll the page so the copy's OWN midpoint lands on the button's fixed
  // band midpoint (the worst-case alignment a visitor could ever scroll
  // to — the band is viewport-relative and constant, so this position
  // always exists short of the document's scroll ends), then check the two
  // rects don't intersect. A plain scrollIntoView({block:'center'}) does NOT
  // reproduce this — it centers the copy in the *viewport*, not in the
  // button's band near the bottom, so it can miss the real collision.
  await gotoLive(page);
  await page.setViewportSize({ width: 390, height: 844 });

  async function assertClearOfPause(selector: string): Promise<void> {
    const overlap = await page.evaluate((sel) => {
      const el = document.querySelector(sel) as HTMLElement | null;
      const btn = document.getElementById('office-pause') as HTMLElement | null;
      if (!el || !btn || btn.hidden) return { found: !!el, overlap: false };
      // Align the copy's midpoint with the button's fixed-band midpoint —
      // the worst-case scroll position for this exact pair.
      const b = btn.getBoundingClientRect();
      const r = el.getBoundingClientRect();
      const elAbsMid = (r.top + r.bottom) / 2 + window.scrollY;
      const bandMid = (b.top + b.bottom) / 2;
      window.scrollTo({ top: Math.max(0, Math.round(elAbsMid - bandMid)), behavior: 'instant' });
      const r2 = el.getBoundingClientRect();
      const b2 = btn.getBoundingClientRect();
      const overlap = !(
        r2.right <= b2.left ||
        r2.left >= b2.right ||
        r2.bottom <= b2.top ||
        r2.top >= b2.bottom
      );
      return { found: true, overlap };
    }, selector);
    expect(overlap.found, `${selector} not found`).toBe(true);
    expect(overlap.overlap, `${selector} overlaps #office-pause's fixed band`).toBe(false);
  }

  await assertClearOfPause('.hero__ghost[href="#showcase-vibing"]'); // hero CTA ("▸ play with it live")
  await assertClearOfPause('.office-gap:not(.office-gap--closer) .gap-caption'); // gap-1 caption
  await assertClearOfPause('.how__step:first-child .how__detail p'); // HowItWorks step 01 body
  await assertClearOfPause('[data-vibing-time-label]'); // Showcase VIBING clock readout
});

test('the elevator shaft never overlaps the studio panel copy at 390 or 768', async ({ page }) => {
  // R2a (wb-3 matrix sweep): .shaft is position:fixed at every width (only
  // its rail width shrinks ≤760px, via --shaft-w) — the roster's feature-row
  // text ran under it at BOTH 390 (14px dot-rail) and 768 (24px full rail),
  // since .container never reserved a gutter for it (unlike the statusline's
  // own body-padding reservation for ITS fixed bar). Horizontal position
  // doesn't depend on scroll, so this checks pure geometry, no scrolling
  // needed: every roster row's right edge must clear the shaft's left edge.
  await gotoLive(page);
  for (const width of [390, 768]) {
    await page.setViewportSize({ width, height: 844 });
    const overlaps = await page.evaluate(() => {
      const shaft = document.querySelector('.shaft');
      if (!shaft) return [];
      const shaftLeft = shaft.getBoundingClientRect().left;
      return Array.from(document.querySelectorAll<HTMLElement>('.roster__row'))
        .map((el) => el.getBoundingClientRect().right - shaftLeft)
        .filter((over) => over > 0);
    });
    expect(
      overlaps,
      `${width}px: roster rows reach ${overlaps}px past the shaft's left edge`
    ).toEqual([]);
  }
});

test('an install copy from the Install section hires a coworker: pix:install-copy → pix:hired', async ({
  page,
  context,
}) => {
  // wb-2: the closer's own copy row is gone (redundant right after Install)
  // and the statusline chip is now a plain jump link — Install.astro's tabs
  // are the surviving install-copy surface, so the hire chain is driven from
  // there.
  await context.grantPermissions(['clipboard-write']);
  const errors = watchErrors(page);
  await page.addInitScript(() => {
    sessionStorage.setItem('pix-booted', '1');
    (window as { __hired?: boolean }).__hired = false;
    document.addEventListener(
      'pix:hired',
      () => ((window as { __hired?: boolean }).__hired = true)
    );
  });
  await page.goto('./');
  // hire() is a no-op before the first live frame — wait for the office.
  await expect(page.locator('.backdrop.is-live')).toBeAttached({ timeout: 15_000 });
  await page.evaluate(() =>
    document.getElementById('install')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  await page.locator('.install__panel.is-active .install__copy').click();
  // pix:install-copy → Office.hire() → pix:hired {name}.
  await expect
    .poll(() => page.evaluate(() => (window as { __hired?: boolean }).__hired), {
      timeout: 10_000,
    })
    .toBe(true);
  expect(errors()).toEqual([]);
});

test('proof split: replay clip plays in view and obeys the page pause', async ({ page }) => {
  const errors = watchErrors(page);
  await gotoLive(page);
  await page.evaluate(() =>
    document.getElementById('proof')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  // Desktop viewport → the wide variant is the active one; it hydrates + plays.
  const vid = page.locator('.proof__video--wide');
  await expect(page.locator('.proof__video--tall')).toBeHidden();
  await expect.poll(() => vid.evaluate((v) => !(v as HTMLVideoElement).paused)).toBe(true);
  // WCAG 2.2.2: #office-pause's pix:paused signal governs the proof clip too.
  const paused = () => vid.evaluate((v) => (v as HTMLVideoElement).paused);
  await page.evaluate(() =>
    document.dispatchEvent(new CustomEvent('pix:paused', { detail: { paused: true } }))
  );
  await expect.poll(paused).toBe(true);
  await page.evaluate(() =>
    document.dispatchEvent(new CustomEvent('pix:paused', { detail: { paused: false } }))
  );
  await expect.poll(paused).toBe(false);
  // The floating-window coda survives the slot swap.
  await expect(page.locator('.proof__coda')).toContainText('pixtuoid floating');
  expect(errors()).toEqual([]);
});

test('proof split: narrow viewport swaps to the tall stack of the SAME render', async ({
  browser,
}) => {
  const context = await browser.newContext({
    viewport: { width: 390, height: 820 },
    isMobile: true,
    hasTouch: true,
  });
  const page = await context.newPage();
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  await page.evaluate(() =>
    document.getElementById('proof')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  await expect(page.locator('.proof__video--tall')).toBeVisible();
  await expect(page.locator('.proof__video--wide')).toBeHidden();
  await context.close();
});

test('proof split: narrow + reduced motion shows the tall poster, not a blank box', async ({
  browser,
}) => {
  // The active variant (tall, at this width) must promote its poster even
  // though the reduced-motion arm returns before hydrate() ever runs —
  // hydrate() is the only other place data-poster gets promoted.
  const context = await browser.newContext({
    viewport: { width: 390, height: 844 },
    isMobile: true,
    hasTouch: true,
    reducedMotion: 'reduce',
  });
  const page = await context.newPage();
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  await page.evaluate(() =>
    document.getElementById('proof')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  const tall = page.locator('.proof__video--tall');
  const wide = page.locator('.proof__video--wide');
  await expect(tall).toBeVisible();
  await expect(wide).toBeHidden();
  await expect(tall).toHaveAttribute('poster', /proof-tall-poster/);
  expect(await tall.evaluate((v) => v.querySelectorAll('source').length)).toBe(0);
  expect(await wide.evaluate((v) => v.querySelectorAll('source').length)).toBe(0);
  await expect.poll(() => tall.evaluate((v) => (v as HTMLVideoElement).paused)).toBe(true);
  await context.close();
});
