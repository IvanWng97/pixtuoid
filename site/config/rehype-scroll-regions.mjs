// A doc's code blocks and tables scroll sideways inside themselves on a narrow
// screen (global.css), and Safari never makes a scroller a tab stop, so their
// overflow is out of a keyboard's reach (axe `scrollable-region-focusable`;
// Deque: "always put a tabindex of 0 on the scrollable region"). Each becomes a
// named region: a generic element prohibits `aria-label`. A table gets a
// wrapper instead of the role, which would replace its table semantics.

function textOf(node) {
  if (node.type === 'text') return node.value;
  return (node.children || []).map(textOf).join('');
}

const region = (ariaLabel) => ({ tabIndex: 0, role: 'region', ariaLabel });

function codeLabel(pre) {
  const classes = [pre, ...(pre.children || [])].flatMap((n) => n.properties?.className || []);
  const lang = classes.find((c) => String(c).startsWith('language-'));
  return lang ? `${String(lang).slice('language-'.length)} code` : 'code';
}

function tableLabel(table) {
  const head = (table.children || []).find((n) => n.tagName === 'thead');
  const row = head?.children?.find((n) => n.tagName === 'tr');
  const cells = (row?.children || [])
    .filter((n) => n.tagName === 'th')
    .map((n) => textOf(n).trim())
    .filter(Boolean);
  return cells.length ? `table: ${cells.join(', ')}` : 'table';
}

function walk(node) {
  const children = node.children || [];
  children.forEach((child, i) => {
    if (child.type !== 'element') return;
    if (child.tagName === 'pre') {
      Object.assign((child.properties ||= {}), region(codeLabel(child)));
    } else if (child.tagName === 'table') {
      children[i] = {
        type: 'element',
        tagName: 'div',
        properties: { className: ['table-scroll'], ...region(tableLabel(child)) },
        children: [child],
      };
    } else {
      walk(child);
    }
  });
}

export default function rehypeScrollRegions() {
  return walk;
}
