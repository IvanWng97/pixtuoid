// @ts-check
import { defineConfig } from 'astro/config';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// Single-source the displayed version from the workspace Cargo.toml so the boot
// intro never goes stale on a release bump.
const cargoToml = readFileSync(fileURLToPath(new URL('../Cargo.toml', import.meta.url)), 'utf8');
const version = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1] ?? '0.0.0';

// Project page → https://ivanwng97.github.io/pixtuoid/
// If a custom domain is later added, set base back to '/' (and update CNAME).
export default defineConfig({
  site: 'https://ivanwng97.github.io',
  base: '/pixtuoid',
  trailingSlash: 'ignore',
  vite: { define: { __PIXTUOID_VERSION__: JSON.stringify(version) } },
});
