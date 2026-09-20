import assert from 'node:assert/strict';
import test from 'node:test';

import rehypeBeautifulMermaid from './rehype-beautiful-mermaid.mjs';

const SOURCE = `flowchart TB
  accTitle: pixtuoid data flow
  accDescr: A hook event flows into the reducer.
  A["hook"] --> B["Reducer::apply"]
`;

function codeBlock(lang, value) {
  return {
    type: 'element',
    tagName: 'pre',
    properties: {},
    children: [
      {
        type: 'element',
        tagName: 'code',
        properties: { className: [`language-${lang}`] },
        children: [{ type: 'text', value }],
      },
    ],
  };
}

function textOf(node) {
  if (node.type === 'text') return node.value;
  return (node.children ?? []).map(textOf).join('');
}

function run(tree) {
  rehypeBeautifulMermaid()(tree);
  return tree;
}

test('a ```mermaid block becomes one inline <svg> with no client-side script', () => {
  const tree = run({ type: 'root', children: [codeBlock('mermaid', SOURCE)] });
  assert.equal(tree.children.length, 1);
  const svg = tree.children[0];
  assert.equal(svg.tagName, 'svg');
  assert.ok(textOf(svg).includes('Reducer::apply'), 'node labels render as text');
  assert.equal(JSON.stringify(svg).includes('<script'), false);
});

test('accTitle / accDescr become the <title>/<desc> the SVG is labelled by, not nodes', () => {
  const tree = run({ type: 'root', children: [codeBlock('mermaid', SOURCE)] });
  const svg = tree.children[0];
  const [title, desc] = svg.children;
  assert.equal(title.tagName, 'title');
  assert.equal(textOf(title), 'pixtuoid data flow');
  assert.equal(desc.tagName, 'desc');
  assert.equal(textOf(desc), 'A hook event flows into the reducer.');
  assert.equal(svg.properties.role, 'img');
  assert.equal(svg.properties.ariaLabelledBy, `${title.properties.id} ${desc.properties.id}`);
  assert.equal(textOf(svg).includes('accTitle'), false, 'the directive is not drawn as a node');
  assert.equal(textOf(svg).includes('accDescr'), false);
});

function stylesheetOf(svg) {
  return svg.children
    .filter((c) => c.tagName === 'style')
    .flatMap((s) => s.children.map((c) => c.value))
    .join('\n');
}

test('colors are the page theme: the root declares no --bg/--fg of its own', () => {
  const tree = run({ type: 'root', children: [codeBlock('mermaid', SOURCE)] });
  const svg = tree.children[0];
  assert.doesNotMatch(String(svg.properties.style ?? ''), /--(bg|fg)\s*:/);
  const sheet = stylesheetOf(svg);
  assert.match(sheet, /var\(--bg\)/);
  assert.match(sheet, /var\(--fg\)/);
});

test('the stylesheet fetches nothing: no @import, no URL (CSP style-src/font-src self)', () => {
  const tree = run({ type: 'root', children: [codeBlock('mermaid', SOURCE)] });
  const sheet = stylesheetOf(tree.children[0]);
  assert.doesNotMatch(sheet, /@import|https?:/);
});

test('a non-mermaid code block is left alone', () => {
  const block = codeBlock('rust', 'fn main() {}');
  const tree = run({ type: 'root', children: [block] });
  assert.equal(tree.children[0], block);
});

test('a diagram without accessibility directives still renders, unlabelled', () => {
  const tree = run({ type: 'root', children: [codeBlock('mermaid', 'flowchart LR\n  A --> B')] });
  const svg = tree.children[0];
  assert.equal(svg.tagName, 'svg');
  assert.equal(
    svg.children.some((c) => c.tagName === 'title'),
    false
  );
  assert.equal(svg.properties.ariaLabelledBy, undefined);
});
