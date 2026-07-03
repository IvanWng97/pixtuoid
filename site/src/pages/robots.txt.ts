import type { APIRoute } from 'astro';

// robots.txt, prerendered to dist/robots.txt. The sitemap URL is DERIVED from
// the one canonical origin (`site` in astro.config.mjs) + BASE_URL, so a
// custom-domain move (README "Custom domain") can't leave a stale hardcoded
// origin behind. Note the project-page caveat: GitHub Pages serves this at
// /pixtuoid/robots.txt, while crawlers only fetch robots.txt from the ORIGIN
// root — it becomes fully authoritative the day the site moves to its own
// domain (base '/'); until then the sitemap is submitted directly to Search
// Console (see the sitemap() note in astro.config.mjs).
export const GET: APIRoute = ({ site }) => {
  const base = import.meta.env.BASE_URL.replace(/\/$/, '');
  const sitemap = new URL(`${base}/sitemap-index.xml`, site);
  return new Response(`User-agent: *\nAllow: /\n\nSitemap: ${sitemap.href}\n`, {
    headers: { 'Content-Type': 'text/plain; charset=utf-8' },
  });
};
