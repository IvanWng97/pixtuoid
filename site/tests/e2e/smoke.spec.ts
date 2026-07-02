import { expect, test, type Page } from '@playwright/test';

// The smoke suite: one assertion per cross-component CONTRACT of the OPEN
// FLOOR page — the seams that only exist at runtime (window globals, custom
// events, data-attribute wiring) where tsc/eslint/knip/astro-build are blind.
// Every test here is a regression pin for a bug class a human review actually
// caught on this site:
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
  expect(errors()).toEqual([]);
  await context.close();
});
