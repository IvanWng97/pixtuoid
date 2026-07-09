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
