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
  const m = css.match(/rgba?\(([^)]+)\)/);
  if (m) {
    const [r, g, b, a] = m[1].split(',').map((s) => parseFloat(s));
    return [r, g, b, a ?? 1];
  }
  // A color-mix()-derived computed value serializes in the CSS Color 4
  // color() functional notation (0..1 floats), not legacy rgb() 0..255 —
  // hit by Hero.astro's .hero__badge-code contrast lift.
  const cm = css.match(/^color\(srgb\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)(?:\s*\/\s*([\d.]+))?\)$/);
  if (cm) {
    const [, r, g, b, a] = cm;
    return [parseFloat(r) * 255, parseFloat(g) * 255, parseFloat(b) * 255, a ? parseFloat(a) : 1];
  }
  throw new Error(`unparseable color: ${css}`);
}
function parseHex(hex: string): [number, number, number] {
  const m = hex.trim().match(/^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i);
  if (!m) throw new Error(`unparseable hex color: ${hex}`);
  return [parseInt(m[1], 16), parseInt(m[2], 16), parseInt(m[3], 16)];
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
  await expect.poll(bufW).toBe(100); // re-aspected
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

test('hero install row: copy chip flashes and fires pix:install-copy {source:hero}', async ({
  page,
  context,
}) => {
  await context.grantPermissions(['clipboard-write']);
  const errors = watchErrors(page);
  await page.addInitScript(() => {
    sessionStorage.setItem('pix-booted', '1');
    (window as unknown as { __copySources: string[] }).__copySources = [];
    document.addEventListener('pix:install-copy', (e) =>
      (window as unknown as { __copySources: string[] }).__copySources.push(
        (e as CustomEvent<{ source: string }>).detail.source
      )
    );
  });
  await page.goto('./');
  // The primary CTA carries the REAL brew command (single-sourced from install.json).
  await expect(page.locator('#hero-install-cmd')).toContainText(
    'brew install IvanWng97/pixtuoid/pixtuoid'
  );
  // Ten CLI badges — one per sources.json entry, compatibility answered in viewport 1.
  await expect(page.locator('.hero__badge')).toHaveCount(10);
  const copy = page.locator('#hero-install-row [data-install-copy]');
  await copy.click();
  await expect(copy).toHaveText(/copied|select & copy/);
  await expect
    .poll(() =>
      page.evaluate(() => (window as unknown as { __copySources: string[] }).__copySources)
    )
    .toContain('hero');
  expect(errors()).toEqual([]);
});

test('an install copy hires a coworker: pix:install-copy → pix:hired', async ({
  page,
  context,
}) => {
  await context.grantPermissions(['clipboard-write']);
  const errors = watchErrors(page);
  await gotoLive(page); // hire needs the LIVE office (__pixHire exists)
  await page.evaluate(() => {
    (window as unknown as { __hired: string[] }).__hired = [];
    document.addEventListener('pix:hired', (e) =>
      (window as unknown as { __hired: string[] }).__hired.push(
        (e as CustomEvent<{ name: string }>).detail.name
      )
    );
  });
  expect(await page.evaluate(() => window.__pixInstall!.copy('closer'))).toBe(true);
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __hired: string[] }).__hired))
    .toEqual(['cc·yours']);
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
  // it just returns false).
  await context.grantPermissions(['clipboard-write']);
  const errors = watchErrors(page);
  await gotoLive(page); // hire needs the LIVE office (__pixHire exists)
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
  for (let i = 0; i < 4; i++) {
    expect(await page.evaluate(() => window.__pixInstall!.copy('statusline'))).toBe(true);
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
  // fetch never runs, so window.__pixHire is never published. copy() must
  // still succeed (the clipboard write is independent of the office) and
  // OfficeBackdrop's `if (!window.__pixHire) return;` guard must make the
  // hire side a true no-op — no throw, no pix:hired receipt.
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
  expect(await page.evaluate(() => window.__pixInstall!.copy('statusline'))).toBe(true);
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
  // Reduced motion is the ONLY path that hides the pause switch: nothing
  // auto-animates here (the wasm-fail poster keeps it visible — ticker/dust/clips
  // still run there, see the wasm-failure test).
  await expect(page.locator('#office-pause')).toBeHidden();
  // Reduced motion also strips the showcase clip's autoplay: native controls
  // appear and the video stays paused (WCAG 2.2.2).
  const video = page.locator('[data-stage="agents"] video');
  await expect(video).toHaveAttribute('controls', '');
  await expect.poll(() => video.evaluate((v) => (v as HTMLVideoElement).paused)).toBe(true);
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
  await expect(page.locator('[data-sl-onair]')).toHaveText('○ STILL');
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

test('statusline install chip: copy flashes ✓, clipboard gets the one-liner, then the hire receipt', async ({
  page,
  context,
}) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  const errors = watchErrors(page);
  await gotoLive(page); // live office → the copy also hires → the receipt
  const label = page.locator('#sl-install .sl__copy-label');
  await expect(label).toHaveText('brew install');
  await page.locator('#sl-install [data-sl-copy]').click();
  await expect(label).toHaveText('copied ✓');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    'brew install IvanWng97/pixtuoid/pixtuoid'
  );
  // the receipt queues BEHIND the 2s copied-✓ window, then flashes
  await expect(label).toHaveText('you · hired · just now', { timeout: 6_000 });
  // …and the chip returns to rest
  await expect(label).toHaveText('brew install', { timeout: 6_000 });
  expect(errors()).toEqual([]);
});

