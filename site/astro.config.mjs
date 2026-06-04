// @ts-check
import { defineConfig } from 'astro/config';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// Single-source the displayed version from the workspace Cargo.toml so the boot
// intro never goes stale on a release bump.
const cargoToml = readFileSync(fileURLToPath(new URL('../Cargo.toml', import.meta.url)), 'utf8');
const version = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1] ?? '0.0.0';

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
