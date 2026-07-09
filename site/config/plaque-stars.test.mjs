// Unit tests for the star-plaque's display-line kernel — the offline-build
// null arm (__GH_STARS__ unreachable) has zero e2e coverage: the suite always
// builds with GH_STARS_OVERRIDE=842 (astro.config.mjs), and a vite `define`
// can't be flipped per-test. Same posture as csp-hashes.test.mjs / gh-stars.test.mjs.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { starText } from './plaque-stars.mjs';

test('a real count renders the raw untruncated number', () => {
  assert.equal(starText('842'), '★ 842');
});

test('a large count is not truncated or formatted', () => {
  assert.equal(starText('123456'), '★ 123456');
});

test('null (offline build) renders a bare star: no "null"/"undefined" leak, no trailing space', () => {
  const out = starText(null);
  assert.equal(out, '★');
  assert.ok(!out.includes('null'));
  assert.ok(!out.includes('undefined'));
  assert.equal(out, out.trim());
});
