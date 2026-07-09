// The star plaque's display line, extracted so the offline-build null arm
// (__GH_STARS__ unreachable, see gh-stars.mjs) is unit-testable — the e2e
// suite always builds with GH_STARS_OVERRIDE set (astro.config.mjs), and a
// vite `define` can't be flipped per-test, so nothing else exercises it.

/**
 * @param {string | null} stars the `__GH_STARS__` build-time value
 * @returns {string} "★ <count>" for a real count, or bare "★" when null
 */
export function starText(stars) {
  return `★ ${stars ?? ''}`.trim();
}
