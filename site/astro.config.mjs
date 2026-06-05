// @ts-check
import { defineConfig } from 'astro/config';
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { posix } from 'node:path';
import rehypeMermaid from 'rehype-mermaid';

// Single-source the displayed version from the workspace Cargo.toml so the boot
// intro never goes stale on a release bump. Scope the match to the
// [workspace.package] table so a dependency's line-anchored `version = "…"` (in a
// [dependencies.x] sub-table) can't be picked up — and throw rather than silently
// shipping a bogus version if the parse ever fails.
const cargoToml = readFileSync(fileURLToPath(new URL('../Cargo.toml', import.meta.url)), 'utf8');
const pkgSection = cargoToml.match(/\[workspace\.package\]([\s\S]*?)(?:\n\[|$)/)?.[1] ?? '';
const version = pkgSection.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!version) {
  throw new Error('astro.config: could not parse [workspace.package] version from ../Cargo.toml');
}

// Guard: every theme in the switcher manifest must have a rendered demo PNG.
// site CI never runs the binary, so without this a theme added to themes.json
// before its screenshot exists would deploy a chip with a 404 image (#121).
// Fix by running scripts/gen-demos.sh (the binary must ship the theme first).
const themeIds = /** @type {{ id: string }[]} */ (
  JSON.parse(readFileSync(fileURLToPath(new URL('./src/themes.json', import.meta.url)), 'utf8'))
).map((t) => t.id);
const missingDemos = themeIds.filter(
  (id) => !existsSync(fileURLToPath(new URL(`./public/demos/theme_${id}.png`, import.meta.url)))
);
if (missingDemos.length) {
  throw new Error(
    `astro.config: themes.json lists theme(s) with no public/demos/theme_<id>.png — run scripts/gen-demos.sh: ${missingDemos.join(', ')}`
  );
}

// Same guard for the weather gallery: every weather.json id needs a weather_<id>.png.
const weatherIds = /** @type {{ id: string }[]} */ (
  JSON.parse(readFileSync(fileURLToPath(new URL('./src/weather.json', import.meta.url)), 'utf8'))
).map((w) => w.id);
const missingWeather = weatherIds.filter(
  (id) => !existsSync(fileURLToPath(new URL(`./public/demos/weather_${id}.png`, import.meta.url)))
);
if (missingWeather.length) {
  throw new Error(
    `astro.config: weather.json lists weather(s) with no public/demos/weather_<id>.png — run scripts/gen-demos.sh: ${missingWeather.join(', ')}`
  );
}

// And for the Features bento: every features.json card.img must exist in
// public/demos/ (day/night/theme shots referenced by free-form filename sit
// outside the two id-based guards above — a typo'd img would deploy a 404 card).
const cardImgs = /** @type {{ card?: { img?: string } }[]} */ (
  JSON.parse(readFileSync(fileURLToPath(new URL('./src/features.json', import.meta.url)), 'utf8'))
).flatMap((f) => (f.card?.img ? [f.card.img] : []));
const missingCards = cardImgs.filter(
  (img) => !existsSync(fileURLToPath(new URL(`./public/demos/${img}`, import.meta.url)))
);
if (missingCards.length) {
  throw new Error(
    `astro.config: features.json card img(s) missing from public/demos/ — run scripts/gen-demos.sh or fix the filename: ${missingCards.join(', ')}`
  );
}

// Rewrite repo-relative links in rendered markdown (e.g. ../crates/...) to GitHub
// so docs/CONFIGURATION.md's links resolve on the deployed site.
function rehypeRepoLinks() {
  const repo = 'https://github.com/IvanWng97/pixtuoid/blob/main/';
  const DOC_DIR = 'docs'; // CONFIGURATION.md lives in docs/ — repo-relative links resolve from there
  const SCHEME = /^[a-z][a-z0-9+.-]*:/i; // https: / mailto: / javascript: …
  const DANGEROUS = /^\s*(?:javascript|data|vbscript):/i;
  /** @param {any} node */
  const walk = (node) => {
    if (node.tagName === 'a' && node.properties && typeof node.properties.href === 'string') {
      const href = node.properties.href;
      if (DANGEROUS.test(href)) {
        // neutralize an unsafe scheme — defense-in-depth (the doc is trusted today)
        node.properties.href = '#';
      } else if (!href.startsWith('#') && !SCHEME.test(href)) {
        // repo-relative (./ ../ bare or /root-relative): resolve from docs/, clamp
        // any climb above the repo root, then point at the GitHub blob
        const joined = href.startsWith('/') ? href : posix.join(DOC_DIR, href);
        const rel = posix
          .normalize(joined)
          .replace(/^(?:\.\.\/)+/, '')
          .replace(/^\/+/, '');
        node.properties.href = repo + rel;
      }
      // else: in-page #anchor or absolute http(s)/mailto — leave untouched
    }
    (node.children || []).forEach(walk);
  };
  /** @param {any} tree */
  const transform = (tree) => walk(tree);
  return transform;
}

// Project page → https://ivanwng97.github.io/pixtuoid/
// If a custom domain is later added, set base back to '/' (and update CNAME).
export default defineConfig({
  site: 'https://ivanwng97.github.io',
  base: '/pixtuoid',
  trailingSlash: 'ignore',
  markdown: {
    // keep ```mermaid as a RAW code node — Shiki would otherwise highlight it
    // into a <pre> before rehype-mermaid can turn it into an inline SVG.
    syntaxHighlight: { type: 'shiki', excludeLangs: ['mermaid'] },
    rehypePlugins: [
      // build-time render: ```mermaid → inline <svg> (zero client JS, CSP-safe).
      [
        rehypeMermaid,
        {
          strategy: 'inline-svg',
          mermaidConfig: { theme: 'neutral', flowchart: { htmlLabels: true } },
        },
      ],
      rehypeRepoLinks, // after mermaid so it walks the final tree
    ],
  },
  vite: { define: { __PIXTUOID_VERSION__: JSON.stringify(version) } },
});
