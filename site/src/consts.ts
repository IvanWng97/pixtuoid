import themesData from './themes.json';
import weatherData from './weather.json';
import showcaseData from './showcase.json';
import featuresData from './features.json';

// Shared site constants + a base-path-safe asset/link helper.
// (The site serves at the origin root of pixtuoid.dev — base '/' — but every
//  internal URL still goes through asset()/BASE_URL so a base change, like the
//  old /pixtuoid/ project page, can never silently break links.)
export const REPO = 'https://github.com/IvanWng97/pixtuoid';
export const CRATES = 'https://crates.io/crates/pixtuoid';
export const SPONSOR = 'https://buymeacoffee.com/IvanWng97';
// Deploy origin. The BUILD authority is `site` in astro.config.mjs — `Astro.site`
// reflects it, and this const is only the type-narrowing fallback for the
// (build-time unreachable) `Astro.site` undefined arm, shared so the two head
// consumers (Base.astro canonical/og, index.astro JSON-LD) can't drift apart.
export const SITE_ORIGIN = 'https://pixtuoid.dev';

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

// ── Floor identity: id ↔ floor string ↔ label, single-sourced ──
// THE Working Building shared-contract manifest: the Statusline lift, the
// Base.astro key handler, the (wb-3) ElevatorShaft, and every narrative
// section stamp their id / data-floor / data-floor-label / eyebrow from this
// ONE array, so a renumber / rename / add can't silently desync them.
// Reading order = top floor down (the page scrolls 6F → 1F).
export interface Floor {
  id: string; // section id + the digit-key / elevator jump target (getElementById) — the EXISTING dom ids, unchanged
  fl: string; // '6F'..'1F' — the data-floor stamp, lift readout, digit-key vocabulary
  label: string; // 'penthouse — welcome' — data-floor-label + wayfinding copy
}
export const FLOORS: Floor[] = [
  { id: 'lobby', fl: '6F', label: 'penthouse — welcome' },
  { id: 'showcase', fl: '5F', label: 'studio — demos' },
  { id: 'amenities', fl: '4F', label: 'amenities — see it real' },
  { id: 'how', fl: '3F', label: 'machine room — quickstart' },
  { id: 'tools', fl: '2F', label: 'tenants — compatibility' },
  { id: 'install', fl: '1F', label: 'front desk — install' },
];
export const floorById = (id: string): Floor => {
  const f = FLOORS.find((x) => x.id === id);
  if (!f) throw new Error(`consts: no FLOORS entry for floor id "${id}"`);
  return f;
};
// the short name half of a label ('penthouse — welcome' → 'penthouse'), for
// eyebrows and the lift readout
export const floorName = (f: Floor): string => f.label.split(' — ')[0];

// The ONE floor-spy IntersectionObserver band: the Statusline's scrollspy and
// the ElevatorShaft's current-floor LED/car each build their own observer
// over the SAME [data-floor] sections, so a one-sided retune of the band
// would make the two readouts silently disagree. Both thread this in via
// define:vars (is:inline scripts can't `import` at runtime) — no second
// '-45%' literal anywhere.
export const FLOOR_SPY_ROOT_MARGIN = '-45% 0px -45% 0px';
// Rounding slack for the bottom-clamp both floor-spy consumers apply at true
// scroll max (scrollY+innerHeight vs scrollHeight can differ by a subpixel):
// ONE value so the shaft LED and the statusline can never desync at page end.
export const BOTTOM_CLAMP_EPSILON_PX = 2;

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
  id: string; // slug; hash target #showcase-<id> — ALSO the features.json row's `channel` value (the wb-3 bijection)
  label: string; // monitor label (channel number is derived from manifest order)
  kind: 'clip' | 'variant-set' | 'live';
  asset?: string; // clip: demos/<asset>.mp4 [+ .webm] + <asset>-poster.png
  // clip: intrinsic dims (CLS); live: the canvas BUFFER dims — the vibing
  // "camera": smaller = closer zoom, fewer desks fill the floor naturally.
  // MUST match media.json's poster job (w/h) or the static poster and the
  // live canvas reframe on the crossfade; pinned by
  // config/vibing-parity.test.mjs.
  w?: number;
  h?: number;
  variants?: ShowcaseVariant[]; // variant-set channels: INLINE variants (channel-level variantsRef was retired with the #468 stills; a live channel's manifest refs live on its variantGroups)
  retint?: boolean; // chips retint the page (themes only)
  variantGroups?: VariantGroup[]; // live channel: multiple independently-labeled chip groups
  timeSlider?: boolean; // live channel: exposes a time-of-day scrub control
  poster?: string; // live channel: public/demos/-relative static fallback image
  // diegetic one-liner under the stage — OPTIONAL: a channel whose caption
  // would just restate its joined feature's `desc` (currently only "pets")
  // omits it and falls back to that desc (ChannelStage's `description`),
  // rather than print the same content twice on the same screen.
  caption?: string;
  duration?: string; // clip badge, m:ss
  status: 'live' | 'soon'; // soon = dimmed placeholder monitor, no assets needed
  default?: boolean; // exactly one — the channel tuned at load
}

