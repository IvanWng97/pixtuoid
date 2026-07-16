import { test } from 'node:test';
import assert from 'node:assert/strict';

import { resolveDisplayedVersion } from './released-version.mjs';

test('a release tag wins over the cargo version (the mid-cycle-bump gap)', () => {
  // main bumped to 0.16.0 but the latest RELEASE is v0.15.0 — show 0.15.0
  assert.deepEqual(resolveDisplayedVersion('v0.15.0', '0.16.0'), {
    version: '0.15.0',
    source: 'tag',
  });
});

test('at the release tag itself the two agree', () => {
  assert.deepEqual(resolveDisplayedVersion('v0.16.0', '0.16.0'), {
    version: '0.16.0',
    source: 'tag',
  });
});

test('missing tag history falls back to Cargo.toml', () => {
  assert.deepEqual(resolveDisplayedVersion(null, '0.16.0'), {
    version: '0.16.0',
    source: 'cargo-toml',
  });
});

test('a malformed or non-release tag falls back rather than shipping garbage', () => {
  for (const bad of ['nightly', 'v1.2', 'v1.2.3-rc1', '0.15.0', 'v0.15.0-9-gabc123', '']) {
    assert.equal(resolveDisplayedVersion(bad, '0.16.0').source, 'cargo-toml', bad);
  }
});

test('surrounding whitespace from the git call is tolerated', () => {
  assert.equal(resolveDisplayedVersion('v0.15.0\n', '0.16.0').version, '0.15.0');
});
