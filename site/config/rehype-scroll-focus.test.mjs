import { test } from 'node:test';
import assert from 'node:assert/strict';
import rehypeScrollFocus from './rehype-scroll-focus.mjs';

const el = (tagName, children = [], properties = {}) => ({
  type: 'element',
  tagName,
  properties,
  children,
});
const txt = (value) => ({ type: 'text', value });
const run = (children) => {
  const tree = { type: 'root', children };
  rehypeScrollFocus()(tree);
  return tree;
};
const table = (...headers) =>
  el('table', [
    el('thead', [
      el(
        'tr',
        headers.map((h) => el('th', h ? [txt(h)] : []))
      ),
    ]),
  ]);
const names = (tree) =>
  tree.children.filter((n) => n.tagName === 'div').map((n) => n.properties.ariaLabel);

test('a code block is a tab stop and nothing more', () => {
  const pre = run([el('pre', [el('code', [txt('just test')])], { className: ['language-bash'] })])
    .children[0];
  assert.equal(pre.properties.tabIndex, 0);
  assert.equal(pre.properties.role, undefined, 'no landmark per code block');
  assert.equal(pre.properties.ariaLabel, undefined);
});

test('a table scrolls inside a named region and keeps its own semantics', () => {
  const t = table('Key', 'Default');
  const wrap = run([el('h2', [txt('Keys')]), t]).children[1];
  assert.equal(wrap.tagName, 'div');
  assert.deepEqual(wrap.properties.className, ['table-scroll']);
  assert.equal(wrap.properties.tabIndex, 0);
  assert.equal(wrap.properties.role, 'region');
  assert.equal(wrap.properties.ariaLabel, 'Keys table');
  assert.equal(wrap.children[0], t);
  assert.deepEqual(t.properties, {}, 'the table keeps its table role');
});

test('a region name is unique on its page', () => {
  const tree = run([
    el('h2', [txt('Keys')]),
    table('Key', 'Default'),
    table('Key', 'Default'),
    el('h2', [el('code', [txt('[floating]')]), txt(' keys')]),
    table('Key'),
  ]);
  assert.deepEqual(names(tree), ['Keys table', 'Keys table 2', '[floating] keys table']);
});

test('with no heading above it a table is named by its header, else plainly', () => {
  const tree = run([table('when', 'run'), table('', ' '), table()]);
  assert.deepEqual(names(tree), ['table: when, run', 'table', 'table 2']);
});

test('a nested table or code block is reached, and nothing else is touched', () => {
  const tree = run([
    el('blockquote', [el('pre', [el('code', [txt('x')])]), table('a')]),
    el('p', [el('code', [txt('inline')])]),
  ]);
  const [pre, wrap] = tree.children[0].children;
  assert.equal(pre.properties.tabIndex, 0);
  assert.deepEqual(wrap.properties.className, ['table-scroll']);
  assert.deepEqual(tree.children[1].properties, {}, 'inline code is not a scroller');
});
