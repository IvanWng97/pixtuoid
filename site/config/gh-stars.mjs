// Build-time GitHub star count → the `__GH_STARS__` vite define (the ★ chips
// and the 2F star plaque). The hash-CSP forbids runtime fetches, so this runs
// ONCE at build in astro.config.mjs — and an offline/local build must never
// fail on it: every error path yields null (consumers omit the count).
// Kernel extracted for unit tests (config/gh-stars.test.mjs), the
// csp-hashes.mjs posture.
import process from 'node:process';

const API = 'https://api.github.com/repos/IvanWng97/pixtuoid';
// Shared with Statusline.astro's PR-feed fetch — ONE bound for every build-time
// GitHub API call, so an offline/slow CI runner fails both the same way.
export const GH_FETCH_TIMEOUT_MS = 5000;

/**
 * @param {typeof fetch} [fetchImpl]
 * @param {string | undefined} [token]
 * @returns {Promise<string | null>} the count as a string ("342"), or null
 */
export async function fetchStarCount(fetchImpl = fetch, token = process.env.GITHUB_TOKEN) {
  // astro.config.mjs calls this with no override seam, so an e2e build that
  // needs a deterministic non-null count can't inject a fetchImpl stub —
  // GH_STARS_OVERRIDE substitutes the count directly, verbatim, no network.
  // Set-but-empty behaves as unset (repo convention, e.g. RUST_LOG=).
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
