// The bounded wasm-init retry was once a byte-identical copy in TWO is:inline
// scripts (OfficeBackdrop's boot() + Showcase's bootCanvas()), and this test
// pinned the two copies equal. Since #721 the retry lives ONCE in the shared
// public/office-driver.js module both consumers dynamic-import, so drift is
// structurally impossible — the test now guards the single source: the consts
// live in office-driver.js AND neither component re-inlines a copy (which would
// silently reintroduce the drift class). Keeps the "one source" a fact, not a
// hope (the repo convention: a cross-boundary invariant is pinned by a test).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const read = (rel) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');
const DRIVER = read('../public/office-driver.js');
const CONSUMERS = ['../src/components/OfficeBackdrop.astro', '../src/components/Showcase.astro'];

for (const name of ['WASM_INIT_RETRIES', 'WASM_INIT_BACKOFF_MS']) {
  test(`${name} is declared once, in office-driver.js`, () => {
    assert.ok(
      new RegExp(`const ${name} = \\d+`).test(DRIVER),
      `office-driver.js must declare ${name}`
    );
  });

  test(`${name} is not re-inlined in either live-office consumer`, () => {
    for (const rel of CONSUMERS) {
      assert.ok(
        !read(rel).includes(name),
        `${rel} re-inlined ${name} — it must ride the shared office-driver.js`
      );
    }
  });
}
