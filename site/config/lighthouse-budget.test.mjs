import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const config = JSON.parse(
  readFileSync(new URL('../lighthouserc.json', import.meta.url), 'utf8')
).ci;

test('performance budgets use stable samples and hard failures', () => {
  assert.ok(config.collect.numberOfRuns >= 3);
  for (const [audit, assertion] of Object.entries(config.assert.assertions)) {
    assert.equal(assertion[0], 'error', `${audit} must fail CI`);
  }
  assert.equal(config.assert.assertions['categories:performance'][1].aggregationMethod, 'median');
});

test('the first-visit reveal has a hard upper bound', () => {
  assert.deepEqual(config.assert.assertions['user-timings:pixtuoid-revealed'], [
    'error',
    { maxNumericValue: 6500, aggregationMethod: 'pessimistic' },
  ]);
});
