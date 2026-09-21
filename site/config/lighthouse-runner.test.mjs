import assert from 'node:assert/strict';
import { Buffer } from 'node:buffer';
import http from 'node:http';
import test from 'node:test';
import { gunzipSync, gzipSync } from 'node:zlib';

import {
  aggregate,
  evaluateAssertions,
  median,
  startPagesLikeProxy,
} from './lighthouse-runner.mjs';

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
  // And the default is median, not optimistic: one GOOD run out of three must
  // not rescue the assertion (median 0.5 fails where optimistic 0.9 passes) —
  // the exact regression direction the divergence exists to prevent.
  const mostlyBad = [lhr({ perf: 0.9 }), lhr({ perf: 0.5 }), lhr({ perf: 0.5 })];
  assert.equal(evaluateAssertions(noMethod, 'u', mostlyBad).length, 1);
});

test('user-timings resolve a mark by startTime and a measure by duration', () => {
  const assertions = {
    'user-timings:pixtuoid-revealed': [
      'error',
      { maxNumericValue: 6500, aggregationMethod: 'pessimistic' },
    ],
  };
  assert.deepEqual(evaluateAssertions(assertions, 'u', [lhr({ mark: 6000 })]), []);
  assert.equal(evaluateAssertions(assertions, 'u', [lhr({ mark: 7000 })]).length, 1);
  // The measure fixture has startTime 1 and duration 42: a 10 bound must fail
  // via the DURATION read — a swapped `startTime ?? duration` fallback would
  // read 1 and green it.
  const measure = { 'user-timings:measured-span': ['error', { maxNumericValue: 10 }] };
  assert.equal(evaluateAssertions(measure, 'u', [lhr()]).length, 1);
  const loose = { 'user-timings:measured-span': ['error', { maxNumericValue: 50 }] };
  assert.deepEqual(evaluateAssertions(loose, 'u', [lhr()]), []);
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

// ---- the Pages-like proxy ---------------------------------------------------

const WASM = Buffer.alloc(64 * 1024, 'pixtuoid');
const PNG = Buffer.from('not really a png, but never compressible by contract');
const JS_GZ = gzipSync(Buffer.from('export const already = "gzipped by the preview server";'));

function listen(server) {
  return new Promise((resolve) => server.listen(0, 'localhost', () => resolve(server)));
}

function fakePreview() {
  return listen(
    http.createServer((req, res) => {
      if (req.url === '/wasm/engine.wasm') {
        res.writeHead(200, { 'content-type': 'application/wasm', 'content-length': WASM.length });
        res.end(WASM);
      } else if (req.url === '/demo.png') {
        res.writeHead(200, { 'content-type': 'image/png', 'content-length': PNG.length });
        res.end(PNG);
      } else if (req.url === '/page.js') {
        res.writeHead(200, { 'content-type': 'text/javascript', 'content-encoding': 'gzip' });
        res.end(JS_GZ);
      } else {
        res.writeHead(404, { 'content-type': 'text/plain' });
        res.end('nope');
      }
    })
  );
}

// Raw http, not fetch: fetch inflates the body and we are measuring the WIRE.
function wire(port, path, headers = {}) {
  return new Promise((resolve, reject) => {
    http
      .get({ host: 'localhost', port, path, headers }, (res) => {
        const chunks = [];
        res.on('data', (c) => chunks.push(c));
        res.on('end', () =>
          resolve({ status: res.statusCode, headers: res.headers, body: Buffer.concat(chunks) })
        );
      })
      .on('error', reject);
  });
}

async function withProxy(fn) {
  const upstream = await fakePreview();
  const proxy = await startPagesLikeProxy({ upstreamPort: upstream.address().port, port: 0 });
  try {
    await fn(proxy.address().port);
  } finally {
    proxy.close();
    upstream.close();
  }
}

test('the proxy gzips wasm the way GitHub Pages does, and only when the client accepts it', async () => {
  await withProxy(async (port) => {
    const gz = await wire(port, '/wasm/engine.wasm', { 'accept-encoding': 'gzip, deflate, br' });
    assert.equal(gz.status, 200);
    assert.equal(gz.headers['content-encoding'], 'gzip');
    assert.equal(gz.headers['content-type'], 'application/wasm');
    assert.equal(gz.headers['content-length'], undefined, 'the old length would truncate the body');
    assert.ok(gz.body.length < WASM.length / 10, `wire ${gz.body.length} B`);
    assert.deepEqual(gunzipSync(gz.body), WASM);

    const plain = await wire(port, '/wasm/engine.wasm');
    assert.equal(plain.headers['content-encoding'], undefined);
    assert.deepEqual(plain.body, WASM);

    // Pages answers a br-only client with the raw bytes (fetched 2026-09-20).
    const brOnly = await wire(port, '/wasm/engine.wasm', { 'accept-encoding': 'br' });
    assert.equal(brOnly.headers['content-encoding'], undefined);
    assert.deepEqual(brOnly.body, WASM);
  });
});

test('the proxy passes everything else through untouched', async () => {
  await withProxy(async (port) => {
    const png = await wire(port, '/demo.png', { 'accept-encoding': 'gzip' });
    assert.equal(png.headers['content-encoding'], undefined);
    assert.equal(png.headers['content-length'], String(PNG.length));
    assert.deepEqual(png.body, PNG);

    const js = await wire(port, '/page.js', { 'accept-encoding': 'gzip' });
    assert.equal(js.headers['content-encoding'], 'gzip');
    assert.deepEqual(js.body, JS_GZ, 'never double-encoded');

    const missing = await wire(port, '/nope', { 'accept-encoding': 'gzip' });
    assert.equal(missing.status, 404);
    assert.equal(missing.body.toString(), 'nope');
  });
});
