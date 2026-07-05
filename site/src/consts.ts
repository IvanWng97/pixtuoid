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

// ── Theme constants (single source for the Base/Nav inline-script family) ──
// Seeded parse-first into window.__pixTheme (Base.astro head) via define:vars so
// the theme init, the Escape restore, the FX theme-color sync, and Nav's toggle
// all read ONE storage key + valid set + BG map and can't drift. THEME_BG is the
// mobile browser-chrome tint per theme; its hexes MIRROR global.css's per-theme
// --bg (day #f4eee2 / night #1d1813 / dracula #282a36) — a CSS↔JS pairing that
// can't share a literal, so it's pinned here by comment (retune both together).
export const THEME_STORAGE_KEY = 'pix-theme';
export const VALID_THEMES = ['day', 'night', 'dracula'] as const;
export type ThemeId = (typeof VALID_THEMES)[number];
export const THEME_BG: Record<ThemeId, string> = {
  day: '#f4eee2',
  night: '#1d1813',
  dracula: '#282a36',
};

// ── Floor identity: number ↔ section id ↔ name, single-sourced ──
// Statusline builds its lift registry from this AND every narrative section
// stamps its own data-floor + id + eyebrow prefix from its entry (via
// floorByNumber), so a renumber / rename / add can't silently desync the
// digit-key scrollspy or the lift readout (load-bearing runtime contracts).
// Reading order = top floor down (the page scrolls 6F → 1F).
export interface Floor {
  n: number;
  id: string; // section id + the digit-key jump target (getElementById)
  name: string; // eyebrow + lift readout name
}
export const FLOORS: Floor[] = [
  { n: 6, id: 'lobby', name: 'penthouse' },
  { n: 5, id: 'showcase', name: 'studio' },
  { n: 4, id: 'features', name: 'amenities' },
  { n: 3, id: 'how', name: 'machine room' },
  { n: 2, id: 'tools', name: 'tenants' },
  { n: 1, id: 'install', name: 'front desk' },
];
export const floorByNumber = (n: number): Floor => {
  const f = FLOORS.find((x) => x.n === n);
  if (!f) throw new Error(`consts: no FLOORS entry for floor ${n}`);
  return f;
};

// The dimmer's resting opacity — the single source for FIVE former copies that
// straddle a JS↔CSS boundary. OfficeBackdrop emits it into #dimmer's CSS via an
// inline `--dim-rest` custom property (its base + reduced-motion rules read it);
// Statusline derives its 'lights N%' readout from it (100·(1 − DIM_RESTING)).
// Both read THIS value at build time, so the pre-JS/reduced-motion dim level and
// the statusline readout can never drift (they had already drifted: 55% vs 45%).
export const DIM_RESTING = 0.55;

// ── Rendered docs pages: one manifest the Nav dropdown, the Docs sidebar/pager,
// and each page's `current` type all derive from (was triple-scattered). Fixed
// reading order = the prev/next sequence AND the Nav dropdown order. Adding a
// rendered doc still needs its content collection + page file (see
// site/CLAUDE.md), but the menu/sidebar/pager come free from this one edit.
export interface DocPage {
  id: string;
  route: string; // base-path-relative slug (asset(route))
  label: string;
}
export const DOCS = [
  { id: 'config', route: 'config', label: 'Configuration' },
  { id: 'architecture', route: 'architecture', label: 'Architecture' },
  { id: 'knowledge-base', route: 'knowledge-base', label: 'Knowledge engineering' },
  { id: 'parallel-delivery', route: 'parallel-delivery', label: 'Parallel delivery' },
  { id: 'contributing', route: 'contributing', label: 'Contributing' },
] as const satisfies readonly DocPage[];
export type DocId = (typeof DOCS)[number]['id'];

export interface ThemeShot {
  id: string;
  name: string;
  blurb: string;
  accent: string; // primary hue (chip + retint)
  accent2: string; // gradient end hue
  featured?: boolean; // shown first in the switcher
}

// Single source of truth for the theme switcher → site/src/themes.json. Themes
// now drive the live VIBING channel's theme chips IN-CANVAS (`Office::set_theme`)
// — the static theme gallery, the per-id `theme_<id>.png` stills, and
// astro.config's theme→still existence guard were all retired in #468. The
// surviving guard is the Rust-side `theme_gallery_manifest_matches_all_themes`
// set-equality test (pixtuoid-scene), so a live chip's `data-theme` always
// resolves.
export const THEMES: ThemeShot[] = themesData as ThemeShot[];

interface WeatherShot {
  id: string; // matches `--weather <id>` (the live VIBING chip's data-weather)
  name: string;
  blurb: string;
}

