import { expect, test } from '@playwright/test';
import featuresData from '../../src/features.json' with { type: 'json' };
import sourcesData from '../../src/sources.json' with { type: 'json' };

// wb-3's runtime contracts: the merged 5F band (the dial is the ONE channel
// switcher; the feature roster below the stage is quiet, non-interactive
// content), the floor-anchor vocabulary, the elevator shaft, and the scroll
// budget. Companion to smoke.spec.ts; runs against the PRODUCTION build.

// features.json is the total collection, partitioned by `channel` (consts.ts)
// — read it directly so these specs can't drift from the manifest they're
// pinning (a hand-copied expected string would silently go stale).
type Feature = { name: string; desc: string; channel?: string };
const features = featuresData as Feature[];
const descByChannel = new Map(features.filter((f) => f.channel).map((f) => [f.channel!, f.desc]));

test('cold load: the dial marks the default channel pressed, accordion shows its desc', async ({
  page,
}) => {
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  // the default channel is 'vibing' (src/showcase.json's default:true) — the
  // dial is the ONE interactive switcher now (the feature roster below the
  // stage no longer mirrors channel state; it never did anything but tune).
  await expect(page.locator('button.mon[data-ch="vibing"]')).toHaveAttribute(
    'aria-pressed',
    'true'
  );
  // the accordion body is server-rendered with the default channel's joined
  // features.json desc — no JS/click required to see it.
  await expect(page.locator('#dial-desc')).toHaveText(descByChannel.get('vibing')!);
});

test('dial accordion: clicking a channel reveals its features.json desc under the dial', async ({
  page,
}) => {
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  const btn = page.locator('button.mon[data-ch="openclaw"]');
  await expect(btn).toHaveAttribute('aria-expanded', 'false');
  await btn.click();
  await expect(btn).toHaveAttribute('aria-expanded', 'true');
  // exactly one entry is expanded at a time (single-open accordion)
  await expect(page.locator('button.mon[data-ch="vibing"]')).toHaveAttribute(
    'aria-expanded',
    'false'
  );
  await expect(page.locator('#dial-desc')).toHaveText(descByChannel.get('openclaw')!);
});

test('the below-stage roster is exactly the features WITHOUT a channel', async ({ page }) => {
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  // wb-3: the per-row "tune in →" link (and its half-fabricated channel
  // mapping — Coffee run → vibing, monitor glow → spaces) is retired. A
  // stays-dead pin: no channel-tune trigger or #showcase-<id> anchor is left
  // anywhere in the roster.
  await expect(page.locator('#showcase [data-feature-ch]')).toHaveCount(0);
  await expect(page.locator('#showcase button.roster__row')).toHaveCount(0);
  await expect(page.locator('#showcase a[href^="#showcase-"]')).toHaveCount(0);
  // the wb-3-plus-wb-3.1 partition: every channel-carrying feature tunes the
  // dial instead, so the roster is exactly (and in order) the complement.
  const expectedNames = features.filter((f) => !f.channel).map((f) => f.name);
  expect(expectedNames.length).toBeGreaterThan(0);
  const renderedNames = await page.locator('#showcase .roster__name').allTextContents();
  expect(renderedNames).toEqual(expectedNames);
});

test('the feature roster stays a quiet, non-interactive grid — the dial is the switcher', async ({
  page,
}) => {
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  // a dial click still retunes the studio (the switcher survives the roster's
  // demotion) — "meetings" is the chitchat-bubble channel.
  const btn = page.locator('button.mon[data-ch="meetings"]');
  await btn.scrollIntoViewIfNeeded();
  await btn.click();
  await expect(page.locator('[data-stage="meetings"]')).toBeVisible();
  await expect(page.locator('[data-stage="vibing"]')).toBeHidden();
  await expect(btn).toHaveAttribute('aria-pressed', 'true');
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
    { fl: '6F', label: 'penthouse — welcome', id: 'lobby' },
    { fl: '5F', label: 'studio — demos', id: 'showcase' },
    { fl: '4F', label: 'amenities — see it real', id: 'amenities' },
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
  const carY = () =>
    page.evaluate(
      () =>
        new DOMMatrix(getComputedStyle(document.querySelector('[data-shaft-car]')!).transform).m42
    );
  // baseline BEFORE riding — 6F's resting Y is already > 0 (the top stop isn't
  // pinned at the rail's own y=0), so "> 0" after the ride would pass even if
  // the car never moved; record it and assert the DELTA instead
  const preY = await carY();
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
  // the car rode down the rail by a MEANINGFUL amount — more than half of one
  // inter-stop gap (the rail's 6 stops are evenly spaced top-to-bottom, so
  // that's the actual on-page geometry, not an arbitrary literal): riding all
  // the way from 6F to 1F must cover several gaps, so half of one is a floor
  // well under what a real ride produces but well over layout jitter/rounding
  const gap = await page.evaluate(() => {
    const stops = Array.from(document.querySelectorAll<HTMLElement>('[data-shaft-stop]'));
    return (stops[stops.length - 1].offsetTop - stops[0].offsetTop) / (stops.length - 1);
  });
  const postY = await carY();
  expect(postY - preY).toBeGreaterThan(gap / 2);
});

