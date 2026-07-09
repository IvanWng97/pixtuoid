import type { APIRoute } from 'astro';

// robots.txt, prerendered to dist/robots.txt. The sitemap URL is DERIVED from
// the one canonical origin (`site` in astro.config.mjs) + BASE_URL, so a
// custom-domain move (README "Custom domain") can't leave a stale hardcoded
// origin behind. On pixtuoid.dev (base '/') this serves at the ORIGIN root —
// the only place crawlers fetch robots.txt from — so it is authoritative and
// crawlers discover the sitemap through it. (On the old /pixtuoid/ project
// page it was unreachable and the sitemap went to Search Console directly.)
export const GET: APIRoute = ({ site }) => {
  const base = import.meta.env.BASE_URL.replace(/\/$/, '');
  const sitemap = new URL(`${base}/sitemap-index.xml`, site);
  return new Response(`User-agent: *\nAllow: /\n\nSitemap: ${sitemap.href}\n`, {
    headers: { 'Content-Type': 'text/plain; charset=utf-8' },
  });
};