// Single source of truth for the weather chips → site/src/weather.json. Weather
// now drives the live VIBING channel's weather chips IN-CANVAS
// (`Office::set_weather`) — the static weather gallery, the per-id
// `weather_<id>.png` stills, and astro.config's weather→still existence guard
// were all retired in #468. The surviving guard is the Rust-side
// `weather_gallery_manifest_matches_the_weather_enum` set-equality test
// (pixtuoid-scene), so a live chip's `data-weather` always resolves.
const WEATHERS: WeatherShot[] = weatherData as WeatherShot[];

export interface ShowcaseVariant {
  id: string;
  name: string;
  blurb: string;
  src: string; // public/demos/-relative filename
  accent?: string;
  accent2?: string;
  featured?: boolean; // default chip for its channel
}

// A `live` channel's chip groups: each group is independently labeled and
// resolves its own manifest (weather chips preview only; theme chips also
// retint the page — hence per-group `retint`, not the channel-level one).
export interface VariantGroup {
  key: 'weather' | 'theme';
  label: string;
  variantsRef: 'themes' | 'weather';
  retint?: boolean;
}

export interface ShowcaseChannel {
  id: string; // slug; hash target #showcase-<id>
  label: string; // monitor label (channel number is derived from manifest order)
  kind: 'clip' | 'variant-set' | 'live';
  asset?: string; // clip: demos/<asset>.mp4 [+ .webm] + <asset>-poster.png
  w?: number; // clip intrinsic dims (CLS)
  h?: number;
  variantsRef?: 'themes' | 'weather'; // variant-set backed by an existing manifest
  variants?: ShowcaseVariant[]; // …or inline variants
  retint?: boolean; // chips retint the page (themes only)
  variantGroups?: VariantGroup[]; // live channel: multiple independently-labeled chip groups
  timeSlider?: boolean; // live channel: exposes a time-of-day scrub control
  poster?: string; // live channel: public/demos/-relative static fallback image
  caption: string; // diegetic one-liner under the stage
  duration?: string; // clip badge, m:ss
  status: 'live' | 'soon'; // soon = dimmed placeholder monitor, no assets needed
  default?: boolean; // exactly one — the channel tuned at load
}

// Single source of truth for the Studio Wall → site/src/showcase.json.
// themes.json / weather.json stay untouched (their README-sync + just gen-media
// loops + Rust enum guard tests are unaffected); variant-set channels reference
// them via variantsRef and resolve here.
// The manifest's kind/status/default/asset invariants are enforced at build time by the showcase guard in astro.config.mjs.
export const SHOWCASE: ShowcaseChannel[] = showcaseData as unknown as ShowcaseChannel[];

// The shape Showcase.astro passes down to ChannelStage/MonitorWall: each
// channel enriched with `ch` (zero-padded channel number, from manifest order)
// and `variants` resolved via showcaseVariants() (always an array, may be empty).
export interface EnrichedVariantGroup {
  key: string;
  label: string;
  retint: boolean;
  variants: ShowcaseVariant[];
}

export interface EnrichedShowcaseChannel extends ShowcaseChannel {
  ch: string;
  variants: ShowcaseVariant[];
  groups: EnrichedVariantGroup[];
}

// The manifest resolution a `variantsRef` maps to — the SINGLE place both
// showcaseVariants (channel-level) and showcaseGroups (live-channel, one per
// group) read THEMES/WEATHERS, so the two callers can never disagree on shape.
// `src` below is dead weight for a live-channel consumer (showcaseGroups) —
// ChannelStage's live-chip branch renders only id/name/accent and never reads
// it (those theme_<id>.png/weather_<id>.png stills are gone, #468); it's kept
// because variant-set channels' chip branch still `data-src`-swaps with it.
function variantsForRef(ref: 'themes' | 'weather'): ShowcaseVariant[] {
  if (ref === 'themes')
    return THEMES.map((t) => ({
      id: t.id,
      name: t.name,
      blurb: t.blurb,
      src: `theme_${t.id}.png`,
      accent: t.accent,
      accent2: t.accent2,
      featured: t.featured,
    }));
  return WEATHERS.map((w) => ({
    id: w.id,
    name: w.name,
    blurb: w.blurb,
    src: `weather_${w.id}.png`,
    // storm is the most striking opener for the weather channel
    featured: w.id === 'storm',
  }));
}

export function showcaseVariants(c: ShowcaseChannel): ShowcaseVariant[] {
  // A channel may carry a manifest-backed `variantsRef` AND extra inline
  // `variants`, appended after it (variant-set channels only) — no current
  // channel exercises the append, but the shape supports it.
  const inline = c.variants ?? [];
  if (c.variantsRef === 'themes' || c.variantsRef === 'weather')
    return [...variantsForRef(c.variantsRef), ...inline];
  return inline;
}

export function showcaseGroups(c: ShowcaseChannel): EnrichedVariantGroup[] {
  return (c.variantGroups ?? []).map((g: VariantGroup) => ({
    key: g.key,
    label: g.label,
    retint: g.retint ?? false,
    variants: variantsForRef(g.variantsRef),
  }));
}