// Single source of truth for the Studio Wall → site/src/showcase.json.
// themes.json / weather.json stay untouched (their README-sync + just gen-media
// loops + Rust enum guard tests are unaffected); a live channel's variantGroups
// reference them via a per-group variantsRef and resolve in showcaseGroups.
// The manifest's kind/status/default/asset invariants are enforced at build time by the showcase guard in astro.config.mjs.
export const SHOWCASE: ShowcaseChannel[] = showcaseData as unknown as ShowcaseChannel[];

// ── The feature collection: features.json is the TOTAL set (also the README
// Features table's source, via gen-readme.mjs). A row that has a video/live
// demo channel carries `channel: "<showcase id>"`; the Studio Wall dial is
// exactly those rows, joined against SHOWCASE for the stage's clip/live
// config — one collection, partitioned by has-demo (Showcase.astro's roster
// is the complementary `!channel` half). The channel↔feature bijection is
// enforced at build time by astro.config.mjs's guard.
export interface Feature {
  icon: string;
  name: string;
  desc: string;
  featured?: boolean;
  pix?: string;
  channel?: string; // joins to a SHOWCASE row's `id`
  card?: { blurb: string };
}
export const FEATURES: Feature[] = featuresData as Feature[];

const featureByChannelId = new Map(FEATURES.filter((f) => f.channel).map((f) => [f.channel!, f]));

// A SHOWCASE channel's joined feature — guaranteed to resolve by the
// astro.config.mjs bijection guard, so callers use it unchecked (matching
// ChannelStage's existing `defVariant!` non-null-assertion idiom).
export function featureForChannel(id: string): Feature {
  return featureByChannelId.get(id)!;
}

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
  featureDesc: string; // joined features.json row's desc — the dial accordion body, and the caption fallback
}

// The manifest a live channel's per-group `variantsRef` maps to — showcaseGroups
// is the ONLY caller (channel-level variantsRef was retired, #468). `src` below
// is dead weight — ChannelStage's live-chip branch renders only id/name/accent
// and never reads it (those theme_<id>.png/weather_<id>.png stills are gone) —
// kept only so the ShowcaseVariant shape stays uniform.
function variantsForRef(ref: 'themes' | 'weather'): ShowcaseVariant[] {
  if (ref === 'themes')
    return THEMES.map((t) => ({
      id: t.id,
      name: t.name,
      src: `theme_${t.id}.png`,
      accent: t.accent,
      accent2: t.accent2,
      featured: t.featured,
    }));
  return WEATHERS.map((w) => ({
    id: w.id,
    name: w.name,
    src: `weather_${w.id}.png`,
    // storm is the most striking opener for the weather channel
    featured: w.id === 'storm',
  }));
}

export function showcaseVariants(c: ShowcaseChannel): ShowcaseVariant[] {
  // variant-set channels supply INLINE variants only. Channel-level `variantsRef`
  // resolved the theme_<id>.png/weather_<id>.png stills #468 deleted, so it was
  // retired (the astro.config guard rejects a stray one); a live channel's
  // manifest refs live on its variantGroups (showcaseGroups).
  return c.variants ?? [];
}

export function showcaseGroups(c: ShowcaseChannel): EnrichedVariantGroup[] {
  return (c.variantGroups ?? []).map((g: VariantGroup) => ({
    key: g.key,
    label: g.label,
    retint: g.retint ?? false,
    variants: variantsForRef(g.variantsRef),
  }));
}
