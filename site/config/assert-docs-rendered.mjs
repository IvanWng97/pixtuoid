// Assert every rendered doc page actually has a body, and that /architecture kept
// its mermaid <svg>. Catches the silent rehype-mermaid empty-render class: a
// headless-Chromium/Playwright version hiccup collapses a doc's <Content /> to an
// EMPTY <article> WITHOUT failing `astro build` — it shipped an empty /architecture
// once (the deploy build's pnpm fallback pulled a Playwright that didn't match the
// installed Chromium). GENERIC on purpose: it globs every page carrying the Docs
// layout's `<article class="prose">` (config / architecture / contributing /
// knowledge-base / parallel-delivery + any future doc), so there's no per-page or
// per-heading string to drift — the exact "hardcoded grep" this replaces.
//
// Runs in TWO places off ONE source: `npm run check:docs` in `verify` (site-check,
// the host build — catches content/mermaid-syntax regressions) AND in pages.yml
// after the deploy build (catches deploy-ENV failures the host build can't repro).
//
// Usage: node config/assert-docs-rendered.mjs [distDir=dist]
import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import process from 'node:process';

const dist = process.argv[2] ?? 'dist';
// Real doc bodies are ~8k–18k chars of stripped text; a collapsed render is ~4. A
// generous floor flags the failure class without coupling to any page's real size.
const MIN_BODY_CHARS = 500;
// \bprose\b (not a sole-class match) so a future `class="prose max-w-none"` or an
// appended Astro scoped class doesn't drop the article → redden every deploy.
const DOC_ARTICLE = /<article class="[^"]*\bprose\b[^"]*"[^>]*>([\s\S]*?)<\/article>/;

const failures = [];
let docPages = 0;

// Doc pages render to dist/<route>/index.html (one level deep). Non-doc dirs
// (_astro, demos, wasm, …) simply won't carry the prose article and are skipped.
for (const entry of readdirSync(dist, { withFileTypes: true })) {
  if (!entry.isDirectory()) continue;
  const file = join(dist, entry.name, 'index.html');
  if (!existsSync(file)) continue;
  const m = readFileSync(file, 'utf8').match(DOC_ARTICLE);
  if (!m) continue; // not a Docs-layout page
  docPages += 1;
  const body = m[1]
    .replace(/<[^>]+>/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
  if (body.length < MIN_BODY_CHARS) {
    failures.push(
      `/${entry.name}: doc body only ${body.length} chars (< ${MIN_BODY_CHARS}) — render collapsed`
    );
  }
  if (entry.name === 'architecture' && !/<svg[\s>]/.test(m[1])) {
    failures.push('/architecture: no inline <svg> — the mermaid diagram did not render');
  }
}

if (docPages === 0) {
  failures.push(`no doc pages found under ${dist}/ (the 'article.prose' selector drifted?)`);
}

if (failures.length > 0) {
  console.error(`✗ doc-render check FAILED (${dist}):\n  ${failures.join('\n  ')}`);
  process.exit(1);
}
console.log(`✓ doc-render check: ${docPages} doc pages have a body; /architecture <svg> present`);
