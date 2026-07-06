// FLOOR_SPY_ROOT_MARGIN (consts.ts) is the ONE authority for the floor-spy
// IntersectionObserver band — the Statusline's scrollspy and the
// ElevatorShaft's current-floor LED/car observer must read the SAME band or
// "the two readouts can never disagree" is only a comment, not a fact. This
// pins both component sources to the const (via define:vars, since is:inline
// scripts can't `import` at runtime) and bans a reintroduced literal '-45%'
// copy — a one-sided retune fails this instead of shipping silent drift.
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
