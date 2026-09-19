import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const siteRoot = new URL('../', import.meta.url);
const manifest = JSON.parse(readFileSync(new URL('package.json', siteRoot), 'utf8'));
const lock = JSON.parse(readFileSync(new URL('package-lock.json', siteRoot), 'utf8'));
const npmrc = new Set(
  readFileSync(new URL('.npmrc', siteRoot), 'utf8').split(/\r?\n/).filter(Boolean)
);

test('the install-script policy runs on the pinned npm generation', () => {
  assert.equal(manifest.packageManager, 'npm@12.0.1');
  assert.equal(manifest.engines.npm, '>=12.0.0 <13');
  assert.ok(npmrc.has('engine-strict=true'));
  assert.ok(npmrc.has('strict-allow-scripts=true'));
});

test('install-script permissions stay at least privilege', () => {
  assert.deepEqual(manifest.allowScripts, {
    'esbuild@0.28.1': true,
    fsevents: false,
  });
});

// The direct `mermaid` pin (site/knip.jsonc) only renders anything while it is the
// copy mermaid-isomorphic resolves: two copies, or a held major beside a moved
// range, keep every other gate green while the SVG comes from the other one.
test('one mermaid: the direct pin tracks mermaid-isomorphic (dependabot ignore row)', () => {
  const copies = Object.keys(lock.packages).filter((k) => k.endsWith('node_modules/mermaid'));
  assert.deepEqual(copies, ['node_modules/mermaid']);
  const pinned = lock.packages['node_modules/mermaid'].version.split('.')[0];
  const wanted =
    lock.packages['node_modules/mermaid-isomorphic'].dependencies.mermaid.match(/\d+/)[0];
  assert.equal(
    pinned,
    wanted,
    'mermaid-isomorphic moved: lift the mermaid ignore in .github/dependabot.yml'
  );
});
