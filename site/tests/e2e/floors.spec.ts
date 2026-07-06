import { expect, test } from '@playwright/test';

// wb-3's runtime contracts: the merged 5F band (feature rows ARE channel
// triggers), the floor-anchor vocabulary, the elevator shaft, and the scroll
// budget. Companion to smoke.spec.ts; runs against the PRODUCTION build.

test('a feature row retunes the studio to its demo channel', async ({ page }) => {
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  // "pantry chitchat" row → the MEETINGS channel (chitchat bubbles on screen)
  const row = page.locator('[data-feature-ch="meetings"]');
  await row.scrollIntoViewIfNeeded();
  await row.click();
  await expect(page.locator('[data-stage="meetings"]')).toBeVisible();
  await expect(page.locator('[data-stage="vibing"]')).toBeHidden();
  // the row and the dial agree on the tuned channel (one tune() path)
  await expect(row).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('button.mon[data-ch="meetings"]')).toHaveAttribute(
    'aria-pressed',
    'true'
  );
  // "coffee run" rides back to the LIVE office channel (the spec's worked example)
  const coffee = page.locator('[data-feature-ch="vibing"]').first();
  await coffee.click();
  await expect(page.locator('[data-stage="vibing"]')).toBeVisible();
  await expect(coffee).toHaveAttribute('aria-pressed', 'true');
  // the standalone Features section is GONE — merged, not duplicated
  await expect(page.locator('section.features')).toHaveCount(0);
});

test('CRT channel keys: a digit tunes the channel and does NOT ride the floor elevator', async ({
  page,
}) => {
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  // move focus INTO the studio (a dial button lives in the data-keys-scope region)
  await page.locator('button.dial__ch').first().focus();
  // channel 02 is 'openclaw' (showcase.json order) — '2' tunes it…
  await page.keyboard.press('2');
  await expect(page.locator('[data-stage="openclaw"]')).toBeVisible();
  await expect(page.locator('button.dial__ch[data-ch="openclaw"]')).toHaveAttribute(
    'aria-pressed',
    'true'
  );
  // …and the building's floor elevator did NOT jump to 2F (the scope claimed the key)
  await expect(page.locator('[data-lift-digit]')).not.toHaveText('2F');
});
