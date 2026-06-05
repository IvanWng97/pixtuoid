// @ts-check
import { defineConfig } from 'astro/config';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

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

// Rewrite repo-relative links in rendered markdown (e.g. ../crates/...) to GitHub
// so docs/CONFIGURATION.md's links resolve on the deployed site.
function rehypeRepoLinks() {
  const repo = 'https://github.com/IvanWng97/pixtuoid/blob/main/';
  /** @param {any} node */
  const walk = (node) => {
    if (
      node.tagName === 'a' &&
      node.properties &&
      typeof node.properties.href === 'string' &&
      node.properties.href.startsWith('../')
    ) {
      node.properties.href = repo + node.properties.href.replace(/^(\.\.\/)+/, '');
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
  markdown: { rehypePlugins: [rehypeRepoLinks] },
  vite: { define: { __PIXTUOID_VERSION__: JSON.stringify(version) } },
});
