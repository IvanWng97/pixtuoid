// @ts-check
import { defineConfig } from 'astro/config';
import { readFileSync, existsSync, writeFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { posix, join } from 'node:path';
import sitemap from '@astrojs/sitemap';
import rehypeMermaid from 'rehype-mermaid';
import { unified } from '@astrojs/markdown-remark';
import { rewriteCspMeta } from './config/csp-hashes.mjs';
import rehypeCallouts from './config/rehype-callouts.mjs';
import { fetchStarCount } from './config/gh-stars.mjs';
import { latestReleaseTag, resolveDisplayedVersion } from './config/released-version.mjs';

// The DISPLAYED version is the latest RELEASE tag — what `cargo install`/brew
// actually serve — not main's Cargo.toml, which runs AHEAD between a mid-cycle
// bump and its release tag; the Cargo.toml parse is only the tag-less fallback.
// Scope the match to [workspace.package] so a dependency's line-anchored
// `version = "…"` can't be picked up.
const cargoToml = readFileSync(fileURLToPath(new URL('../Cargo.toml', import.meta.url)), 'utf8');
const pkgSection = cargoToml.match(/\[workspace\.package\]([\s\S]*?)(?:\n\[|$)/)?.[1] ?? '';
const cargoVersion = pkgSection.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!cargoVersion) {
  throw new Error('astro.config: could not parse [workspace.package] version from ../Cargo.toml');
}
const { version, source: versionSource } = resolveDisplayedVersion(
  latestReleaseTag(),
  cargoVersion
);
console.log(`[pixtuoid] displayed version ${version} (from ${versionSource})`);

// Baked at build time — the CSP forbids a runtime fetch. null on any failure.
const ghStars = await fetchStarCount();

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
  if (c.variantsRef)
    throw new Error(
      `astro.config: showcase.json "${c.id}" has a channel-level variantsRef, retired in #468 — variant-set channels use inline "variants", live channels use variantGroups`
    );
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
    if (!(c.variants && c.variants.length))
      throw new Error(`astro.config: showcase.json variant-set "${c.id}" has no "variants"`);
    for (const v of c.variants)
      if (!demo(v.src))
        throw new Error(
          `astro.config: showcase.json "${c.id}" variant "${v.id}" missing public/demos/${v.src}`
        );
  } else if (c.kind === 'live') {
    // A `live` channel is rendered by the wasm office canvas, not static demo
    // assets — no asset/w/h required, but the fallback poster IS.
    if (!c.poster)
      throw new Error(
        `astro.config: showcase.json live channel "${c.id}" needs a "poster" — the no-JS/no-wasm/reduced-motion fallback image`
      );
    if (!demo(c.poster))
      throw new Error(
        `astro.config: showcase.json live channel "${c.id}" missing public/demos/${c.poster}`
      );
    for (const g of c.variantGroups ?? [])
      if (g.variantsRef !== 'themes' && g.variantsRef !== 'weather')
        throw new Error(
          `astro.config: showcase.json "${c.id}" variantGroups["${g.key}"] has unknown variantsRef "${g.variantsRef}" (expected "themes" or "weather")`
        );
  } else {
    throw new Error(`astro.config: showcase.json "${c.id}" has unknown kind "${c.kind}"`);
  }
}

// Studio Wall ↔ Features bridge: showcase.json channels and the features.json
// rows carrying `channel:` must be a bijection, or the dial silently shows an
// empty accordion / a feature silently has no home.
const features = /** @type {any[]} */ (
  JSON.parse(readFileSync(fileURLToPath(new URL('./src/features.json', import.meta.url)), 'utf8'))
);
const featureChannelOwner = new Map();
for (const f of features) {
  if (!f.channel) continue;
  if (featureChannelOwner.has(f.channel))
    throw new Error(
      `astro.config: features.json "${featureChannelOwner.get(f.channel)}" and "${f.name}" both claim channel "${f.channel}"`
    );
  featureChannelOwner.set(f.channel, f.name);
}
for (const c of showcase) {
  if (!featureChannelOwner.has(c.id))
    throw new Error(
      `astro.config: showcase.json channel "${c.id}" has no features.json row with channel:"${c.id}" — add one, or drop the channel`
    );
}
for (const [chId, name] of featureChannelOwner) {
  if (!scIds.has(chId))
    throw new Error(
      `astro.config: features.json "${name}" has channel:"${chId}" but showcase.json has no such channel — fix the id or drop channel`
    );
}

// Rewrite repo-relative markdown links to GitHub so they resolve once deployed.
function rehypeRepoLinks() {
  const repo = 'https://github.com/IvanWng97/pixtuoid/blob/main/';
  const DOC_DIR = 'docs'; // the rendered docs live in docs/ — links resolve from there
  const SCHEME = /^[a-z][a-z0-9+.-]*:/i;
  const DANGEROUS = /^\s*(?:javascript|data|vbscript):/i;
  /** @param {any} node */
  const walk = (node) => {
    if (node.tagName === 'a' && node.properties && typeof node.properties.href === 'string') {
      const href = node.properties.href;
      if (DANGEROUS.test(href)) {
        // defense-in-depth — the rendered doc is trusted today
        node.properties.href = '#';
      } else if (!href.startsWith('#') && !SCHEME.test(href)) {
        // resolve from docs/, clamp any climb above the repo root
        const joined = href.startsWith('/') ? href : posix.join(DOC_DIR, href);
        const rel = posix
          .normalize(joined)
          .replace(/^(?:\.\.\/)+/, '')
          .replace(/^\/+/, '');
        node.properties.href = repo + rel;
      }
    }
    (node.children || []).forEach(walk);
  };
  /** @param {any} tree */
  const transform = (tree) => walk(tree);
  return transform;
}

