// The bounded wasm-init retry must live ONCE in the shared office-driver.js both
// live-office components dynamic-import: a re-inlined copy would silently
// reintroduce the two-scripts drift class.
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
    // Match a DECLARATION, not a bare mention — a comment cross-referencing the
    // const by name must not spuriously red this.
    for (const rel of CONSUMERS) {
      assert.ok(
        !new RegExp(`const ${name}\\s*=`).test(read(rel)),
        `${rel} re-inlined ${name} — it must ride the shared office-driver.js`
      );
    }
  });
}
