import themesData from './themes.json';

// Shared site constants + a base-path-safe asset/link helper.
// (GitHub Pages serves the site under /pixtuoid/, so every internal URL must
//  be prefixed with import.meta.env.BASE_URL — asset() does that robustly.)
export const REPO = 'https://github.com/IvanWng97/pixtuoid';
export const CRATES = 'https://crates.io/crates/pixtuoid';
export const TAP = 'ivanwng97/pixtuoid';

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
