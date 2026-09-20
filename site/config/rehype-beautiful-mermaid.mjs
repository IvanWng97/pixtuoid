// ```mermaid → inline <svg> at build time, with no browser: beautiful-mermaid
// lays the graph out in-process, so the site build needs neither Chromium nor a
// Playwright-version match to render /architecture. The SVG's colors are the
// page's own --bg/--fg tokens, so one render follows every theme.
import { renderMermaidSVG } from 'beautiful-mermaid';
import { fromHtml } from 'hast-util-from-html';

// Mermaid's accessibility directives are not part of beautiful-mermaid's grammar
// (it draws them as nodes), so they are lifted out here and put back as the
// <title>/<desc> the SVG is labelled by.
const ACC_DIRECTIVE = /^\s*acc(Title|Descr)\s*:\s*(.*?)\s*$/;

// The library's stylesheet opens with an @import of Google Fonts: the site's CSP
// (style-src/font-src 'self') blocks it, the e2e console watchdog reds on the
// block, and the text falls to the same system-ui either way.
const FONT_IMPORT = /@import\s+url\([^)]*\)[^;]*;\s*/g;

// The root re-declares --bg/--fg; bound to the page's tokens of the same name
// that is a self-referencing custom property, which is invalid and collapses
// every color-mix() to black. Dropped, the library's var(--bg)/var(--fg)
// references inherit the page's values directly.
const ROOT_TOKENS = /--(bg|fg)\s*:[^;]*;?/g;

function splitAccessibility(source) {
  const acc = {};
  const lines = source.split('\n').filter((line) => {
    const m = ACC_DIRECTIVE.exec(line);
    if (!m) return true;
    acc[m[1]] = m[2];
    return false;
  });
  return { source: lines.join('\n'), title: acc.Title, descr: acc.Descr };
}

function mermaidSource(node) {
  if (node.type !== 'element' || node.tagName !== 'pre') return null;
  const code = node.children.find((c) => c.type === 'element' && c.tagName === 'code');
  if (!code || !(code.properties?.className ?? []).includes('language-mermaid')) return null;
  return code.children.map((c) => c.value ?? '').join('');
}

function inheritPageTokens(svg) {
  const style = String(svg.properties.style ?? '')
    .replace(ROOT_TOKENS, '')
    .trim();
  if (style) svg.properties.style = style;
  else delete svg.properties.style;
  for (const sheet of svg.children.filter((c) => c.tagName === 'style')) {
    for (const text of sheet.children) text.value = text.value.replace(FONT_IMPORT, '');
  }
  return svg;
}

function labelled(svg, index, title, descr) {
  const id = `diagram-${index}`;
  const label = (tagName, value) => ({
    type: 'element',
    tagName,
    properties: { id: `${id}-${tagName}` },
    children: [{ type: 'text', value }],
  });
  const labels = [];
  if (title) labels.push(label('title', title));
  if (descr) labels.push(label('desc', descr));
  if (labels.length === 0) return svg;
  svg.children.unshift(...labels);
  svg.properties.role = 'img';
  svg.properties.ariaLabelledBy = labels.map((l) => l.properties.id).join(' ');
  return svg;
}

function render(raw, index) {
  const { source, title, descr } = splitAccessibility(raw);
  const html = renderMermaidSVG(source, { transparent: true });
  const svg = fromHtml(html, { fragment: true }).children.find(
    (c) => c.type === 'element' && c.tagName === 'svg'
  );
  if (!svg) throw new Error('beautiful-mermaid returned no <svg>');
  return labelled(inheritPageTokens(svg), index, title, descr);
}

export default function rehypeBeautifulMermaid() {
  return (tree) => {
    let index = 0;
    const walk = (node) => {
      if (!node.children) return;
      node.children = node.children.map((child) => {
        const source = mermaidSource(child);
        if (source === null) {
          walk(child);
          return child;
        }
        return render(source, index++);
      });
    };
    walk(tree);
  };
}
