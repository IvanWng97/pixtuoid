// THEME_BG (consts.ts, seeded into __pixTheme.BG for the mobile browser-chrome
// <meta theme-color>) MIRRORS global.css's per-theme `--bg`, a CSS↔JS pairing
// that can't share a literal. Rule-2 (magic values that cross a boundary get a
// match-test, not just a "retune together" comment): this asserts they agree,
// so a one-sided retune fails `just site-check` instead of shipping a mobile
// chrome tint that disagrees with the painted background.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const read = (rel) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8');
const consts = read('../src/consts.ts');
const css = read('../src/styles/global.css');

/** THEME_BG = { day: '#f4eee2', ... } → Map(id → hex). */
function themeBg() {
  const body = consts.match(/THEME_BG\s*:\s*Record<[^>]*>\s*=\s*\{([\s\S]*?)\}/);
  assert.ok(body, 'THEME_BG object literal not found in consts.ts');
  const map = new Map();
  for (const m of body[1].matchAll(/(\w+)\s*:\s*'(#[0-9a-fA-F]{3,8})'/g))
    map.set(m[1], m[2].toLowerCase());
  return map;
}

/** The value a `--bg` resolves to for a theme, following one `var(--x)` hop. */
function cssBg(varDecls, raw) {
  const v = raw.trim();
  const hop = v.match(/^var\((--[\w-]+)\)$/);
  return (hop ? varDecls.get(hop[1]) : v)?.toLowerCase();
}

test('THEME_BG mirrors global.css --bg for every theme', () => {
  // collect every `--name: value;` custom-prop declaration (last wins, as in CSS)
  const decls = new Map();
  for (const m of css.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) decls.set(m[1], m[2].trim());

  // per-theme --bg: day from the BASE `:root {` block (not the flat last-wins
  // map — `--bg` is redefined per theme), each other from its
  // `:root[data-theme='x']` block.
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
