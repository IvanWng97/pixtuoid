import { defineConfig, devices } from '@playwright/test';

// The e2e smoke suite runs against the PRODUCTION build via `astro preview`
// (the official Astro testing posture: "run your tests against your production
// code" — the dev server's Vite cache has twice masqueraded as a site bug in
// this repo). `dist/` must exist: `just site-e2e` builds first; in CI the
// build step precedes the e2e step. Port 4321 is astro's default for dev AND
// preview — a live dev server squatting it makes the webServer spawn fail
// LOUD (reuseExistingServer stays false below), never quietly test dev bytes.
export default defineConfig({
  testDir: './tests/e2e',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? 'github' : 'list',
  timeout: 45_000,
  use: {
    baseURL: 'http://localhost:4321/pixtuoid/',
    trace: 'on-first-retry',
  },
  projects: [
    // Chromium only: it's the browser CI already installs (rehype-mermaid),
    // and the suite gates cross-component CONTRACTS, not engine differences.
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
  ],
  webServer: {
    // The astro bin DIRECTLY — an `npm run` wrapper leaves an orphaned astro
    // child holding the port when Playwright kills the tree, and
    // reuseExistingServer would then silently test the ORPHAN'S stale build
    // (caught by this suite's own teeth test). reuse stays false for the same
    // reason: a squatted port must fail loud, never quietly test old bytes.
    command: 'node node_modules/astro/bin/astro.mjs preview --port 4321',
    url: 'http://localhost:4321/pixtuoid/',
    reuseExistingServer: false,
    timeout: 30_000,
  },
});
