// Unit tests for the build-time star-count kernel: the offline/failure paths
// MUST yield null (an offline `astro build` can never fail on this), and a
// reachable API yields the raw count as a string. Same posture as
// csp-hashes.test.mjs: the kernel is pure-ish and injectable, the config owns
// only the wiring.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import process from 'node:process';
import { fetchStarCount } from './gh-stars.mjs';

const ok = (count) =>
  /** @type {any} */ ({ ok: true, json: async () => ({ stargazers_count: count }) });

test('a reachable API yields the raw count as a string', async () => {
  assert.equal(await fetchStarCount(async () => ok(342), undefined), '342');
});

test('a non-ok response yields null (offline build must not fail)', async () => {
  const notFound = /** @type {any} */ ({ ok: false, json: async () => ({}) });
  assert.equal(await fetchStarCount(async () => notFound, undefined), null);
});

test('a thrown fetch (offline / timeout) yields null', async () => {
  assert.equal(
    await fetchStarCount(async () => {
      throw new Error('offline');
    }, undefined),
    null
  );
});

test('a malformed body yields null, not "undefined"', async () => {
  const weird = /** @type {any} */ ({ ok: true, json: async () => ({}) });
  assert.equal(await fetchStarCount(async () => weird, undefined), null);
});

test('a provided token rides the authorization header', async () => {
  let seen;
  await fetchStarCount(async (_url, init) => {
    seen = init.headers.authorization;
    return ok(1);
  }, 'tok');
  assert.equal(seen, 'Bearer tok');
});

test('the fetch is bounded by an AbortSignal (the offline-build timeout)', async () => {
  let seenSignal;
  await fetchStarCount(async (_url, init) => {
    seenSignal = init.signal;
    return ok(1);
  }, undefined);
  assert.ok(seenSignal instanceof AbortSignal);
});

test('GH_STARS_OVERRIDE short-circuits the fetch and returns it verbatim', async () => {
  const prev = process.env.GH_STARS_OVERRIDE;
  process.env.GH_STARS_OVERRIDE = '842';
  try {
    let called = false;
    const count = await fetchStarCount(async () => {
      called = true;
      return ok(1);
    }, undefined);
    assert.equal(count, '842');
    assert.equal(called, false);
  } finally {
    if (prev === undefined) delete process.env.GH_STARS_OVERRIDE;
    else process.env.GH_STARS_OVERRIDE = prev;
  }
});

test('a set-but-empty GH_STARS_OVERRIDE behaves as unset (degenerate-env rule)', async () => {
  const prev = process.env.GH_STARS_OVERRIDE;
  process.env.GH_STARS_OVERRIDE = '';
  try {
    assert.equal(await fetchStarCount(async () => ok(7), undefined), '7');
  } finally {
    if (prev === undefined) delete process.env.GH_STARS_OVERRIDE;
    else process.env.GH_STARS_OVERRIDE = prev;
  }
});
