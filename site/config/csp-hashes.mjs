// astro.config.mjs's astro:build:done hook walks dist/ and calls
// rewriteCspMeta() per page.
import { createHash } from 'node:crypto';

const HASH = /^'sha(256|384|512)-/;

// Quote-aware opening tag: it ends at the first `>` that is NOT inside a quoted
// attribute value, so `data-x="a>b"` can't truncate it and hash the wrong bytes
// (a CSP block in production only). The end tag must match everything a browser
// treats as a script close, including the parser-error forms `</script >` and
// `</script foo="bar">` — leaving content past a fake-strict `</script>`
// unhashed is the CodeQL js/bad-tag-filter primitive.
const SCRIPT_RE = /<script\b((?:[^>"']|"[^"]*"|'[^']*')*)>([\s\S]*?)<\/script[^>]*>/gi;

// A real `src` ATTRIBUTE (external script → rides 'self', no hash). Quoted
// values are stripped first so a `src=` inside another attribute's VALUE can't
// be mistaken for the attribute; the `(?:^|\s)` boundary keeps `data-src=` out.
function hasSrcAttr(attrs) {
  return /(?:^|\s)src\s*=/i.test(attrs.replace(/"[^"]*"|'[^']*'/g, ''));
}

/**
 * The set of `'sha256-…'` tokens for every inline <script> in `html`.
 * @param {string} html
 * @returns {Set<string>}
 */
export function inlineScriptHashes(html) {
  const hashes = new Set();
  for (const m of html.matchAll(SCRIPT_RE)) {
    if (hasSrcAttr(m[1] ?? '')) continue;
    hashes.add(`'sha256-${createHash('sha256').update(m[2], 'utf8').digest('base64')}'`);
  }
  return hashes;
}

// The whole CSP element, not just its content attribute: it is RELOCATED as well
// as rewritten. Depends on Astro rendering the attributes in this fixed order.
const CSP_META_RE = /<meta http-equiv="content-security-policy" content="([^"]*)"\s*\/?>/i;

// Where the policy is re-anchored. A `<meta http-equiv>` CSP governs only the
// content that FOLLOWS it, and Astro emits it after whatever `<script>`/`<style>`
// the layout wrote above the head-injection point — which therefore ran
// unpoliced. Anchoring on the charset rather than `<head>` keeps `<meta charset>`
// inside the first 1024 bytes the encoding sniffer reads; the policy's hashes
// would otherwise push it out.
const CHARSET_RE = /<meta[^>]*\scharset\s*=[^>]*>/i;
const HEAD_OPEN_RE = /<head\b[^>]*>/i;

/**
 * Rewrite the CSP <meta> and hoist it above every script and style it governs.
 * ALL style-src hashes are stripped so the configured 'unsafe-inline' stays
 * honored — one present hash disables it for the whole directive.
 * @param {string} html
 * @returns {string | null} the rewritten html, or null if no CSP <meta> exists
 * @throws if a CSP <meta> exists but the document has no charset/<head> anchor
 */
export function rewriteCspMeta(html) {
  const found = html.match(CSP_META_RE);
  if (!found || found.index === undefined) return null;
  const hashes = inlineScriptHashes(html);
  const directives = found[1]
    .split(';')
    .map((d) => {
      const toks = d.trim().split(/\s+/).filter(Boolean);
      if (toks[0] !== 'script-src' && toks[0] !== 'style-src') return d.trim();
      const resources = toks.slice(1).filter((t) => !HASH.test(t));
      const add = toks[0] === 'script-src' ? [...hashes] : [];
      return [toks[0], ...resources, ...add].join(' ');
    })
    .filter(Boolean)
    .join('; ');

  const stripped = html.slice(0, found.index) + html.slice(found.index + found[0].length);
  const anchor = stripped.match(CHARSET_RE) ?? stripped.match(HEAD_OPEN_RE);
  if (!anchor || anchor.index === undefined) {
    throw new Error('csp-hashes: no charset/<head> anchor to hoist the CSP <meta> to');
  }
  const at = anchor.index + anchor[0].length;
  const meta = `<meta http-equiv="content-security-policy" content="${directives}">`;
  return stripped.slice(0, at) + meta + stripped.slice(at);
}
