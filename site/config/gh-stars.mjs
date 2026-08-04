// Build-time GitHub star count → the `__GH_STARS__` vite define. The hash-CSP
// forbids runtime fetches, so this runs ONCE at build in astro.config.mjs — and
// an offline/local build must never fail on it: every error path yields null.
import process from 'node:process';

const API = 'https://api.github.com/repos/IvanWng97/pixtuoid';
// Shared with Statusline.astro's PR-feed fetch, so an offline/slow CI runner
// fails every build-time GitHub call the same way.
export const GH_FETCH_TIMEOUT_MS = 5000;

/**
 * @param {typeof fetch} [fetchImpl]
 * @param {string | undefined} [token]
 * @returns {Promise<string | null>} the count as a string ("342"), or null
 */
export async function fetchStarCount(fetchImpl = fetch, token = process.env.GITHUB_TOKEN) {
  // astro.config.mjs calls this with no seam to inject a fetchImpl stub, so an
  // e2e build needing a deterministic count substitutes it here instead, no
  // network. Set-but-empty behaves as unset (repo convention, e.g. RUST_LOG=).
  if (process.env.GH_STARS_OVERRIDE) return process.env.GH_STARS_OVERRIDE;
  try {
    /** @type {Record<string, string>} */
    const headers = { accept: 'application/vnd.github+json' };
    if (token) headers.authorization = `Bearer ${token}`;
    const res = await fetchImpl(API, { headers, signal: AbortSignal.timeout(GH_FETCH_TIMEOUT_MS) });
    if (!res.ok) return null;
    const repo = await res.json();
    const n = repo?.stargazers_count;
    return Number.isFinite(n) ? String(n) : null;
  } catch {
    return null;
  }
}
