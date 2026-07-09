// Markdown callouts → terminal-window chrome (spec §6): every top-level
// blockquote in a rendered doc becomes a small terminal window — "~ note"
// dark chrome by default, a red-dot "~ warning" window when the quote OPENS
// with an imperative-warning strong (**Don't …** / **Never …** / **Warning**).
// Pure hast transform, unit-tested by rehype-callouts.test.mjs and registered
// in astro.config.mjs's processor (the csp-hashes.mjs co-location pattern).
// The red dot reuses global.css's .terminal__dot--r — no second red literal.

const WARN_RE = /^(don'?t|never|warning|danger|caution)\b/i;

function textOf(node) {
  if (!node) return '';
  if (node.type === 'text') return node.value;
  return (node.children || []).map(textOf).join('');
}

// The FIRST <strong> in document order — CONTRIBUTING's "**Don't chain …**"
// idiom puts the imperative there; a plain editorial quote has none → note.
function firstStrongText(node) {
  if (node.type === 'element' && node.tagName === 'strong') return textOf(node);
  for (const c of node.children || []) {
    if (c.type !== 'element') continue;
    const t = firstStrongText(c);
    if (t) return t;
  }
  return '';
}

function calloutFor(blockquote) {
  // Astro's default remark-smartypants has already turned a straight "'" into
  // U+2019 by the time this hast transform runs — normalize back so "Don't"
  // classifies the same whether the source markdown typed it straight or not.
  const opener = firstStrongText(blockquote).trim().replace(/’/g, "'");
  const warn = WARN_RE.test(opener);
  const kind = warn ? 'warn' : 'note';
  const barChildren = [];
  if (warn) {
    barChildren.push({
      type: 'element',
      tagName: 'span',
      properties: { className: ['terminal__dot', 'terminal__dot--r'] },
      children: [],
    });
  }
  barChildren.push({
    type: 'element',
    tagName: 'span',
    properties: { className: ['callout__title'] },
    children: [{ type: 'text', value: warn ? '~ warning' : '~ note' }],
  });
  return {
    type: 'element',
    tagName: 'div',
    properties: { className: ['callout', `callout--${kind}`] },
    children: [
      {
        type: 'element',
        tagName: 'div',
        properties: { className: ['callout__bar'], ariaHidden: 'true' },
        children: barChildren,
      },
      {
        ...blockquote,
        properties: { ...(blockquote.properties || {}), className: ['callout__body'] },
      },
    ],
  };
}

export default function rehypeCallouts() {
  return function transform(tree) {
    (function walk(node) {
      const kids = node.children || [];
      for (let i = 0; i < kids.length; i++) {
        const c = kids[i];
        if (c.type !== 'element') continue;
        if (c.tagName === 'blockquote') {
          // wrap and DON'T descend — a nested quote stays a plain quote
          kids[i] = calloutFor(c);
          continue;
        }
        walk(c);
      }
    })(tree);
  };
}
