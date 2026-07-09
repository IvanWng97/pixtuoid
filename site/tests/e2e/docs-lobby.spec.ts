import { expect, test, type Page } from '@playwright/test';

// wb-5 runtime contracts: the docs shell joins the building (statusline mount,
// callout chrome, closing install strip) and the lobby floor (tenant board,
// star plaque, pantry FAQ). Runs against the PRODUCTION build (see
// playwright.config.ts) — same posture as smoke.spec.ts.

function watchErrors(page: Page): () => string[] {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
  page.on('console', (msg) => {
    if (msg.type() === 'error') errors.push(`console.error: ${msg.text()}`);
  });
  return () => errors;
}

test('docs mount the statusline: route path left, install chip right, no landing organs', async ({
  page,
}) => {
  const errors = watchErrors(page);
  await page.goto('./config');
  const bar = page.locator('#statusline');
  await expect(bar).toBeVisible();
  await expect(bar).toContainText('~ pixtuoid docs · /config');
  // wb-1's right-end chip block mounts on every variant
  await expect(bar.locator('#sl-install')).toBeAttached();
  // the landing organs (floor lift, feed, env readouts) are index-only
  await expect(bar.locator('[data-floor-toggle]')).toHaveCount(0);
  await expect(bar.locator('.sl__feed')).toHaveCount(0);
  expect(errors()).toEqual([]);
});

test('404 mounts the statusline too', async ({ page }) => {
  await page.goto('./no-such-desk');
  await expect(page.locator('#statusline')).toContainText('~ pixtuoid docs · /404');
});

test('the docs sidebar is an elevator panel: current-doc LED + a building bank from FLOORS', async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 900 }); // rails hide below 1000px
  await page.goto('./config');
  const panel = page.locator('.docs__sidebar');
  await expect(panel).toHaveClass(/hw-panel/);
  // the current doc's LED is the lit one
  await expect(panel.locator('a[aria-current="page"] .led-dot')).toHaveClass(/led-selected/);
  expect(await panel.locator('.led-dot.led-selected').count()).toBe(1);
  // the building bank reads the shared FLOORS manifest — 6 floors, landing anchors
  const building = panel.locator('.docs__list--building a');
  await expect(building).toHaveCount(6);
  await expect(building.first()).toHaveAttribute('href', /\/pixtuoid\/#/);
});

test('markdown callouts render as terminal windows (note + warning)', async ({ page }) => {
  // CONTRIBUTING.md:34 carries the "**Don't chain …**" blockquote → warning;
  // ARCHITECTURE.md:5 carries a plain editorial blockquote → note.
  await page.goto('./contributing');
  const warn = page.locator('.callout--warn').first();
  await expect(warn).toBeVisible();
  await expect(warn.locator('.callout__title')).toHaveText('~ warning');
  await expect(warn.locator('.terminal__dot--r')).toBeAttached();
  await page.goto('./architecture');
  const note = page.locator('.callout--note').first();
  await expect(note).toBeVisible();
  await expect(note.locator('.callout__title')).toHaveText('~ note');
});

test('tenant board: hw-panel screen, LED dots with shape distinction, sr-only values survive', async ({
  page,
}) => {
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  await page.evaluate(() =>
    document.getElementById('tools')!.scrollIntoView({ block: 'center', behavior: 'instant' })
  );
  const board = page.locator('#tools .tools__board');
  await expect(board).toHaveClass(/hw-panel/);
  // ten supported CLIs × macOS = at least ten lit LEDs; windows column = rings
  expect(await board.locator('.tools__led--on').count()).toBeGreaterThanOrEqual(10);
  expect(await board.locator('.tools__led--exp').count()).toBeGreaterThan(0);
  // the LED is aria-hidden; the value still rides the sr-only sibling (WCAG)
  await expect(board.locator('td.tools__cell .sr-only').first()).toHaveText(
    /supported|experimental/
  );
  // the section anchor speaks the shared floor vocabulary (wb-1 already stamped it)
  await expect(page.locator('#tools')).toHaveAttribute('data-floor', '2F');
});
