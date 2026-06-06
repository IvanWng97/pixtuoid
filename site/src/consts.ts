import themesData from './themes.json';
import weatherData from './weather.json';
import showcaseData from './showcase.json';

// Shared site constants + a base-path-safe asset/link helper.
// (GitHub Pages serves the site under /pixtuoid/, so every internal URL must
//  be prefixed with import.meta.env.BASE_URL — asset() does that robustly.)
export const REPO = 'https://github.com/IvanWng97/pixtuoid';
export const CRATES = 'https://crates.io/crates/pixtuoid';
export const SPONSOR = 'https://buymeacoffee.com/IvanWng97';

const BASE = import.meta.env.BASE_URL;
export const asset = (p: string): string => `${BASE.replace(/\/$/, '')}/${p.replace(/^\//, '')}`;

export interface ThemeShot {
  id: string;
  name: string;
  blurb: string;
  accent: string; // primary hue (chip + retint)
  accent2: string; // gradient end hue
  featured?: boolean; // shown first in the switcher
}

// Single source of truth for the theme switcher → site/src/themes.json.
// Add a theme there + render its screenshot (scripts/gen-demos.sh) and the gallery,
// the live count, the retint, and the render script all pick it up. No component edits.
export const THEMES: ThemeShot[] = themesData as ThemeShot[];

export interface WeatherShot {
  id: string; // matches `--weather <id>` + public/demos/weather_<id>.png
  name: string;
  blurb: string;
}

// Single source of truth for the weather gallery → site/src/weather.json. The
// manifest↔art↔gallery triangle is guarded here (gen-demos.sh derives its render
// loop from this file; astro.config fails the build if any id lacks its
// weather_<id>.png); the manifest↔Rust-enum edge is guarded by the
// `weather_gallery_manifest_matches_the_weather_enum` unit test in pixtuoid.
export const WEATHERS: WeatherShot[] = weatherData as WeatherShot[];

export interface ShowcaseVariant {
  id: string;
  name: string;
  blurb: string;
  src: string; // public/demos/-relative filename
  accent?: string;
  accent2?: string;
  featured?: boolean; // default chip for its channel
}

export interface ShowcaseChannel {
  id: string; // slug; hash target #showcase-<id>
  label: string; // monitor label (channel number is derived from manifest order)
  kind: 'clip' | 'variant-set';
  asset?: string; // clip: demos/<asset>.mp4 [+ .webm] + <asset>-poster.png
  w?: number; // clip intrinsic dims (CLS)
  h?: number;
  variantsRef?: 'themes' | 'weather'; // variant-set backed by an existing manifest
  variants?: ShowcaseVariant[]; // …or inline variants
  retint?: boolean; // chips retint the page (themes only)
  caption: string; // diegetic one-liner under the stage
  duration?: string; // clip badge, m:ss
  status: 'live' | 'soon'; // soon = dimmed placeholder monitor, no assets needed
  default?: boolean; // exactly one — the channel tuned at load
}

// Single source of truth for the Studio Wall → site/src/showcase.json.
// themes.json / weather.json stay untouched (their README-sync + gen-demos.sh
// loops + Rust enum guard tests are unaffected); variant-set channels reference
// them via variantsRef and resolve here.
// The manifest's kind/status/default/asset invariants are enforced at build time by the showcase guard in astro.config.mjs.
export const SHOWCASE: ShowcaseChannel[] = showcaseData as unknown as ShowcaseChannel[];

// The shape Showcase.astro passes down to ChannelStage/MonitorWall: each
// channel enriched with `ch` (zero-padded channel number, from manifest order)
// and `variants` resolved via showcaseVariants() (always an array, may be empty).
export interface EnrichedShowcaseChannel extends ShowcaseChannel {
  ch: string;
  variants: ShowcaseVariant[];
}

export function showcaseVariants(c: ShowcaseChannel): ShowcaseVariant[] {
  if (c.variantsRef === 'themes')
    return THEMES.map((t) => ({
      id: t.id,
      name: t.name,
      blurb: t.blurb,
      src: `theme_${t.id}.png`,
      accent: t.accent,
      accent2: t.accent2,
      featured: t.featured,
    }));
  if (c.variantsRef === 'weather')
    return WEATHERS.map((w) => ({
      id: w.id,
      name: w.name,
      blurb: w.blurb,
      src: `weather_${w.id}.png`,
      // storm is the most striking opener for the weather channel
      featured: w.id === 'storm',
    }));
  return c.variants ?? [];
}
