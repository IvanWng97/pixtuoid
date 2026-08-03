import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const read = (rel) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');
const consts = read('../src/consts.ts');
const css = read('../src/styles/global.css');

function themeBg() {
  const body = consts.match(/THEME_BG\s*:\s*Record<[^>]*>\s*=\s*\{([\s\S]*?)\}/);
  assert.ok(body, 'THEME_BG object literal not found in consts.ts');
  const map = new Map();
  for (const m of body[1].matchAll(/(\w+)\s*:\s*'(#[0-9a-fA-F]{3,8})'/g))
    map.set(m[1], m[2].toLowerCase());
  return map;
}

function cssBg(varDecls, raw) {
  const v = raw.trim();
  const hop = v.match(/^var\((--[\w-]+)\)$/);
  return (hop ? varDecls.get(hop[1]) : v)?.toLowerCase();
}

test('THEME_BG mirrors global.css --bg for every theme', () => {
  const decls = new Map();
  for (const m of css.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) decls.set(m[1], m[2].trim());

  // `--bg` is redefined per theme, so it can't come from the flat last-wins map:
  // day reads the base `:root {` block, the rest their `[data-theme='x']` block.
  const base = css.match(/:root\s*\{([\s\S]*?)\}/);
  assert.ok(base, 'base :root block not found');
  const dayBg = base[1].match(/--bg\s*:\s*([^;]+);/);
  assert.ok(dayBg, 'base :root has no --bg');
  const bgFor = { day: cssBg(decls, dayBg[1]) };
  for (const m of css.matchAll(/\[data-theme=['"](\w+)['"]\]\s*\{([\s\S]*?)\}/g)) {
    const inner = m[2].match(/--bg\s*:\s*([^;]+);/);
    if (inner) bgFor[m[1]] = cssBg(decls, inner[1]);
  }

  for (const [id, hex] of themeBg()) {
    assert.equal(
      bgFor[id],
      hex,
      `THEME_BG.${id} (${hex}) must equal global.css --bg for ${id} (${bgFor[id]})`
    );
  }
});