test('statusline install chip: the icon-only mobile collapse still shows the copied/hired flash (review round, #504)', async ({
  page,
  context,
}) => {
  // ≤760px hides .sl__copy-label — the desktop test above asserts on TEXT
  // that's invisible here. This pins the glyph swap + chip pulse that stand
  // in for it (a pixel-diff at this width showed no perceivable change
  // before this fix — sighted mobile users got zero copy confirmation).
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  const errors = watchErrors(page);
  // U5: the copy flash and the hire receipt share .is-flash — a naive
  // classList.toggle('is-flash', true) is a no-op (and the CSS animation
  // can't restart) when the receipt fires while the copy flash's OWN class
  // is still on. Count REAL animationstart events for the chip-pulse
  // keyframe (not just class presence) to prove it fires twice, not once.
  await page.addInitScript(() => {
    (window as unknown as { __chipPulses: number }).__chipPulses = 0;
    document.addEventListener('animationstart', (e) => {
      if ((e as AnimationEvent).animationName === 'chip-pulse') {
        (window as unknown as { __chipPulses: number }).__chipPulses++;
      }
    });
  });
  await gotoLive(page); // live office → the copy also hires → the receipt
  await page.setViewportSize({ width: 375, height: 800 });
  const chip = page.locator('#sl-install .sl__copy');
  const flashIcon = page.locator('#sl-install .sl__copy-icon-flash');
  await expect(chip).not.toHaveClass(/is-flash/);
  await expect(flashIcon).toBeHidden();
  await page.locator('#sl-install [data-sl-copy]').click();
  await expect(chip).toHaveClass(/is-flash/);
  await expect(flashIcon).toBeVisible();
  // …and once the whole copied → hired-receipt sequence settles, it reverts
  await expect(page.locator('#sl-install .sl__copy-label')).toHaveText('brew install', {
    timeout: 8_000,
  });
  await expect(chip).not.toHaveClass(/is-flash/);
  await expect(flashIcon).toBeHidden();
  // TWO starts: the initial copy flash, then the queued hire-receipt flash —
  // the bug measured exactly 1 (the second toggle was a same-value no-op).
  expect(
    await page.evaluate(() => (window as unknown as { __chipPulses: number }).__chipPulses)
  ).toBe(2);
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
  await page.keyboard.press('4');
  await expect(page.locator('[data-lift-digit]')).toHaveText('2F');
  // …and the choice is persisted (single-char shortcuts have a real off-switch)
  expect(await page.evaluate(() => localStorage.getItem('pix-keys'))).toBe('off');
  // flip it back ON — the digit rides again
  await keysToggle.click();
  await expect(keysToggle).toHaveAttribute('aria-checked', 'true');
  await page.keyboard.press('4');
  await expect(page.locator('[data-lift-digit]')).toHaveText('4F', { timeout: 10_000 });
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
  // Splash log displays 4 lines (~2.1s) then holds for engine (~4s MAX_ENGINE_WAITS) + settle fade (460ms) ≈ 6.5s.
  await expect(page.locator('html')).not.toHaveAttribute('data-booting', '1', { timeout: 10_000 });
  await expect.poll(() => page.evaluate(() => sessionStorage.getItem('pix-booted'))).toBe('1');
  expect(await page.evaluate(() => document.getElementById('main')!.hasAttribute('inert'))).toBe(
    false
  );
  // finish() dispatched pix:revealed, arming the reveal-on-scroll observer —
  // opacity:0 still counts as "visible" to Playwright, so assert the CLASS.
  await expectSectionReveal(page, 'features');
  // Gate round-trip: a seeded session skips the overlay, and the IMMEDIATE
  // pix:revealed path must arm the reveal observer just the same.
  await page.reload();
  await expect(page.locator('#boot')).not.toBeVisible();
  await expectSectionReveal(page, 'features');
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
  // Splash clears data-booting in ~2.1s (4×450ms line dwell) + 460ms fade ≈ 2.6s; the unguarded gate hangs to ~5.9s. 3.5s separates.
  await expect(page.locator('html')).not.toHaveAttribute('data-booting', '1', { timeout: 3_500 });
  expect(errors()).toEqual([]);
});

