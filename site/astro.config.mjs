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
// actually serve — not main's Cargo.toml, which runs AHEAD of the released
// version between a mid-cycle bump and its release tag (see
// config/released-version.mjs). The Cargo.toml parse survives as the
// FALLBACK for tag-less builds (pages.yml/site.yml fetch full history so
// real deploys never take it). Scope the match to the [workspace.package]
// table so a dependency's line-anchored `version = "…"` (in a
// [dependencies.x] sub-table) can't be picked up — and throw rather than
// silently shipping a bogus fallback if the parse ever fails.
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

// Build-time star count (§2): baked like the version — the CSP forbids a
// runtime fetch. null on any failure; consumers omit the count (never fail).
const ghStars = await fetchStarCount();

// (The old themes.json→theme_<id>.png and weather.json→weather_<id>.png
// demo-still guards were removed with the static THEMES/WEATHER channels (#468):
// the live VIBING channel renders those in-canvas, so no per-id still exists.)

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
  // Channel-level variantsRef is RETIRED (#468 deleted its theme_<id>.png /
  // weather_<id>.png stills). A stray one on ANY kind would resolve those dead
  // stills → broken <img>s, so reject it before the per-kind checks — variant-set
  // channels use inline `variants`, live channels use per-group variantGroups (audit C11).
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
    // (channel-level variantsRef already rejected above, audit C11 — variant-set
    // channels supply inline `variants`.)
    if (!(c.variants && c.variants.length))
      throw new Error(`astro.config: showcase.json variant-set "${c.id}" has no "variants"`);
    for (const v of c.variants)
      if (!demo(v.src))
        throw new Error(
          `astro.config: showcase.json "${c.id}" variant "${v.id}" missing public/demos/${v.src}`
        );
  } else if (c.kind === 'live') {
    // A `live` channel is rendered by the wasm office canvas, not static demo
    // assets — no asset/w/h required. The fallback poster IS required (it's the
    // no-JS/no-wasm/reduced-motion image); the old guard only validated it when
    // present, so a live channel omitting it shipped a blank stage (audit C14).
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

// Studio Wall ↔ Features bridge: features.json is the total feature
// collection, and a row with a video/live demo channel carries
// `channel: "<showcase id>"` (consts.ts). The dial + its accordion desc and
// the below-stage roster are ONE collection partitioned by that field, so
// the two manifests must agree on a bijection — every showcase.json channel
// has exactly one features.json row claiming it, and vice versa — or the
// dial silently shows an empty accordion / a feature silently has no home.
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

// CSP, part 2 of 2 (part 1 is `security.csp` below): Astro 7's built-in CSP
// emits the <meta> on every page and owns the RESOURCE lists, but (verified vs
// 7.0.5) it does NOT hash template-level `is:inline` scripts — the only kind
// this site has — and it unconditionally appends style hashes, which would make
// browsers IGNORE the 'unsafe-inline' that Shiki/mermaid style attributes need.
// This build:done hook closes both gaps from the BUILT html itself, so the
// hashes are mechanically derived and can never drift from the content
// (script-src re-hashed per page, style-src stripped of hashes). The delicate
// per-page HTML→hashes→rewrite transform lives in ./config/csp-hashes.mjs so it
// is unit-testable (config/csp-hashes.test.mjs) and its script-tag scan can't
// diverge from the HTML tokenizer; this hook only walks dist/ and applies it.
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

// Custom domain → https://pixtuoid.dev/ (the domain lives in the repo's
// Settings → Pages, not in the artifact — Actions deploys need no CNAME file).
// GitHub redirects the old project page ivanwng97.github.io/pixtuoid/ here.
export default defineConfig({
  site: 'https://pixtuoid.dev',
  base: '/',
  trailingSlash: 'ignore',
  // Astro 7 flipped the default to 'jsx' (JSX-rule whitespace stripping), which
  // drops the space between adjacent inline elements on separate source lines —
  // measured: dozens of visible-text joins across every page ("pixtuoid v0.11.1"
  // → "pixtuoidv0.11.1", boot lines, docs prose). Pin the Astro 6 behavior.
  compressHTML: true,
  markdown: {
    // keep ```mermaid as a RAW code node — Shiki would otherwise highlight it
    // into a <pre> before rehype-mermaid can turn it into an inline SVG.
    // (syntaxHighlight stays a top-level markdown option in Astro 7 — the
    // vite-plugin passes it into whichever processor is active.)
    syntaxHighlight: { type: 'shiki', excludeLangs: ['mermaid'] },
    // Astro 7: Sätteri is the default Markdown processor and the legacy
    // `markdown.rehypePlugins` key is deprecated (hard error without
    // @astrojs/markdown-remark installed). Opt back into the remark/rehype
    // pipeline explicitly — rehype-mermaid needs it.
    processor: unified({
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
        rehypeCallouts, // last: promote doc blockquotes to terminal-window chrome (§6)
      ],
    }),
  },
  // Sitemap respects `site` + `base` → https://pixtuoid.dev/sitemap-index.xml.
  // robots.txt (prerendered by src/pages/robots.txt.ts) serves at the origin
  // root on the custom domain, so crawlers pick the sitemap up from it directly.
  integrations: [sitemap(), cspInlineHashes()],
  // Prefetch same-site links on hover — the docs pages are tiny static HTML,
  // so hover-to-tap latency covers the fetch. The injected prefetch client is
  // a BUNDLED external script (script-src 'self' covers it under the CSP).
  // CSP, part 1 of 2: Astro's built-in CSP emits the <meta> into EVERY page's
  // head (404 included) and owns the resource lists — the one thing it can't
  // compute for this site is the is:inline script hashes, which the
  // cspInlineHashes() integration above derives from the built html.
  // script-src carries NO 'unsafe-inline' (hashes instead — the point of the
  // exercise); 'wasm-unsafe-eval' permits WebAssembly.instantiate for the
  // live-office hero (wasm compilation ONLY, not JS eval). style-src KEEPS
  // 'unsafe-inline': Shiki spans, the mermaid inline SVG, and a few style={}
  // attributes are inline STYLE ATTRIBUTES, which hashes cannot express (and
  // any present hash would make browsers ignore 'unsafe-inline').
  // frame-ancestors and SRI need HTTP headers GitHub Pages can't set. NOTE:
  // security.csp is build/preview-only by design — `astro dev` serves no CSP.
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
    // ALWAYS inline the page stylesheets. Astro's default 'auto' inlines only
    // sheets smaller than vite's assetsInlineLimit — pinned to 0 below for the
    // CSP-font posture — so 'auto' would inline NOTHING and every page would
    // render-block on two external CSS requests (~592ms RTT on simulated
    // mobile, on the FCP/LCP critical path). 'always' bypasses assetsInlineLimit
    // entirely; woff2 fonts are not stylesheets so they stay hashed files under
    // font-src 'self', and inline <style> is allowed (style-src keeps
    // 'unsafe-inline', kept hash-free by the cspInlineHashes hook).
    inlineStylesheets: 'always',
  },
  vite: {
    define: {
      __PIXTUOID_VERSION__: JSON.stringify(version),
      __GH_STARS__: JSON.stringify(ghStars),
    },
    // Never inline assets as data: URLs. Vite's default 4KiB inlining turned
    // the small @fontsource unicode-range subsets into data: fonts, which the
    // hand-rolled CSP (font-src 'self', Base.astro) silently BLOCKED in
    // production — caught by the e2e suite's console-error watchdog. Keeping
    // the CSP strict and the assets as files is the fix, not `data:` in CSP.
    build: { assetsInlineLimit: 0 },
  },
});
