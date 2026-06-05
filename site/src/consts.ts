import themesData from './themes.json';
import weatherData from './weather.json';

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
