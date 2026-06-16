// @ts-check
import { defineConfig } from 'astro/config';
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { posix } from 'node:path';
import sitemap from '@astrojs/sitemap';
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
// Fix by running `just gen-media` (the binary must ship the theme first).
const themeIds = /** @type {{ id: string }[]} */ (
  JSON.parse(readFileSync(fileURLToPath(new URL('./src/themes.json', import.meta.url)), 'utf8'))
).map((t) => t.id);
const missingDemos = themeIds.filter(
  (id) => !existsSync(fileURLToPath(new URL(`./public/demos/theme_${id}.png`, import.meta.url)))
);
if (missingDemos.length) {
  throw new Error(
    `astro.config: themes.json lists theme(s) with no public/demos/theme_<id>.png — run just gen-media: ${missingDemos.join(', ')}`
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
    `astro.config: weather.json lists weather(s) with no public/demos/weather_<id>.png — run just gen-media: ${missingWeather.join(', ')}`
  );
}

// Studio Wall guard: showcase.json must have exactly one default channel, unique
// ids, and every `live` channel's assets on disk (clips: webm + mp4 + poster —
// the encode_clip ladder always emits webm, so ChannelStage emits its <source>
// unconditionally). `soon` placeholders need nothing.
const showcase = /** @type {any[]} */ (
  JSON.parse(readFileSync(fileURLToPath(new URL('./src/showcase.json', import.meta.url)), 'utf8'))
);
const scDefaults = showcase.filter((c) => c.default);
if (scDefaults.length !== 1 || scDefaults[0].status !== 'live') {
  throw new Error(
    `astro.config: showcase.json needs exactly one default LIVE channel (got ${scDefaults.map((c) => c.id).join(', ') || 'none'})`
  );
}
const scIds = new Set();
for (const c of showcase) {
  if (scIds.has(c.id)) throw new Error(`astro.config: showcase.json duplicate id "${c.id}"`);
  scIds.add(c.id);
  if (c.status === 'soon') continue;
  const demo = /** @param {string} f */ (f) =>
    existsSync(fileURLToPath(new URL(`./public/demos/${f}`, import.meta.url)));
  if (c.kind === 'clip') {
    if (!c.asset)
      throw new Error(
        `astro.config: showcase.json live clip "${c.id}" is missing the required "asset" field`
      );
    const missing = [`${c.asset}.webm`, `${c.asset}.mp4`, `${c.asset}-poster.png`].filter(
      (f) => !demo(f)
    );
    if (missing.length)
      throw new Error(
        `astro.config: showcase.json live clip "${c.id}" missing public/demos/ asset(s): ${missing.join(', ')} — run just gen-media`
      );
    if (!Number.isFinite(c.w) || !Number.isFinite(c.h))
      throw new Error(
        `astro.config: showcase.json live clip "${c.id}" needs numeric "w"/"h" (intrinsic video dims, for CLS)`
      );
  } else if (c.kind === 'variant-set') {
    if (c.variantsRef) {
      if (c.variantsRef !== 'themes' && c.variantsRef !== 'weather')
        throw new Error(
          `astro.config: showcase.json "${c.id}" has unknown variantsRef "${c.variantsRef}" (expected "themes" or "weather")`
        );
    } else if (!(c.variants && c.variants.length)) {
      throw new Error(
        `astro.config: showcase.json variant-set "${c.id}" has neither variantsRef nor variants`
      );
    }
    for (const v of c.variants ?? [])
      if (!demo(v.src))
        throw new Error(
          `astro.config: showcase.json "${c.id}" variant "${v.id}" missing public/demos/${v.src}`
        );
  } else {
    throw new Error(`astro.config: showcase.json "${c.id}" has unknown kind "${c.kind}"`);
  }
}

// Cross-guard: every features.json card `#showcase-<id>` deep-link must resolve
// to a real showcase channel — so renaming/removing a channel (e.g. OFFICE/FLOORS
// → AGENTS) can't silently orphan a "See it live →" button (the showcase guard
// above only checks ids internal to showcase.json).
const features = /** @type {any[]} */ (
  JSON.parse(readFileSync(fileURLToPath(new URL('./src/features.json', import.meta.url)), 'utf8'))
);
for (const f of features) {
  const href = f.card?.href;
  if (typeof href === 'string' && href.startsWith('#showcase-')) {
    const id = href.slice('#showcase-'.length);
    if (!scIds.has(id))
      throw new Error(
        `astro.config: features.json "${f.name}" links to #showcase-${id}, which is not a showcase.json channel id (${[...scIds].join(', ')})`
      );
  }
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
  // Sitemap respects `site` + `base`, so emitted URLs carry the /pixtuoid
  // prefix → submit /pixtuoid/sitemap-index.xml to Search Console (this is a
  // project page, so a repo-local robots.txt would serve under /pixtuoid/ and
  // crawlers only read robots.txt from the origin root — see the PR notes).
  integrations: [sitemap()],
  vite: { define: { __PIXTUOID_VERSION__: JSON.stringify(version) } },
});
