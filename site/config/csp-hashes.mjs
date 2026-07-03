// The CSP hash-rewrite kernel, extracted from astro.config.mjs so its delicate
// string surgery is unit-testable (config/csp-hashes.test.mjs) and can't
// silently diverge from the HTML tokenizer. astro.config.mjs's astro:build:done
// hook walks dist/ and calls rewriteCspMeta() per page; the emitted policy is
// byte-identical to the previous inline implementation on every current page
// (proven by the e2e console-error watchdog against the production build). Only
// the pathological future inputs the hardened regex now handles differ.
import { createHash } from 'node:crypto';

const HASH = /^'sha(256|384|512)-/;

// Quote-aware opening tag: it ends at the first `>` that is NOT inside a quoted
// attribute value. The old `<script(\s[^>]*)?>` truncated on `data-x="a>b"` and
// hashed the wrong bytes, so a legal is:inline script would be CSP-blocked in
// production only. Group 1 = the raw attribute list; group 2 = the exact bytes
// between the tags (what the browser hashes). The end tag matches everything a
// browser treats as a script close — not just `</script>` but the parser-error
// forms `</script >`, `</script\n>`, and `</script foo="bar">` (a browser ends
// the script there anyway). An unmatched close would silently drop that
// script's hash → a prod-only CSP block, and leaving content past a fake-strict
// `</script>` unhashed is the CodeQL js/bad-tag-filter primitive; `[^>]*`
// swallows the garbage up to the real `>`.
const SCRIPT_RE = /<script\b((?:[^>"']|"[^"]*"|'[^']*')*)>([\s\S]*?)<\/script[^>]*>/gi;

// A real `src` ATTRIBUTE (external script → rides 'self', no hash). Strip quoted
// values first so a `src=` sitting inside another attribute's VALUE (e.g.
// data-cmd="ffmpeg src=in.mp4") can't be mistaken for the attribute; the
// `(?:^|\s)` boundary keeps `data-src=` from matching `src=`.
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

/**
 * Rewrite the CSP <meta>: re-derive script-src's inline-script hashes from the
 * built HTML and strip ALL style-src hashes so the configured 'unsafe-inline'
 * stays honored (one present hash disables it for the whole directive).
 * @param {string} html
 * @returns {string | null} the rewritten html, or null if no CSP <meta> exists
 */
export function rewriteCspMeta(html) {
  const hashes = inlineScriptHashes(html);
  let rewrote = false;
  const updated = html.replace(
    /(<meta http-equiv="content-security-policy" content=")([^"]*)(")/i,
    (_, pre, /** @type {string} */ content, post) => {
      rewrote = true;
      const out = content
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
      return pre + out + post;
    }
  );
  return rewrote ? updated : null;
}
