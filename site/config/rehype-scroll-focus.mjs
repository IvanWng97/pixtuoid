// Every doc code block and table is a tab stop, so a keyboard reaches the one
// that scrolls sideways on a narrow screen in a browser that never makes a
// scroller a tab stop itself (https://bugs.webkit.org/show_bug.cgi?id=277290).
// A `pre` needs nothing more (https://github.com/w3c/wcag/issues/3029). A table
// scrolls in a wrapper instead: a role on `<table>` would replace its own, a
// focusable `div` needs a role, and a `region` needs a name unique on its page
// (https://www.w3.org/WAI/ARIA/apg/practices/landmark-regions/).
import { toText } from 'hast-util-to-text';

const HEADING = /^h[1-6]$/;

function tableName(table, heading, taken) {
  const head = table.children?.find((n) => n.tagName === 'thead');
  const row = head?.children?.find((n) => n.tagName === 'tr');
  const cells = (row?.children || [])
    .filter((n) => n.tagName === 'th')
    .map((n) => toText(n).trim())
    .filter(Boolean);
  const base = heading ? `${heading} table` : cells.length ? `table: ${cells.join(', ')}` : 'table';
  let name = base;
  for (let n = 2; taken.has(name); n++) name = `${base} ${n}`;
  taken.add(name);
  return name;
}

export default function rehypeScrollFocus() {
  return (tree) => {
    const taken = new Set();
    let heading = '';
    const walk = (node) => {
      const children = node.children || [];
      children.forEach((child, i) => {
        if (child.type !== 'element') return;
        if (HEADING.test(child.tagName)) {
          heading = toText(child).trim();
        } else if (child.tagName === 'pre') {
          (child.properties ||= {}).tabIndex = 0;
        } else if (child.tagName === 'table') {
          children[i] = {
            type: 'element',
            tagName: 'div',
            properties: {
              className: ['table-scroll'],
              tabIndex: 0,
              role: 'region',
              ariaLabel: tableName(child, heading, taken),
            },
            children: [child],
          };
        } else {
          walk(child);
        }
      });
    };
    walk(tree);
  };
}
