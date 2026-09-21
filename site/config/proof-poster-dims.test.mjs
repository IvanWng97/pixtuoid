// ProofSplit's <video width/height> restate the OUTPUT size of gen-media's `proof`
// job so the box holds its aspect before the poster lands. A re-render at another
// grid would leave them stale, and the section would jump when the poster arrives.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const component = readFileSync(
  new URL('../src/components/ProofSplit.astro', import.meta.url),
  'utf8'
);

function pngSize(name) {
  // IHDR: width and height are the two big-endian u32s at byte 16.
  const png = readFileSync(new URL(`../public/demos/${name}`, import.meta.url));
  return { width: png.readUInt32BE(16), height: png.readUInt32BE(20) };
}

function declaredSize(variant) {
  const tag = new RegExp(`<video[^>]*proof__video--${variant}[^>]*>`).exec(component)?.[0] ?? '';
  return {
    width: Number(/\bwidth="(\d+)"/.exec(tag)?.[1]),
    height: Number(/\bheight="(\d+)"/.exec(tag)?.[1]),
  };
}

test("each proof video declares its own poster's pixel size", () => {
  assert.deepEqual(declaredSize('wide'), pngSize('proof-poster.png'));
  assert.deepEqual(declaredSize('tall'), pngSize('proof-tall-poster.png'));
});

test("the no-JS fallback image declares the wide poster's size too", () => {
  const img = /<noscript>[\s\S]*?<img[\s\S]*?\/>/.exec(component)?.[0] ?? '';
  assert.deepEqual(
    {
      width: Number(/width="(\d+)"/.exec(img)?.[1]),
      height: Number(/height="(\d+)"/.exec(img)?.[1]),
    },
    pngSize('proof-poster.png')
  );
});
