// The VIBING channel's static poster must be a truthful still of the live
// canvas it covers: same buffer dims (the "camera" — smaller buffer = closer
// zoom), same layout seed. Those values necessarily live in three places —
// showcase.json (the canvas), scripts/media.json (the poster render), and
// Showcase.astro's VIBING_SEED (the Office constructor) — so this test pins
// the copies together (the workspace magic-number rule: values that cross a
// config boundary get a drift tooth, not trust).
import { strict as assert } from 'node:assert';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import test from 'node:test';

const here = dirname(fileURLToPath(import.meta.url));
const showcase = JSON.parse(readFileSync(join(here, '..', 'src', 'showcase.json'), 'utf8'));
const media = JSON.parse(readFileSync(join(here, '..', '..', 'scripts', 'media.json'), 'utf8'));
const astro = readFileSync(join(here, '..', 'src', 'components', 'Showcase.astro'), 'utf8');
const stage = readFileSync(join(here, '..', 'src', 'components', 'ChannelStage.astro'), 'utf8');

const vibing = showcase.find((c) => c.id === 'vibing');
const poster = media.find((j) => j.id === 'vibing-poster');

test('vibing poster job mirrors the live canvas (dims + seed)', () => {
  assert.ok(vibing, 'showcase.json has a vibing channel');
  assert.ok(poster, 'media.json has a vibing-poster job');
  assert.equal(poster.w, vibing.w, 'poster width == canvas buffer width');
  assert.equal(poster.h, vibing.h, 'poster height == canvas buffer height');
  const seedMatch = astro.match(/const VIBING_SEED = (\d+)/);
  assert.ok(seedMatch, 'Showcase.astro declares VIBING_SEED as a numeric literal');
  assert.equal(
    Number(seedMatch[1]),
    poster.seed,
    'poster layout seed == the live Office constructor seed'
  );
  // The remaining two cross-boundary copies: the poster's hour must equal the
  // time slider's SSR default, and its weather the default-active chip —
  // else the crossfade reframes to a different sky/wetness.
  const hourMatch = stage.match(/value="(\d+)"/);
  assert.ok(hourMatch, "ChannelStage.astro declares the time slider's default value");
  assert.equal(Number(hourMatch[1]), poster.hour, 'poster hour == slider default hour');
  const weatherMatch = stage.match(/weather: '([a-z]+)'/);
  assert.ok(weatherMatch, 'ChannelStage.astro declares the default weather chip id');
  assert.equal(weatherMatch[1], poster.weather, 'poster weather == default-active chip');
});
