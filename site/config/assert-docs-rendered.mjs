// Assert every rendered doc page actually has a body, and that /architecture kept
// its mermaid <svg>. Catches the silent rehype-mermaid empty-render class: a
// headless-Chromium/Playwright version hiccup collapses a doc's <Content /> to an
// EMPTY <article> WITHOUT failing `astro build` — it shipped an empty /architecture
// once (the deploy build's pnpm fallback pulled a Playwright that didn't match the
// installed Chromium). GENERIC on purpose: it globs every page carrying the Docs
// layout's `<article class="prose">` (config / architecture / contributing /
// knowledge-base / parallel-delivery + any future doc), so there's no per-page or
// per-heading string to drift.
//
// Runs off ONE source in THREE places: `check:docs` in `verify` (local site-check),
// as a step in site.yml (the PR-gating CI, on the host build — catches content /
// mermaid-syntax regressions pre-merge), AND in pages.yml's `build-cmd` (catches
// deploy-ENV failures the host build can't repro).
//
// The pure `checkDocPages()` core is unit-tested (assert-docs-rendered.test.mjs,
// mirroring the config/*.mjs pure-fn + test split); the CLI below is a thin wrapper.
//
// Usage: node config/assert-docs-rendered.mjs [distDir=dist]
import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import process from 'node:process';

// Real doc bodies are ~8k–18k chars of stripped text; a collapsed render is ~4. A
// generous floor flags the failure class without coupling to any page's real size.
export const MIN_BODY_CHARS = 500;
// \bprose\b (not a sole-class match) so a future `class="prose max-w-none"` or an
// appended Astro scoped class doesn't drop the article → redden every deploy.
const DOC_ARTICLE = /<article class="[^"]*\bprose\b[^"]*"[^>]*>([\s\S]*?)<\/article>/;

/**
 * Pure core (unit-tested): given the doc-page entries `[{ name, html }]`, return the
 * failures + how many Docs-layout pages were seen. No fs / process — testable
 * without a real `dist/`.
 */
export function checkDocPages(entries) {
  const failures = [];
  let docPages = 0;
  for (const { name, html } of entries) {
    const m = html.match(DOC_ARTICLE);
    if (!m) continue; // not a Docs-layout page
    docPages += 1;
    const body = m[1]
      .replace(/<[^>]+>/g, ' ')
      .replace(/\s+/g, ' ')
      .trim();
    if (body.length < MIN_BODY_CHARS) {
      failures.push(
        `/${name}: doc body only ${body.length} chars (< ${MIN_BODY_CHARS}) — render collapsed`
      );
    }
    if (name === 'architecture' && !/<svg[\s>]/.test(m[1])) {
      failures.push('/architecture: no inline <svg> — the mermaid diagram did not render');
    }
  }
  if (docPages === 0) {
    failures.push("no doc pages found (the 'article.prose' selector drifted?)");
  }
  return { failures, docPages };
}

// Thin CLI wrapper: read dist/<route>/index.html into entries (doc pages are one
// level deep; non-doc dirs _astro/demos/wasm lack the prose article and drop out),
// run the core, exit non-zero on any failure so CI / the deploy reddens.
function main(dist) {
  const entries = readdirSync(dist, { withFileTypes: true })
    .filter((e) => e.isDirectory())
    .map((e) => ({ name: e.name, file: join(dist, e.name, 'index.html') }))
    .filter((e) => existsSync(e.file))
    .map((e) => ({ name: e.name, html: readFileSync(e.file, 'utf8') }));
  const { failures, docPages } = checkDocPages(entries);
  if (failures.length > 0) {
    console.error(`✗ doc-render check FAILED (${dist}):\n  ${failures.join('\n  ')}`);
    process.exit(1);
  }
  console.log(`✓ doc-render check: ${docPages} doc pages have a body; /architecture <svg> present`);
}

// Run the CLI only when invoked directly (`node config/assert-docs-rendered.mjs`),
// so the test can import checkDocPages without triggering the fs read + process.exit.
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv[2] ?? 'dist');
}
