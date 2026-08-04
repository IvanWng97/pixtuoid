// Extracted so the offline-build null arm is unit-testable: a vite `define` can't
// be flipped per-test, and the e2e suite always builds with GH_STARS_OVERRIDE set.

/**
 * @param {string | null} stars the `__GH_STARS__` build-time value
 * @returns {string}
 */
export function starText(stars) {
  return `★ ${stars ?? ''}`.trim();
}
