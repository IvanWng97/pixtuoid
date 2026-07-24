import { expect, test, type Page } from '@playwright/test';

declare global {
  interface Window {
    __layoutShifts: LayoutShift[];
  }
}

interface LayoutShift {
  startTime: number;
  value: number;
}

const CLS_BUDGET = 0.1;
const CLS_SESSION_GAP_MS = 1_000;
const CLS_SESSION_WINDOW_MS = 5_000;
const FONT_DELAY_MS = 500;
const LIGHTHOUSE_VIEWPORT = { width: 412, height: 823 };

async function observeLayoutShifts(page: Page): Promise<void> {
  await page.addInitScript(() => {
    window.__layoutShifts = [];
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries() as Array<
        PerformanceEntry & { hadRecentInput: boolean; value: number }
      >) {
        if (!entry.hadRecentInput) {
          window.__layoutShifts.push({ startTime: entry.startTime, value: entry.value });
        }
      }
    }).observe({ type: 'layout-shift', buffered: true });
  });
}

function maxLayoutShiftSession(entries: LayoutShift[]): number {
  let maximum = 0;
  let session = 0;
  let sessionStart: number | undefined;
  let previous: number | undefined;

  for (const entry of entries) {
    if (
      sessionStart === undefined ||
      previous === undefined ||
      entry.startTime - previous >= CLS_SESSION_GAP_MS ||
      entry.startTime - sessionStart >= CLS_SESSION_WINDOW_MS
    ) {
      session = 0;
      sessionStart = entry.startTime;
    }
    session += entry.value;
    maximum = Math.max(maximum, session);
    previous = entry.startTime;
  }
  return maximum;
}

test('CLS sessions start at the first shift and split at canonical boundaries', () => {
  expect(
    maxLayoutShiftSession([
      { startTime: 500, value: 0.01 },
      { startTime: 1_400, value: 0.01 },
      { startTime: 2_300, value: 0.01 },
      { startTime: 3_200, value: 0.01 },
      { startTime: 4_100, value: 0.01 },
      { startTime: 5_099, value: 0.01 },
    ])
  ).toBeCloseTo(0.06);
  expect(
    maxLayoutShiftSession([
      { startTime: 500, value: 0.08 },
      { startTime: 1_500, value: 0.08 },
    ])
  ).toBeCloseTo(0.08);
  expect(
    maxLayoutShiftSession([
      { startTime: 500, value: 0.02 },
      { startTime: 1_400, value: 0.02 },
      { startTime: 2_300, value: 0.02 },
      { startTime: 3_200, value: 0.02 },
      { startTime: 4_100, value: 0.02 },
      { startTime: 5_000, value: 0.02 },
      { startTime: 5_500, value: 0.07 },
    ])
  ).toBeCloseTo(0.12);
});

for (const route of ['architecture/', 'parallel-delivery/']) {
  test(`${route} stays stable when web fonts miss first paint`, async ({ page }) => {
    await page.setViewportSize(LIGHTHOUSE_VIEWPORT);
    await page.route(`**/${route}`, async (request) => {
      const response = await request.fetch();
      const body = await response.text();
      await request.fulfill({
        response,
        body: body.replace(
          '</head>',
          "<style>:root{--font-body:'Lora','Courier New',monospace!important;--font-mono:'Monaspace Neon',Arial,sans-serif!important}</style></head>"
        ),
      });
    });
    await page.route('**/*.woff2', async (request) => {
      await new Promise((resolve) => setTimeout(resolve, FONT_DELAY_MS));
      await request.continue();
    });
    await observeLayoutShifts(page);

    await page.goto(`./${route}`);
    await page.waitForLoadState('networkidle');

    expect(
      maxLayoutShiftSession(await page.evaluate(() => window.__layoutShifts))
    ).toBeLessThanOrEqual(CLS_BUDGET);
  });
}