// Astro's built-in CSP (`security.csp` below) does NOT hash template-level
// `is:inline` scripts (verified vs Astro 7.0.5) — the only kind this site has —
// it appends style hashes (which make browsers IGNORE the 'unsafe-inline'
// Shiki/mermaid needs), and it emits its <meta> BELOW scripts the layout
// already wrote, which a meta policy does not govern. This hook closes all
// three from the BUILT html.
function cspInlineHashes() {
  return {
    name: 'csp-inline-hashes',
    hooks: {
      /** @param {{ dir: URL }} opts */
      'astro:build:done': ({ dir }) => {
        /** @type {string[]} */
        const htmlFiles = [];
        (function walk(/** @type {string} */ d) {
          for (const e of readdirSync(d, { withFileTypes: true })) {
            const p = join(d, e.name);
            if (e.isDirectory()) walk(p);
            else if (e.name.endsWith('.html')) htmlFiles.push(p);
          }
        })(fileURLToPath(dir));
        for (const file of htmlFiles) {
          const updated = rewriteCspMeta(readFileSync(file, 'utf8'));
          if (updated === null) {
            throw new Error(
              `csp-inline-hashes: no CSP <meta> found in ${file} — did security.csp get disabled?`
            );
          }
          writeFileSync(file, updated);
        }
      },
    },
  };
}

// The custom domain lives in the repo's Settings → Pages, not in the artifact —
// Actions deploys need no CNAME file.
export default defineConfig({
  site: 'https://pixtuoid.dev',
  base: '/',
  trailingSlash: 'ignore',
  // Astro 7's 'jsx' default drops the space between adjacent inline elements on
  // separate source lines, joining visible text ("pixtuoid v0.11.1" →
  // "pixtuoidv0.11.1"). Pin the Astro 6 behavior.
  compressHTML: true,
  markdown: {
    // excludeLangs keeps ```mermaid a RAW code node — the highlighter would
    // otherwise make it a <pre> before rehype-mermaid can make it an SVG. Prism
    // emits classes, not Shiki's inline style attributes, so it needs no CSP
    // style hash.
    syntaxHighlight: { type: 'prism', excludeLangs: ['mermaid'] },
    // Astro 7 deprecated the legacy `markdown.rehypePlugins` key (a hard error
    // without @astrojs/markdown-remark): opt back into the remark/rehype
    // pipeline explicitly — rehype-mermaid needs it.
    processor: unified({
      rehypePlugins: [
        // inline-svg: rendered at build time, so zero client JS and CSP-safe.
        [
          rehypeMermaid,
          {
            strategy: 'inline-svg',
            mermaidConfig: { theme: 'neutral', flowchart: { htmlLabels: true } },
          },
        ],
        rehypeRepoLinks, // after mermaid so it walks the final tree
        rehypeCallouts, // last: promotes doc blockquotes to terminal-window chrome
      ],
    }),
  },
  integrations: [sitemap(), cspInlineHashes()],
  // script-src carries NO 'unsafe-inline' — cspInlineHashes() above supplies the
  // is:inline hashes instead; 'wasm-unsafe-eval' permits WebAssembly.instantiate
  // for the live-office hero (wasm compilation ONLY, not JS eval). style-src
  // KEEPS 'unsafe-inline': Shiki spans, the mermaid SVG and a few style={} attrs
  // are inline STYLE ATTRIBUTES, which hashes cannot express. NOTE: security.csp
  // is build/preview-only by design — `astro dev` serves no CSP.
  security: {
    csp: {
      directives: [
        "default-src 'self'",
        "base-uri 'self'",
        "object-src 'none'",
        "img-src 'self'",
        "media-src 'self'",
        "font-src 'self'",
        "connect-src 'self'",
        "form-action 'self'",
      ],
      scriptDirective: { resources: ["'self'", "'wasm-unsafe-eval'"] },
      styleDirective: { resources: ["'self'", "'unsafe-inline'"] },
    },
  },
  prefetch: { prefetchAll: true, defaultStrategy: 'hover' },
  build: {
    // 'always', not the default 'auto': 'auto' inlines only sheets smaller than
    // assetsInlineLimit, pinned to 0 below, so it would inline NOTHING and every
    // page would render-block on two external CSS requests.
    inlineStylesheets: 'always',
  },
  vite: {
    define: {
      __PIXTUOID_VERSION__: JSON.stringify(version),
      __GH_STARS__: JSON.stringify(ghStars),
    },
    // Never inline assets as data: URLs. Vite's default inlining turned the small
    // @fontsource unicode-range subsets into data: fonts, which font-src 'self'
    // silently BLOCKED — keep the assets as files rather than adding `data:`.
    build: { assetsInlineLimit: 0 },
  },
});
