import assert from 'node:assert/strict';
import test from 'node:test';

import { aggregate, evaluateAssertions, median } from '../scripts/lighthouse-runner.mjs';

const lhr = ({ perf = 0.9, contrast = 1, lcp = 2000, mark = 5000 } = {}) => ({
  categories: { performance: { score: perf } },
  audits: {
    'color-contrast': { score: contrast },
    'largest-contentful-paint': { score: 0.5, numericValue: lcp },
    'user-timings': {
      details: {
        items: [
          { name: 'pixtuoid-revealed', startTime: mark },
          { name: 'measured-span', startTime: 1, duration: 42 },
        ],
      },
    },
  },
});

test('median handles odd and even sample counts', () => {
  assert.equal(median([3, 1, 2]), 2);
  assert.equal(median([4, 1, 2, 3]), 2.5);
});

test('pessimistic is the worst run per kind: min score, max numeric', () => {
  assert.equal(aggregate([0.8, 0.95], 'pessimistic', 'score'), 0.8);
  assert.equal(aggregate([1000, 3000], 'pessimistic', 'numeric'), 3000);
  assert.equal(aggregate([0.8, 0.95], 'optimistic', 'score'), 0.95);
  assert.equal(aggregate([1000, 3000], 'optimistic', 'numeric'), 1000);
  assert.throws(() => aggregate([1], 'typo', 'score'), /unknown aggregationMethod/);
});

test('minScore and maxNumericValue fail in the right direction', () => {
  const assertions = {
    'categories:performance': ['error', { minScore: 0.7, aggregationMethod: 'median' }],
    'largest-contentful-paint': ['error', { maxNumericValue: 9000, aggregationMethod: 'median' }],
  };
  assert.deepEqual(evaluateAssertions(assertions, 'u', [lhr(), lhr(), lhr()]), []);
  const bad = evaluateAssertions(assertions, 'u', [
    lhr({ perf: 0.5, lcp: 20000 }),
    lhr({ perf: 0.5, lcp: 20000 }),
    lhr({ perf: 0.9, lcp: 2000 }),
  ]);
  assert.equal(bad.length, 2, JSON.stringify(bad));
  assert.match(bad[0].error, /violates minScore 0\.7/);
  assert.match(bad[1].error, /violates maxNumericValue 9000/);
});

test('default aggregation is median — the documented LHCI divergence', () => {
  // One bad run out of three: median passes, pessimistic would fail.
  const runs = [lhr({ perf: 0.9 }), lhr({ perf: 0.9 }), lhr({ perf: 0.1 })];
  const noMethod = { 'categories:performance': ['error', { minScore: 0.7 }] };
  assert.deepEqual(evaluateAssertions(noMethod, 'u', runs), []);
  const pessimistic = {
    'categories:performance': ['error', { minScore: 0.7, aggregationMethod: 'pessimistic' }],
  };
  assert.equal(evaluateAssertions(pessimistic, 'u', runs).length, 1);
});

test('user-timings resolve a mark by startTime and a measure by duration', () => {
  const assertions = {
    'user-timings:pixtuoid-revealed': [
      'error',
      { maxNumericValue: 6500, aggregationMethod: 'pessimistic' },
    ],
    'user-timings:measured-span': [
      'error',
      { maxNumericValue: 50, aggregationMethod: 'pessimistic' },
    ],
  };
  assert.deepEqual(evaluateAssertions(assertions, 'u', [lhr({ mark: 6000 })]), []);
  assert.equal(evaluateAssertions(assertions, 'u', [lhr({ mark: 7000 })]).length, 1);
});

test('a missing audit, category, or timing FAILS — never a vacuous pass', () => {
  for (const key of ['categories:nope', 'not-an-audit', 'user-timings:renamed-mark']) {
    const failures = evaluateAssertions({ [key]: ['error', { minScore: 1 }] }, 'u', [lhr()]);
    assert.equal(failures.length, 1, key);
    assert.match(failures[0].error, /missing/, key);
  }
});

test('an assertion with neither bound is a config error, not a skip', () => {
  assert.throws(
    () => evaluateAssertions({ x: ['error', { aggregationMethod: 'median' }] }, 'u', [lhr()]),
    /neither minScore nor maxNumericValue/
  );
});
