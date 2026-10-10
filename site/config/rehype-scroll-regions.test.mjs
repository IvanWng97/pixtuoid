import { test } from 'node:test';
import assert from 'node:assert/strict';
import rehypeScrollRegions from './rehype-scroll-regions.mjs';

const el = (tagName, children = [], properties = {}) => ({
  type: 'element',
  tagName,
  properties,
  children,
});
const txt = (value) => ({ type: 'text', value });
const run = (children) => {
  const tree = { type: 'root', children };
  rehypeScrollRegions()(tree);
  return tree;
};
const region = { tabIndex: 0, role: 'region' };

test('a code block is a named tab stop, its language in the name', () => {
  const tree = run([
    el('pre', [el('code', [txt('just test')], { className: ['language-bash'] })], {
      className: ['language-bash'],
    }),
  ]);
  const pre = tree.children[0];
  assert.equal(pre.tagName, 'pre');
  assert.deepEqual({ tabIndex: pre.properties.tabIndex, role: pre.properties.role }, region);
  assert.equal(pre.properties.ariaLabel, 'bash code');
});

test('a code block with no language is still named', () => {
  const pre = run([el('pre', [el('code', [txt('x')])])]).children[0];
  assert.equal(pre.properties.tabIndex, 0);
  assert.equal(pre.properties.ariaLabel, 'code');
});

test('a table scrolls inside a named tab stop and keeps its own semantics', () => {
  const table = el('table', [
    el('thead', [el('tr', [el('th', [txt('Key')]), el('th', [el('code', [txt('default')])])])]),
    el('tbody', [el('tr', [el('td', [txt('a')]), el('td', [txt('b')])])]),
  ]);
  const tree = run([el('p', [txt('before')]), table]);
  const wrap = tree.children[1];
  assert.equal(wrap.tagName, 'div');
  assert.deepEqual(wrap.properties.className, ['table-scroll']);
  assert.deepEqual({ tabIndex: wrap.properties.tabIndex, role: wrap.properties.role }, region);
  assert.equal(wrap.properties.ariaLabel, 'table: Key, default');
  assert.equal(wrap.children[0], table);
  assert.equal(table.properties.role, undefined, 'the table keeps its table role');
  assert.equal(table.properties.tabIndex, undefined);
});

test('a nested table or code block is reached, and nothing else is touched', () => {
  const tree = run([
    el('blockquote', [el('pre', [el('code', [txt('x')])]), el('table', [])]),
    el('p', [el('code', [txt('inline')])]),
  ]);
  const [pre, wrap] = tree.children[0].children;
  assert.equal(pre.properties.tabIndex, 0);
  assert.equal(wrap.properties.className[0], 'table-scroll');
  assert.equal(wrap.properties.ariaLabel, 'table', 'a headerless table is still named');
  const blank = run([el('table', [el('thead', [el('tr', [el('th', []), el('th', [txt(' ')])])])])])
    .children[0];
  assert.equal(blank.properties.ariaLabel, 'table', 'a blank header row names nothing');
  const p = tree.children[1];
  assert.deepEqual(p.properties, {}, 'inline code is not a scroller');
});