test('elevator shaft: the current-floor LED dot is actually lit, not just text-colored', async ({
  page,
}) => {
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  // `.led-selected` (global.css) only recolors TEXT — the fix wires the dot's
  // own background to the SAME --led token. Read the token straight off the
  // root and compare it to the dot's rendered background so this proves the
  // dot is really lit, not merely that a class name got toggled. Both are
  // resolved by the SAME browser engine (a probe styled with `var(--led)`),
  // so the comparison never depends on hex vs. rgb() string formatting.
  const litColor = await page.evaluate(() => {
    const probe = document.createElement('span');
    probe.style.background = 'var(--led)';
    document.body.appendChild(probe);
    const c = getComputedStyle(probe).backgroundColor;
    probe.remove();
    return c;
  });
  const dotColor = (fl: string) =>
    page
      .locator(`[data-shaft-stop="${fl}"] .led-dot`)
      .evaluate((el) => getComputedStyle(el).backgroundColor);
  // 6F is current on load: its dot is lit, a non-current dot (5F) is not
  await expect(page.locator('[data-shaft-stop="6F"]')).toHaveAttribute('aria-current', 'true');
  expect(await dotColor('6F')).toBe(litColor);
  expect(await dotColor('5F')).not.toBe(litColor);
  // ride to 3F: the lit dot follows
  await page.locator('[data-shaft-stop="3F"]').click();
  await expect(page.locator('[data-shaft-stop="3F"]')).toHaveAttribute('aria-current', 'true', {
    timeout: 10_000,
  });
  expect(await dotColor('3F')).toBe(litColor);
  expect(await dotColor('6F')).not.toBe(litColor);
});

