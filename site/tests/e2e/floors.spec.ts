import { expect, test } from '@playwright/test';

// wb-3's runtime contracts: the merged 5F band (feature rows ARE channel
// triggers), the floor-anchor vocabulary, the elevator shaft, and the scroll
// budget. Companion to smoke.spec.ts; runs against the PRODUCTION build.

test('cold load: the roster row for the default channel starts pressed, in sync with the dial', async ({
  page,
}) => {
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  // the default channel is 'vibing' (src/showcase.json's default:true) — its
  // roster row is "coffee run" (card.href="#showcase-vibing"), never clicked
  const row = page.locator('[data-feature-ch="vibing"]').first();
  await expect(row).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('button.mon[data-ch="vibing"]')).toHaveAttribute(
    'aria-pressed',
    'true'
  );
});

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

test('the six floors declare the elevator anchor contract, top floor down', async ({ page }) => {
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  const floors = await page.$$eval('[data-floor]', (els) =>
    els.map((e) => ({
      fl: e.getAttribute('data-floor'),
      label: e.getAttribute('data-floor-label'),
      id: e.id,
    }))
  );
  expect(floors).toEqual([
    { fl: '6F', label: 'penthouse — hero', id: 'lobby' },
    { fl: '5F', label: 'studio — channels', id: 'showcase' },
    { fl: '4F', label: 'amenities — proof + pantry', id: 'amenities' },
    { fl: '3F', label: 'machine room — quickstart', id: 'how' },
    { fl: '2F', label: 'tenants — compatibility', id: 'tools' },
    { fl: '1F', label: 'front desk — install', id: 'install' },
  ]);
  // the statusline lift readout consumes the SAME fl-form values (scrollspy compat)
  await expect(page.locator('[data-lift-digit]')).toHaveText(/^\dF$/);
  // the #features anchor-compat shim lives in the merged 5F band, so inbound
  // /#features deep links still land where the feature roster now is
  const shimFloor = await page.$eval('#features', (el) =>
    el.closest('[data-floor]')?.getAttribute('data-floor')
  );
  expect(shimFloor).toBe('5F');
});

test('the floating-window still gap is retired (wb-4 owns the slot)', async ({ page }) => {
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  await expect(page.locator('[data-gap-still]')).toHaveCount(0);
  await expect(page.locator('[data-gap-daynight]')).toHaveCount(0);
  // the two KEPT holds: #1 "the real thing" (locked decision) and the closer
  await expect(page.locator('.office-gap')).toHaveCount(2);
});

test('elevator shaft: click-to-ride lands the floor, LED + lift readout agree', async ({
  page,
}) => {
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  await expect(page.locator('[data-shaft-stop]')).toHaveCount(6);
  // 6F is home: the top stop is current on load
  await expect(page.locator('[data-shaft-stop="6F"]')).toHaveAttribute('aria-current', 'true');
  // click-to-ride: 1F front desk
  await page.locator('[data-shaft-stop="1F"]').click();
  await expect(page.locator('[data-shaft-stop="1F"]')).toHaveAttribute('aria-current', 'true', {
    timeout: 10_000,
  });
  // the install section actually owns the viewport center band
  await expect
    .poll(() =>
      page.evaluate(() => {
        const r = document.querySelector('[data-floor="1F"]')!.getBoundingClientRect();
        return r.top < window.innerHeight * 0.55 && r.bottom > window.innerHeight * 0.45;
      })
    )
    .toBe(true);
  // the statusline lift and the shaft read the SAME sections — they must agree
  await expect(page.locator('[data-lift-digit]')).toHaveText('1F');
  // the car rode down the rail (transform moved off the 6F stop)
  const carY = () =>
    page.evaluate(
      () =>
        new DOMMatrix(getComputedStyle(document.querySelector('[data-shaft-car]')!).transform).m42
    );
  expect(await carY()).toBeGreaterThan(0);
});

test('elevator shaft: reduced motion is a static indicator', async ({ browser }) => {
  const ctx = await browser.newContext({ reducedMotion: 'reduce' });
  const page = await ctx.newPage();
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  // no car glide, no ding pulse — the LED indicator still tracks floors.
  // global.css's sitewide reduced-motion reset forces every element's
  // transition-duration near-zero (0.001ms, not literally 0 — kept
  // non-zero so transitionend still fires elsewhere), so assert "no
  // perceptible motion" rather than the exact forced value.
  const styles = await page.evaluate(() => {
    const car = getComputedStyle(document.querySelector('[data-shaft-car]')!);
    return { transition: car.transitionDuration };
  });
  expect(parseFloat(styles.transition)).toBeLessThan(0.001);
  await page.locator('[data-shaft-stop="3F"]').click();
  await expect(page.locator('[data-shaft-stop="3F"]')).toHaveAttribute('aria-current', 'true', {
    timeout: 10_000,
  });
  await ctx.close();
});

test('elevator shaft: the ding pulse joins the pix:paused set', async ({ page }) => {
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  // pause the page, then ride: the arrival must NOT pulse (visual motion held)
  await page.evaluate(() =>
    document.dispatchEvent(new CustomEvent('pix:paused', { detail: { paused: true } }))
  );
  await page.locator('[data-shaft-stop="1F"]').click();
  await expect(page.locator('[data-shaft-stop="1F"]')).toHaveAttribute('aria-current', 'true', {
    timeout: 10_000,
  });
  expect(
    await page.evaluate(() =>
      document.querySelector('[data-shaft-stop="1F"]')!.classList.contains('is-ding')
    )
  ).toBe(false);
});
