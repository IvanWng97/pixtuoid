// The Showcase scrubber lands each jump in the engine's strike-free tail of a
// lightning bucket; its two constants are copies of pixtuoid-scene's, pinned
// here so an engine change can't silently let a scrub flash.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const read = (rel) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');
const SKY = read('../../crates/pixtuoid-scene/src/sky/mod.rs');
const SHOWCASE = read('../src/components/Showcase.astro');

for (const name of ['LIGHTNING_PERIOD_MS', 'STRIKE_GAP_MS']) {
  test(`the scrubber's ${name} is the engine's`, () => {
    const rust = SKY.match(new RegExp(`const ${name}: u64 = (\\d+);`));
    const js = SHOWCASE.match(new RegExp(`const ${name} = (\\d+);`));
    assert.ok(rust, `pixtuoid-scene's sky declares ${name}`);
    assert.ok(js, `Showcase.astro declares ${name}`);
    assert.equal(js[1], rust[1]);
  });
}

test('the engine keeps every strike out of a bucket’s last STRIKE_GAP_MS', () => {
  // the offset's modulus leaves the strike and the gap inside the bucket
  assert.match(SKY, /% \(LIGHTNING_PERIOD_MS - STRIKE_MS - STRIKE_GAP_MS\)/);
});
