// Build-time GitHub star count. The site's CSP is `connect-src 'self'`, so the
// browser can't call api.github.com — we resolve the count once at build (CI
// rebuilds on every push keep it fresh) and bake it into the HTML. All failures
// (offline, 403 rate-limit, shape drift) degrade to null → the count just hides.
import { REPO } from '../consts';

const API = `https://api.github.com/repos/${REPO.replace('https://github.com/', '')}`;
const TIMEOUT_MS = 4000; // don't let a slow/hung API stall the whole build

async function fetchOnce(): Promise<number | null> {
  try {
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), TIMEOUT_MS);
    const res = await fetch(API, {
      headers: { Accept: 'application/vnd.github+json', 'User-Agent': 'pixtuoid-site-build' },
      signal: ctrl.signal,
    });
    clearTimeout(timer);
    if (!res.ok) return null;
    const data: unknown = await res.json();
    const n = (data as { stargazers_count?: unknown }).stargazers_count;
    return typeof n === 'number' && Number.isFinite(n) ? n : null;
  } catch {
    return null;
  }
}

// Memoize the PROMISE so concurrent frontmatter callers (Hero, Footer) share a
// single network round-trip rather than racing to fetch it each.
let inflight: Promise<number | null> | undefined;
export function getStars(): Promise<number | null> {
  if (!inflight) inflight = fetchOnce();
  return inflight;
}

// 1234 → "1.2k", 12345 → "12k", 999 → "999" — compact, like a repo badge.
export function formatStars(n: number): string {
  if (n < 1000) return String(n);
  const k = n / 1000;
  return (n >= 10000 ? Math.round(k) : Math.round(k * 10) / 10) + 'k';
}