test('elevator shaft: every floor is reachable and reads current on BOTH the shaft LED and the statusline digit', async ({
  page,
}) => {
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  // the settle-drive: click each stop in turn, wait for its aria-current to
  // land, then let the IntersectionObserver + rAF ride settle before reading
  // state — a floor whose section is too short to ever own the floor-spy
  // center band (e.g. 4F's pre-fix amenities shell) would settle on a
  // DIFFERENT floor here instead of its own
  for (const fl of ['6F', '5F', '4F', '3F', '2F', '1F']) {
    await page.locator(`[data-shaft-stop="${fl}"]`).click();
    await expect(page.locator(`[data-shaft-stop="${fl}"]`)).toHaveAttribute(
      'aria-current',
      'true',
      {
        timeout: 10_000,
      }
    );
    await expect
      .poll(() =>
        page.evaluate((f) => {
          const r = document.querySelector(`[data-floor="${f}"]`)!.getBoundingClientRect();
          return r.top < window.innerHeight * 0.55 && r.bottom > window.innerHeight * 0.45;
        }, fl)
      )
      .toBe(true);
    await expect(page.locator('[data-shaft-stop][aria-current="true"]')).toHaveCount(1);
    await expect(page.locator('[data-lift-digit]')).toHaveText(fl);
  }

  // Bottom-clamp agreement: 1F + the footer rarely fill the center band
  // (FLOOR_SPY_ROOT_MARGIN, consts.ts) the observer above keys off, so at
  // the TRUE scroll max both readouts must clamp to the last floor (1F)
  // regardless of what the observer last reported — the statusline's clamp
  // and the shaft's mirror of it (ElevatorShaft.astro) read the identical
  // epsilon, so they can't disagree at the page bottom either.
  await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
  await expect(page.locator('[data-shaft-stop="1F"]')).toHaveAttribute('aria-current', 'true', {
    timeout: 10_000,
  });
  await expect(page.locator('[data-shaft-stop][aria-current="true"]')).toHaveCount(1);
  await expect(page.locator('[data-lift-digit]')).toHaveText('1F');
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

test('scroll budget: the page fits its roster-aware viewport-height budget at 1440×900', async ({
  browser,
}) => {
  // The spec's original compression target (§4) was 6.5vh — a plan-authoring
  // proxy that turned out to bake in assumptions three LOCKED design
  // decisions invalidate: hold #1 stays full-viewport, the hero stays
  // 100svh, and 4F needs real height to own the floor-spy band — those
  // decisions outrank the number, not the other way around. Measured proof:
  // even fully flattening every tightenable lever (section padding-block → 0,
  // .section-head margin → 0, the footer's own margin-top → 0, AND removing
  // the closer hold entirely) only reaches 6.519vh — 6.5 is mathematically
  // unreachable without shrinking real content (the compatibility table, the
  // feature roster, the demo wall), which would trade a page-shape proxy for
  // actual information loss.
  //
  // The pin moved up a deliberate notch for Task 4: wb-4's ProofSplit
  // replaced the bare `.amenities` eyebrow shell (which needed an interim
  // 60vh min-height just to reach the floor-spy band) with a real replay
  // stage + coda — taller than that interim floor by construction, so 4F
  // stays reachable for free and the old min-height hack is gone. 8.4 = the
  // measured 8.133vh (same tightened section padding-block clamp,
  // .section-head margin, closer-hold min-height, and footer margin-top as
  // before, now plus ProofSplit's own clamp()-bounded stage) plus ~0.27vh of
  // headroom: tight enough to still catch a future section ballooning,
  // honest about where the page actually sits today.
  //
  // Task 7 moved the pin again: PantryFaq mounts a second block inside
  // section#amenities, after ProofSplit — a still image + a 2-turn chitchat
  // FAQ, genuinely new content, not padding. Before implementing, this same
  // build measured 8.113vh (unchanged from Task 6). The FIRST draft used a
  // symmetric padding-block on .pantry (measured 8.601vh) — redundant with
  // ProofSplit's OWN bottom padding closing the same gap, so it was cut to a
  // single margin-top reusing .section-head's existing "adjacent chunk in
  // one section" scale (8.494vh), confirmed via getBoundingClientRect that
  // the remaining added height is genuinely the still image (294.5px, the
  // taller of its two flex columns) + that margin (28.8px) — no further
  // padding to cut without shrinking the image itself. Final measured:
  // 8.472vh. 8.6 = that measured value plus ~0.13vh of headroom — tighter
  // in absolute terms than Task 4's 0.27vh margin, but this floor's content
  // is now real (an image + real copy, not a placeholder), so a future
  // regression here is a real ballooning, not slack being eaten.
  //
  // The grok source (12th supported CLI, PR #640) moved the pin once more:
  // sources.json grew a row, which is one more tools-table row plus one more
  // hero badge chip — per-source content the page exists to show, not
  // padding. Measured after the row landed: 8.637vh (was 8.472 at 11
  // sources — ~0.165vh per the row + the chip row's wrap). 8.75 = that
  // measured value plus ~0.11vh of headroom, the same order as Task 7's
  // margin; the pin keeps catching section ballooning while scaling with
  // the source roster it deliberately tracks.
  //
  // The kimi source (13th supported CLI, PR #692) forced the pin's THIRD
  // roster-driven bump (8.75 → 8.9) and ended the treadmill: the hero badge
  // row is now the O(1) collapsed code strip (Hero.astro — codes-only cells,
  // hover-expand), so a new source adds ONLY its tools-table row. The budget
  // is therefore ROSTER-AWARE — BASE + N×PER_SOURCE, N from the same
  // sources.json the page renders — so a source-add never touches this test
  // again, while non-roster ballooning (a section growing, a regression
  // re-introducing per-source hero height) still trips it. Measured at the
  // strip's landing: 8.811vh at N=13, and 8.883 at a synthetic N=14 → a
  // 0.072vh/source slope (the table row alone). PER_SOURCE = 0.075 (the
  // measured slope + rounding slack); BASE = 7.95 ≈ 8.811 − 13×0.075 plus
  // ~0.11vh headroom, the same order as the historical bumps' margins.
  // Caveat: the strip is O(1) only while its codes fit ONE line at 1440
  // (~20 sources); past that it wraps a step the linear PER_SOURCE doesn't
  // model — whoever adds the ~20th source should re-measure BASE.
  const SCROLL_BUDGET_BASE_VH = 7.95;
  const SCROLL_BUDGET_PER_SOURCE_VH = 0.075;
  const supported = sourcesData.filter((s) => s.status === 'supported').length;
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await ctx.newPage();
  await page.addInitScript(() => sessionStorage.setItem('pix-booted', '1'));
  await page.goto('./');
  await page.waitForLoadState('networkidle');
  const vh = await page.evaluate(() => document.documentElement.scrollHeight / window.innerHeight);
  expect(vh).toBeLessThanOrEqual(SCROLL_BUDGET_BASE_VH + supported * SCROLL_BUDGET_PER_SOURCE_VH);
  await ctx.close();
});
