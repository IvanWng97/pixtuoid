// Unit tests for the callout promoter — the doc-blockquote → terminal-window
// transform (spec §6). Pure hast in/out, same posture as csp-hashes.test.mjs.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import rehypeCallouts from './rehype-callouts.mjs';

const el = (tagName, children = [], properties = {}) => ({
  type: 'element',
  tagName,
  properties,
  children,
});
const txt = (value) => ({ type: 'text', value });
const p = (...children) => el('p', children);
const strong = (s) => el('strong', [txt(s)]);
const quote = (...children) => el('blockquote', children);
const run = (children) => {
  const tree = { type: 'root', children };
  rehypeCallouts()(tree);
  return tree;
};

test('a plain blockquote becomes a ~ note terminal window', () => {
  const tree = run([quote(p(txt('This file is the single source.')))]);
  const wrap = tree.children[0];
  assert.equal(wrap.tagName, 'div');
  assert.deepEqual(wrap.properties.className, ['callout', 'callout--note']);
  const bar = wrap.children[0];
  assert.deepEqual(bar.properties.className, ['callout__bar']);
  const title = bar.children.find((c) => c.properties?.className?.includes('callout__title'));
  assert.equal(title.children[0].value, '~ note');
  // a note window carries NO red dot
  assert.ok(!JSON.stringify(bar).includes('terminal__dot--r'));
});

test("a **Don't …** opener becomes the red-dot ~ warning window", () => {
  const tree = run([quote(p(strong("Don't chain `cargo clippy && cargo test`"), txt(' — …')))]);
  const wrap = tree.children[0];
  assert.deepEqual(wrap.properties.className, ['callout', 'callout--warn']);
  const bar = wrap.children[0];
  const dot = bar.children.find((c) => c.properties?.className?.includes('terminal__dot--r'));
  assert.ok(dot, 'warning bar carries the terminal red dot');
  const title = bar.children.find((c) => c.properties?.className?.includes('callout__title'));
  assert.equal(title.children[0].value, '~ warning');
});

test("astro's smartypants curly apostrophe (U+2019) still classifies as a warning", () => {
  // docs/CONTRIBUTING.md's own "**Don't chain …**" idiom — the motivating
  // example — comes out of remark as "Don’t" (typeset quote), not the
  // ASCII "Don't" the source markdown was typed with.
  const tree = run([quote(p(strong('Don’t chain `cargo clippy && cargo test`')))]);
  assert.deepEqual(tree.children[0].properties.className, ['callout', 'callout--warn']);
});

test('Never/Warning openers also classify as warnings', () => {
  for (const opener of ['Never do this', 'Warning: hot', 'Caution — edges']) {
    const tree = run([quote(p(strong(opener)))]);
    assert.deepEqual(tree.children[0].properties.className, ['callout', 'callout--warn'], opener);
  }
});

test('the original quote children survive verbatim inside .callout__body', () => {
  const para = p(txt('kept content'));
  const tree = run([quote(para)]);
  const body = tree.children[0].children[1];
  assert.equal(body.tagName, 'blockquote');
  assert.deepEqual(body.properties.className, ['callout__body']);
  assert.equal(body.children[0], para);
});

test('a blockquote nested inside a wrapped quote is not double-wrapped', () => {
  const tree = run([quote(p(txt('outer')), quote(p(txt('inner'))))]);
  const body = tree.children[0].children[1];
  const inner = body.children.find((c) => c.tagName === 'blockquote');
  assert.ok(inner, 'the inner quote stays a plain blockquote');
  assert.ok(!(inner.properties?.className || []).includes('callout__body'));
});

test('non-blockquote content is untouched', () => {
  const para = p(txt('prose'));
  const tree = run([para]);
  assert.equal(tree.children[0], para);
});
