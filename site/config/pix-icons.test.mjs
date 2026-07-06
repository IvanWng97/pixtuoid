// Bridge: every features.json `pix` icon must exist as a generated PNG in
// site/src/assets/pix-icons/ — the manifest-vs-artifact posture the Rust
// theme/weather set-equality guards use. Regenerate via `just gen-icons`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const features = JSON.parse(
  readFileSync(fileURLToPath(new URL('../src/features.json', import.meta.url)), 'utf8')
);

test('features.json declares pix icons for every bento card row', () => {
  const cards = features.filter((f) => f.card);
  assert.ok(cards.length >= 7, 'the bento rows exist');
  assert.deepEqual(
    cards.filter((f) => typeof f.pix !== 'string').map((f) => f.name),
    [],
    'every card-bearing feature carries a pix icon name'
  );
});

test('every features.json pix icon has a generated PNG', () => {
  const missing = features
    .filter((f) => typeof f.pix === 'string')
    .map((f) => f.pix)
    .filter(
      (n) =>
        !existsSync(fileURLToPath(new URL(`../src/assets/pix-icons/${n}.png`, import.meta.url)))
    );
  assert.deepEqual(missing, [], 'run: just gen-icons');
});
