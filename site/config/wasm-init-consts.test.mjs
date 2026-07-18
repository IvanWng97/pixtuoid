// The bounded wasm-init retry lives in TWO is:inline scripts that share no JS
// module scope — OfficeBackdrop's boot() and Showcase's bootCanvas() — so its
// consts are duplicated by necessity (an is:inline script can't `import` a
// shared const at runtime). Only ONE copy ever executes per page (the
// `window.__pixWasm ||` short-circuit picks whichever consumer boots first), so
// a drift is runtime-inert — but a one-sided retune would silently make the
// retry budget depend on which consumer booted first. Pin the two copies equal
// so the "keep in sync" comment is a fact, not a hope (the repo magic-number
// convention: duplicated cross-boundary consts are pinned by a test, not a
// comment alone).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const read = (rel) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');
const FILES = ['../src/components/OfficeBackdrop.astro', '../src/components/Showcase.astro'];

for (const name of ['WASM_INIT_RETRIES', 'WASM_INIT_BACKOFF_MS']) {
  test(`${name} is identical across both wasm-init boot scripts`, () => {
    const values = FILES.map((rel) => {
      const m = read(rel).match(new RegExp(`const ${name} = (\\d+)`));
      assert.ok(m, `${rel} must declare ${name}`);
      return m[1];
    });
    assert.equal(
      values[0],
      values[1],
      `${name} drifted between OfficeBackdrop.astro and Showcase.astro`
    );
  });
}
