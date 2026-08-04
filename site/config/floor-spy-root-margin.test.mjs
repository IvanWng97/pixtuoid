import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const read = (rel) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');

for (const [label, rel] of [
  ['Statusline.astro', '../src/components/Statusline.astro'],
  ['ElevatorShaft.astro', '../src/components/ElevatorShaft.astro'],
]) {
  test(`${label} threads FLOOR_SPY_ROOT_MARGIN instead of a literal '-45%' band`, () => {
    const src = read(rel);
    assert.match(
      src,
      /FLOOR_SPY_ROOT_MARGIN/,
      `${label} must import + define:vars-thread FLOOR_SPY_ROOT_MARGIN`
    );
    assert.doesNotMatch(src, /-45%/, `${label} must not carry its own '-45%' band literal`);
  });
}

test('neither floor-spy consumer re-declares the bottom-clamp epsilon', () => {
  for (const f of ['../src/components/Statusline.astro', '../src/components/ElevatorShaft.astro']) {
    const src = readFileSync(new URL(f, import.meta.url), 'utf8');
    assert.ok(
      !/const BOTTOM_CLAMP_EPSILON_PX/.test(src),
      `${f} must consume consts.ts's epsilon via define:vars, not re-declare it`
    );
    assert.ok(
      src.includes('bottomClampEpsilonPx: BOTTOM_CLAMP_EPSILON_PX'),
      `${f} must thread BOTTOM_CLAMP_EPSILON_PX through define:vars`
    );
  }
});
