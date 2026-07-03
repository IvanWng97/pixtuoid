// Unit tests for the CSP hash kernel — the edge cases the e2e watchdog can only
// catch indirectly (and only on pages the smoke suite exercises). `node --test`
// runs .mjs natively; wired into `npm run verify` (→ `just site-check`).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { inlineScriptHashes, rewriteCspMeta } from './csp-hashes.mjs';

const sha = (s) => `'sha256-${createHash('sha256').update(s, 'utf8').digest('base64')}'`;

test('hashes inline script content verbatim', () => {
  assert.ok(inlineScriptHashes('<script>doWork()</script>').has(sha('doWork()')));
});

test('a > inside a quoted attribute value does not truncate the content', () => {
  const h = inlineScriptHashes('<script is:inline data-note="a>b">doWork()</script>');
  assert.ok(h.has(sha('doWork()')), 'must hash the real content, not b">doWork()');
  assert.ok(!h.has(sha('b">doWork()')));
});

test('parser-error end tags a browser still honors match (js/bad-tag-filter)', () => {
  // A browser ends the script at each of these; if our regex didn't, the
  // trailing content would ship unhashed → prod-only CSP block.
  assert.ok(inlineScriptHashes('<script>doWork()</script >').has(sha('doWork()')));
  assert.ok(inlineScriptHashes('<script>doWork()</script\n>').has(sha('doWork()')));
  assert.ok(inlineScriptHashes('<script>doWork()</script foo="bar">').has(sha('doWork()')));
  assert.ok(inlineScriptHashes('<script>doWork()</script\t\n bar>').has(sha('doWork()')));
});

test('src= inside another attribute value is not treated as a real src', () => {
  const h = inlineScriptHashes('<script data-cmd="ffmpeg src=in.mp4">go()</script>');
  assert.ok(h.has(sha('go()')), 'the inline script must still be hashed');
});

test('a real src attribute skips the script (external, rides self)', () => {
  assert.equal(inlineScriptHashes('<script src="/app.js"></script>').size, 0);
});

test('data-src is not mistaken for src', () => {
  assert.ok(inlineScriptHashes('<script data-src="x">run()</script>').has(sha('run()')));
});

test('rewriteCspMeta injects script hashes and strips style hashes', () => {
  const html =
    '<meta http-equiv="content-security-policy" content="script-src \'self\'; ' +
    "style-src 'self' 'unsafe-inline' 'sha256-OLD'\">" +
    '<script>x()</script>';
  const out = rewriteCspMeta(html);
  assert.ok(out.includes(sha('x()')), 'script-src gains the inline hash');
  assert.ok(!out.includes("'sha256-OLD'"), 'style-src hashes are dropped');
  assert.ok(out.includes("style-src 'self' 'unsafe-inline'"), "'unsafe-inline' survives hash-free");
});

test('rewriteCspMeta returns null when no CSP meta is present', () => {
  assert.equal(rewriteCspMeta('<html></html>'), null);
});