test('first visit: splash displays 4-line log with per-line dwell (~450ms)', async ({ page }) => {
  const errors = watchErrors(page);
  // Test on docs page (no office, no engine wait) for pure splash-timing measurement.
  await page.goto('./config/'); // NO pix-booted seed — the real first visit
  await expect(page.locator('#boot')).toBeVisible();
  // The splash displays 4 log lines: version, booting, themes, CLI count.
  await expect(page.locator('#boot .boot__log')).toContainText('pixtuoid');
  await expect(page.locator('#boot .boot__log')).toContainText('booting office');
  await expect(page.locator('#boot .boot__log')).toContainText('loading themes');
  await expect(page.locator('#boot .boot__log')).toContainText('10 CLIs connected');
  // Splash clears data-booting in ~2.1s (4×450ms line dwell) + 460ms fade ≈ 2.6s.
  await expect(page.locator('html')).not.toHaveAttribute('data-booting', '1', {
    timeout: 3_500,
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
  for (const [path, width] of [
    ['./', 360], // Android
    ['./', 390], // iPhone 12–14
    ['./', 430], // iPhone Pro Max
    ['./config', 390], // docs shell: code blocks / mermaid can overflow too
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
    const { scrollW, clientW } = await page.evaluate(() => ({
      scrollW: document.documentElement.scrollWidth,
      clientW: document.documentElement.clientWidth,
    }));
    expect(
      scrollW,
      `${path} at ${width}px is ${scrollW - clientW}px wider than the viewport (horizontal pan)`
    ).toBeLessThanOrEqual(clientW);
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
  // §5: legibility must not depend on the center-anchored scroll dimmer — the
  // hero subcopy and the feature ledger rows carry a local scrim.
  const bg = await page.evaluate(
    () => getComputedStyle(document.querySelector('.hero .statement-sub')!).backgroundColor
  );
  expect(bg).not.toBe('rgba(0, 0, 0, 0)');
  expect(await page.locator('#features .ledger__row.text-scrim').count()).toBeGreaterThan(0);
});

test('the scrimmed hero subcopy clears WCAG AA at the worst-case composite (day theme)', async ({
  page,
}) => {
  // The binding constraint is WCAG AA (4.5:1) for EVERY token. The worst
  // case is day theme, since it's the theme whose --fg-muted/--scrim pairing
  // has the least headroom: the hero subcopy (--fg-muted) inside
  // .text-scrim, painted over --screen (the darkest pixel the office ever
  // renders). The hero's data-lit-max="0" (position-purity redesign — the
  // mask never darkens the hero, at any scroll position) makes the dimmer
  // step below collapse to an IDENTITY (afterDimmer === officeWorstPixel),
  // so .text-scrim alone carries the whole contrast burden — hence the
  // day --scrim alpha lift to 0.97 (see global.css). Reads REAL computed
  // styles (not hardcoded token values) so a future
  // --scrim/--fg-muted/data-lit-max regression fails this test rather than
  // only a visual read.
  await page.addInitScript(() => {
    sessionStorage.setItem('pix-booted', '1');
    localStorage.setItem('pix-theme', 'day');
  });
  await page.goto('./');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'day');

  const measured = await page.evaluate(() => {
    const sub = document.querySelector('.hero .statement-sub')!;
    const litBlock = document.querySelector('.hero__copy') as HTMLElement;
    return {
      textColor: getComputedStyle(sub).color,
      scrimBg: getComputedStyle(sub).backgroundColor,
      dimmerBg: getComputedStyle(document.getElementById('dimmer')!).backgroundColor,
      dataLitMax: parseFloat(litBlock.dataset.litMax!),
      screenToken: getComputedStyle(document.documentElement).getPropertyValue('--screen'),
    };
  });

  // --screen is a PROXY for the darkest pixel the live office canvas actually
  // renders (a real frame sample isn't practical here) — reviewer-verified
  // immaterial: the ratio shift from any plausible canvas-vs-token delta is
  // <0.005, against a 0.22 margin above the 4.5:1 floor (measured ~4.72:1).
  const officeWorstPixel = parseHex(measured.screenToken);
  const afterDimmer = compositeOver(
    [...parseRgb(measured.dimmerBg).slice(0, 3), measured.dataLitMax] as [
      number,
      number,
      number,
      number,
    ],
    officeWorstPixel
  );
  const afterScrim = compositeOver(parseRgb(measured.scrimBg), afterDimmer);
  const ratio = contrastRatio(
    parseRgb(measured.textColor).slice(0, 3) as [number, number, number],
    afterScrim
  );

  expect(ratio, `WCAG AA floor is 4.5:1; measured ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(
    4.5
  );
});

test('the hero headline outline clears WCAG against the office darkest pixel (day AND night)', async ({
  page,
}) => {
  // Follow-up to the position-purity redesign: with the hero permanently
  // exempt from the dimmer, the headline sits over a live, fully-lit office
  // at ANY scroll position — day's dark ink over the office's darkest
  // pixel (--screen, e.g. a monitor bezel) is itself LOW contrast; the
  // pixel-outline (an 8-direction 0-blur text-shadow stack, solid
  // var(--bg)) is what rescues it, so legibility only needs ONE of
  // {fill, outline} to clear the floor against the worst pixel — reads
  // REAL computed styles so a future edit that drops the shadow rule (day)
  // or weakens the reused --bg polarity (either theme) fails this test.
  for (const theme of ['day', 'night'] as const) {
    await page.addInitScript((t) => {
      sessionStorage.setItem('pix-booted', '1');
      localStorage.setItem('pix-theme', t);
    }, theme);
    await page.goto('./');
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const measured = await page.evaluate(() => {
      const st = document.querySelector('.hero .statement')!;
      const cs = getComputedStyle(st);
      return {
        textShadow: cs.textShadow,
        fill: cs.color,
        outline: getComputedStyle(document.documentElement).getPropertyValue('--bg'),
        screenToken: getComputedStyle(document.documentElement).getPropertyValue('--screen'),
      };
    });
    expect(measured.textShadow, `${theme}: headline must carry the pixel-outline shadow`).not.toBe(
      'none'
    );
    const worst = parseHex(measured.screenToken);
    const fillRatio = contrastRatio(
      parseRgb(measured.fill).slice(0, 3) as [number, number, number],
      worst
    );
    const outlineRatio = contrastRatio(parseHex(measured.outline), worst);
    expect(
      Math.max(fillRatio, outlineRatio),
      `${theme}: neither fill (${fillRatio.toFixed(2)}:1) nor outline (${outlineRatio.toFixed(2)}:1) clears WCAG's 3:1 large-text floor against the office's darkest pixel`
    ).toBeGreaterThanOrEqual(3);
  }
});

test('small chrome text clears WCAG against the office darkest pixel AND a mid-tone office pixel (day AND night)', async ({
  page,
}) => {
  // Follow-up to the headline outline: the same --text-outline idea proved
  // too weak at small mono sizes (eyebrow, hero CTA ghost links, the
  // alt-install/avail line, the floating nav's Docs/GitHub/Coffee) against
  // BUSY office pixels — the soft --chrome-halo alone measured weaker
  // still. --text-outline-sm (global.css) layers a SECOND ring in --screen
  // OUTSIDE the --bg ring: a single --bg-polarity ring alone measured only
  // ~3.8-4.1:1 against a MID-TONE office pixel (the carpet --paper was
  // lifted from, #8A765F, documented in this same file) — short of WCAG's
  // 4.5:1 small-text floor. The added --screen ring lifts every surface to
  // a consistent ~4.47:1 (the best achievable reusing the office's OWN
  // darkest token — a bespoke pure black would clear 4.5 outright but
  // isn't itself used anywhere else in this design system, so it's not
  // introduced just to chase the last 0.03). The 4.4 floor below pins
  // exactly that measured, honest ceiling with a small margin: it fails a
  // regression back toward the single-ring ~3.8-4.1:1, and passes today's
  // real ~4.47:1 with room to spare.
  const CARPET: [number, number, number] = [0x8a, 0x76, 0x5f]; // the office carpet global.css's --paper comment cites
  const CARPET_FLOOR = 4.4;
  const SURFACES: Array<[string, string]> = [
    ['eyebrow', '.hero .eyebrow'],
    ['hero CTA ghost link', '.hero__ghost'],
    ['alt-install line', '.hero__alt-install'],
    ['avail line', '.hero__avail'],
    ['nav GitHub link', '.nav--floating .nav__links a[href*="github.com"]'],
  ];
  for (const theme of ['day', 'night'] as const) {
    await page.addInitScript((t) => {
      sessionStorage.setItem('pix-booted', '1');
      localStorage.setItem('pix-theme', t);
    }, theme);
    await page.goto('./');
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const tokens = await page.evaluate(() => {
      const root = getComputedStyle(document.documentElement);
      return {
        bg: root.getPropertyValue('--bg').trim(),
        screenToken: root.getPropertyValue('--screen').trim(),
      };
    });
    const screenPixel = parseHex(tokens.screenToken);
    const innerRing = parseHex(tokens.bg);
    const outerRing = screenPixel; // --text-outline-sm's second ring IS --screen

    for (const [name, selector] of SURFACES) {
      const measured = await page.evaluate((sel) => {
        const el = document.querySelector(sel);
        if (!el) return null;
        const cs = getComputedStyle(el);
        return { textShadow: cs.textShadow, fill: cs.color };
      }, selector);
      expect(measured, `${theme}: ${name} (${selector}) must exist`).not.toBeNull();
      expect(measured!.textShadow, `${theme}: ${name} must carry a pixel-outline shadow`).not.toBe(
        'none'
      );
      const fill = parseRgb(measured!.fill).slice(0, 3) as [number, number, number];
      const vsScreen = Math.max(
        contrastRatio(fill, screenPixel),
        contrastRatio(innerRing, screenPixel),
        contrastRatio(outerRing, screenPixel)
      );
      const vsCarpet = Math.max(
        contrastRatio(fill, CARPET),
        contrastRatio(innerRing, CARPET),
        contrastRatio(outerRing, CARPET)
      );
      expect(
        vsScreen,
        `${theme}: ${name} — best of {fill,inner,outer} vs --screen is ${vsScreen.toFixed(2)}:1, below the 4.5:1 small-text floor`
      ).toBeGreaterThanOrEqual(4.5);
      expect(
        vsCarpet,
        `${theme}: ${name} — best of {fill,inner,outer} vs the mid-tone carpet is ${vsCarpet.toFixed(2)}:1, below the accepted ${CARPET_FLOOR}:1 floor (4.5 is unreachable reusing only office-derived tokens — see global.css --text-outline-sm)`
      ).toBeGreaterThanOrEqual(CARPET_FLOOR);
    }
  }
});

test('hero badge codes: every per-source hue clears WCAG AA on the chip screen', async ({
  page,
}) => {
  // U4 amendment: 3 of the 10 badge_color hues (Reasonix, Hermes, opencode) fail
  // 4.5:1 raw against --screen — the hues are bridge-pinned wire facts
  // (sources.json <-> pixtuoid_scene theme), so Hero.astro lifts the CODE toward
  // white in this text context only (.hero__badge-code's color-mix). --screen and
  // --badge are both theme-independent, so one theme (the default) covers every
  // visitor. Reads REAL computed styles, not the color-mix math, so a future
  // sources.json hue or lift-percentage regression fails this test.
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  const measured = await page.evaluate(() => {
    const chips = Array.from(document.querySelectorAll('.hero__badge'));
    return chips.map((chip) => ({
      code: chip.querySelector('.hero__badge-code')!.textContent,
      textColor: getComputedStyle(chip.querySelector('.hero__badge-code')!).color,
      bg: getComputedStyle(chip).backgroundColor,
    }));
  });
  expect(measured.length).toBe(10);
  for (const m of measured) {
    const ratio = contrastRatio(
      parseRgb(m.textColor).slice(0, 3) as [number, number, number],
      parseRgb(m.bg).slice(0, 3) as [number, number, number]
    );
    expect(
      ratio,
      `badge "${m.code}": WCAG AA floor is 4.5:1; measured ${ratio.toFixed(2)}:1`
    ).toBeGreaterThanOrEqual(4.5);
  }
});

test('the dimmer is a pure function of scroll position — no first-scroll latch', async ({
  page,
}) => {
  // Regression pin (user-identified): the removed design tracked a visit-
  // scoped `engaged` latch that flipped true on the first scroll gesture and
  // never released, so the hero read bright on a fresh load but dimmed after
  // scrolling down and back up — same scrollY, different appearance, purely
  // from history. The hero's data-lit-max="0" now makes the mask permanently
  // exempt there, so its contribution is 0 at ANY scroll position, fresh or
  // returned-to.
  await gotoLive(page);
  const dim = () =>
    page.evaluate(() => parseFloat(document.getElementById('dimmer')!.style.opacity || '0'));
  // Full lights on a fresh load — the live office IS the first content.
  await expect.poll(dim).toBe(0);
  // Scroll deep enough to actually engage the mask elsewhere on the page —
  // the old latch would flip true here and never release.
  await page.evaluate(() =>
    document.getElementById('features')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  await expect.poll(dim).toBeGreaterThan(0.5);
  // Back to the hero's exact position: must read the SAME full-lights value
  // it did before the deep scroll, not a residual latch-darkened one.
  await page.evaluate(() => window.scrollTo({ top: 0, behavior: 'instant' }));
  await expect.poll(dim).toBe(0);
  // The closer sits in its own office-gap (not a [data-lit] block, unaffected
  // by this redesign) — the mask releases there too.
  await page.evaluate(() =>
    document
      .querySelector('[data-office-hour]')!
      .scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  await expect.poll(dim).toBeLessThan(0.15);
});

test('the closer hold carries the install line and fires pix:install-copy {source:closer}', async ({
  page,
  context,
}) => {
  await context.grantPermissions(['clipboard-write']);
  const errors = watchErrors(page);
  await page.addInitScript(() => {
    sessionStorage.setItem('pix-booted', '1');
    (window as { __copySources?: string[] }).__copySources = [];
    document.addEventListener('pix:install-copy', (e) =>
      (window as { __copySources?: string[] }).__copySources!.push(
        (e as CustomEvent<{ source: string }>).detail.source
      )
    );
  });
  await page.goto('./');
  await page.evaluate(() =>
    document
      .querySelector('[data-office-hour]')!
      .scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  const chip = page.locator('[data-office-hour] [data-install-copy]');
  await expect(chip).toBeVisible();
  await chip.click();
  await expect(chip).toHaveText(/copied|select & copy/);
  await expect
    .poll(() => page.evaluate(() => (window as { __copySources?: string[] }).__copySources))
    .toContain('closer');
  expect(errors()).toEqual([]);
});

test('the golden-hour ratchet advances the render clock when the closer intersects', async ({
  page,
}) => {
  const errors = watchErrors(page);
  // Step-clock instrumentation: wrap Office.prototype.step, once the real
  // wasm module resolves through window.__pixWasm, to record the LAST now_ms
  // the engine was actually stepped with. Deterministic at any wall-clock
  // time — sampling the canvas's sky color instead would shrink to nothing
  // if this test happens to run when the real hour is already near golden
  // hour. Both the hero backdrop and the VIBING channel await the SAME
  // window.__pixWasm promise (site/CLAUDE.md), so patching Office.prototype
  // here catches every Office this page creates.
  await page.addInitScript(() => {
    let wrapped: Promise<unknown> | undefined;
    Object.defineProperty(window, '__pixWasm', {
      configurable: true,
      get: () => wrapped,
      set: (
        promise: Promise<{
          Office: { prototype: { step: (_nowMs: number, _w: number, _h: number) => void } };
        }>
      ) => {
        wrapped = promise.then((mod) => {
          const orig = mod.Office.prototype.step;
          mod.Office.prototype.step = function (
            this: unknown,
            nowMs: number,
            w: number,
            h: number
          ) {
            (window as { __lastStepNowMs?: number }).__lastStepNowMs = nowMs;
            return orig.call(this, nowMs, w, h);
          };
          return mod;
        });
      },
    });
  });
  await gotoLive(page);
  const lastHour = () =>
    page.evaluate(() => {
      const ms = (window as { __lastStepNowMs?: number }).__lastStepNowMs;
      return ms === undefined ? NaN : new Date(ms).getHours();
    });
  // Sanity: the wrapper is actually capturing frames before the ratchet.
  await expect.poll(async () => Number.isNaN(await lastHour())).toBe(false);
  await page.evaluate(() =>
    document
      .querySelector('[data-office-hour]')!
      .scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  // GOLDEN_HOUR in OfficeBackdrop.astro — pinned here per CLAUDE.md's "two
  // copies of a magic value" rule: this poll is the pairing check, and it's
  // the ONLY regression coverage for the ratchet's observable effect (the
  // closer-install test above only pins the install-copy wiring).
  await expect.poll(lastHour, { timeout: 5_000 }).toBe(17);
  expect(errors()).toEqual([]);
});

test('hero copy → hire receipt: pix:install-copy walks a coworker in (pix:hired)', async ({
  page,
  context,
}) => {
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
  await page.locator('#hero-install-row [data-install-copy]').click();
  // wb-1's bridge: pix:install-copy → Office.hire() → pix:hired {name}.
  await expect
    .poll(() => page.evaluate(() => (window as { __hired?: boolean }).__hired), {
      timeout: 10_000,
    })
    .toBe(true);
  expect(errors()).toEqual([]);
});
