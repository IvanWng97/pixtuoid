import { expect, test, type Page } from '@playwright/test';

// The smoke suite: one assertion per cross-component CONTRACT of the OPEN
// FLOOR page — the seams that only exist at runtime (window globals, custom
// events, data-attribute wiring) where tsc/eslint/knip/astro-build are blind.
// The first seven tests are regression pins for bug classes a human review
// actually caught on this site:
//   - the missed one-shot `pix:onair` event (statusline read STILL forever)
//   - the `is:inline` parse-position trap (scrollspy frozen on floor 6)
//   - the floating-nav variant leaking onto the docs pages
//   - a wasm/glue ABI mismatch throwing at runtime under the hero
// Runs against the PRODUCTION build (see playwright.config.ts).

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
  // height is fixed at 180, so width = min(640, max(64, round(w/h · 180))) —
  // 320 at the 1280×720 default, 100 at a 500×900 portrait.
  const bufW = () =>
    page.evaluate(() => (document.getElementById('office-live') as HTMLCanvasElement).width);
  expect(await bufW()).toBe(320);
  await page.setViewportSize({ width: 500, height: 900 });
  await expect.poll(bufW).toBe(100);
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
      }))
    )
    .toEqual({ night: true, hire: true, lights: 'number' });
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

test('the dimmer darkens statements and releases in office gaps', async ({ page }) => {
  await gotoLive(page);
  const dim = () =>
    page.evaluate(() => parseFloat(document.getElementById('dimmer')!.style.opacity || '0'));
  // A statement at viewport center pulls the darkness in…
  await page.evaluate(() =>
    document.getElementById('features')!.scrollIntoView({ block: 'center', behavior: 'instant' })
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
    document.getElementById('features')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  await expect.poll(heroOp).toBeLessThan(0.01);
  await page.evaluate(() => window.scrollTo({ top: 0, behavior: 'instant' }));
  await expect.poll(heroOp).toBeGreaterThan(0.5);
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
  // pre-copy __pixHire() call (the #436 wiring) didn't throw.
  await expect(copy).toHaveText(/Copied|Select & copy/);
  expect(errors()).toEqual([]);
});

test('docs pages keep the sticky nav with section links', async ({ page }) => {
  // The floating-nav treatment is index-ONLY; the docs pages have no office
  // backdrop or statusline, so they keep the sticky bar (the #426-review
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
  // Reduced motion also strips the showcase clip's autoplay: native controls
  // appear and the video stays paused (WCAG 2.2.2).
  const video = page.locator('[data-stage="agents"] video');
  await expect(video).toHaveAttribute('controls', '');
  await expect.poll(() => video.evaluate((v) => (v as HTMLVideoElement).paused)).toBe(true);
  expect(errors()).toEqual([]);
  await context.close();
});

// ---------------------------------------------------------------------------
// The tests below came out of the sitewide interaction audit (91 catalogued
// listeners/globals/observers → these six): every runtime contract with
// med+ user impact and low flake risk that the tests above didn't already pin.

test('first visit: boot intro auto-runs, reveals the page, seeds the gate', async ({ page }) => {
  await page.goto('./'); // NO pix-booted seed — the real first visit
  await expect(page.locator('#boot')).toBeVisible();
  // The auto-run finishes in ~2.5s of sequenced timeouts — poll, no fixed wait.
  await expect(page.locator('html')).not.toHaveAttribute('data-booting', '1', { timeout: 8_000 });
  await expect.poll(() => page.evaluate(() => sessionStorage.getItem('pix-booted'))).toBe('1');
  expect(await page.evaluate(() => document.getElementById('main')!.hasAttribute('inert'))).toBe(
    false
  );
  // finish() dispatched pix:revealed, arming the reveal-on-scroll observer —
  // opacity:0 still counts as "visible" to Playwright, so assert the CLASS.
  await page.evaluate(() =>
    document.getElementById('features')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  await expect(page.locator('#features .section-head.reveal')).toHaveClass(/\bin\b/);
  // Gate round-trip: a seeded session skips the overlay, and the IMMEDIATE
  // pix:revealed path must arm the reveal observer just the same.
  await page.reload();
  await expect(page.locator('#boot')).not.toBeVisible();
  await page.evaluate(() =>
    document.getElementById('features')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  await expect(page.locator('#features .section-head.reveal')).toHaveClass(/\bin\b/);
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
  await page.goto('./#themes'); // the legacy anchor maps to the themes channel
  await expect(page.locator('[data-stage="themes"]')).toBeVisible();
  await expect(page.locator('button.mon[data-ch="themes"]')).toHaveAttribute(
    'aria-pressed',
    'true'
  );
  // First tune hydrated the stage: data-src promoted to a real src.
  await expect(page.locator('[data-stage="themes"] img.terminal__screen')).toHaveAttribute(
    'src',
    /theme_/
  );
  // An in-page hashchange re-tunes.
  await page.evaluate(() => {
    location.hash = '#showcase-weather';
  });
  await expect(page.locator('[data-stage="weather"]')).toBeVisible();
  // Dial click: exactly-one-visible-stage swap + aria radio + URL tracking.
  await page.locator('button.mon[data-ch="spaces"]').click();
  await expect(page.locator('[data-stage="spaces"]')).toBeVisible();
  await expect(page.locator('[data-stage="weather"]')).toBeHidden();
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
  expect(errors()).toEqual([]);
});

test('nav menus + docs: dropdown, TOC scrollspy, 404, mobile burger', async ({ page, browser }) => {
  const errors = watchErrors(page);
  await page.goto('./config#themes'); // arrival-by-hash: the rail lights unscrolled
  await expect(page.locator('[data-toc-link="themes"]')).toHaveAttribute(
    'aria-current',
    'location'
  );
  // The Docs dropdown is the ONLY route to the six doc pages.
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
  await expect(page.locator('.lost__cta .btn-primary')).toHaveAttribute('href', '/pixtuoid/');
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

test('landing fixed chrome: floating nav, statusline readouts, floor popover, day/night gap', async ({
  page,
}) => {
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
  // Gap-2's claim must AGREE with the one clock boundary — consistency, not a
  // fixed value, so it's green at any hour.
  const s = await page.evaluate(() => ({
    night: window.__pixNight!(),
    word: document.querySelector('[data-gap-daynight]')!.textContent,
    src: (document.querySelector('[data-gap-still]') as HTMLImageElement).src,
  }));
  expect(s.word).toBe(s.night ? 'night' : 'day');
  expect(s.src).toContain(s.night ? 'night.png' : 'day.png');
  // Floor popover: toggle → Esc closes → reopen → a floor jump closes AND
  // rides the lift (the same scrollspy round-trip as the digit-keys test).
  const toggle = page.locator('[data-floor-toggle]');
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');
  await expect(page.locator('#sl-floors')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.locator('#sl-floors')).toBeHidden();
  await toggle.click();
  await page.locator('[data-floor-btn="1"]').click();
  await expect(page.locator('#sl-floors')).toBeHidden();
  await expect(page.locator('[data-lift-digit]')).toHaveText('1F', { timeout: 10_000 });
});
