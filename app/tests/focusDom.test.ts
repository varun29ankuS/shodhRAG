/**
 * Quoted answer text: what "Ask about this" reads from a rendered answer.
 *   node --experimental-strip-types --test app/tests/focusDom.test.ts
 *
 * There is no DOM in the Node test setup. `readableText` reads only the
 * structural parts of a node (`ReadableNode`), so these trees are built
 * from plain objects with the same shape as the DOM nodes it gets.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ANSWER_UI_ATTR, readableSelection, readableText } from '../src/features/focus/focusDom.ts';
import type { ReadableNode } from '../src/features/focus/focusDom.ts';
import { selectionTarget, surroundingText } from '../src/features/focus/targets.ts';

function text(value: string): ReadableNode {
  return { nodeType: 3, nodeValue: value, childNodes: [] };
}

function el(attrs: Record<string, string>, classes: string[], ...children: ReadableNode[]): ReadableNode {
  return {
    nodeType: 1,
    nodeValue: null,
    childNodes: children,
    hasAttribute: name => Object.prototype.hasOwnProperty.call(attrs, name),
    classList: { contains: token => classes.includes(token) },
  };
}

function fragment(...children: ReadableNode[]): ReadableNode {
  return { nodeType: 11, nodeValue: null, childNodes: children };
}

/** A citation pill as the renderer draws it. */
const pill = (n: number) => el({ [ANSWER_UI_ATTR]: '' }, [], text(String(n)));

/** An unmatched citation: its digit and its screen-reader sentence. */
const invalid = (n: number) =>
  el({ [ANSWER_UI_ATTR]: '', role: 'note' }, [],
    el({ 'aria-hidden': 'true' }, [], text(String(n))),
    el({}, ['sr-only'], text(`Citation ${n} does not match any source of this answer.`)));

test('citation pills are not part of the paragraph text', () => {
  const p = el({}, [], text('SPANN keeps posting lists on disk, lowering latency.'), pill(1), pill(2), pill(7));
  assert.equal(readableText(p), 'SPANN keeps posting lists on disk, lowering latency.');
});

test('grounding flags and screen-reader text are left out', () => {
  const flag = el({ [ANSWER_UI_ATTR]: '' }, [],
    el({ 'aria-hidden': 'true' }, [], text('unsupported')),
    el({}, ['sr-only'], text('No passage supports this statement.')));
  const p = el({}, [], text('It is 10x faster'), flag, text(' on SIFT1B'), invalid(9), text('.'));
  assert.equal(readableText(p), 'It is 10x faster on SIFT1B.');
  // Screen-reader-only text is dropped even without the answer marker.
  assert.equal(readableText(el({}, [], text('A'), el({}, ['sr-only'], text(' hidden')), text('B'))), 'AB');
});

test('other inline controls keep their text: a "go deeper" mark is answer text', () => {
  const deeper = el({ type: 'button' }, [], text('posting lists'));
  const cell = el({}, [], text('Stores '), deeper, text(' on SSD'), pill(3));
  assert.equal(readableText(cell), 'Stores posting lists on SSD');
});

test('a selection crossing pills reads clean and is found in its paragraph', () => {
  const paragraph = el({}, [],
    text('Graph indexes are fast in memory. SPANN keeps posting lists on disk, lowering latency.'),
    pill(1), pill(2),
    text(' It scales to a billion vectors.'));
  // Range.cloneContents() of "lowering latency.[1][2] It scales": the partly
  // selected text nodes are cut, the pills come along whole.
  const selection = fragment(text('lowering latency.'), pill(1), pill(2), text(' It scales'));
  const selected = readableText(selection);
  assert.equal(selected, 'lowering latency. It scales');
  const context = readableText(paragraph);
  assert.ok(context.includes(selected));
  const target = selectionTarget({ text: selected, context, origin: 'answer' });
  assert.ok(target && target.kind === 'selection');
  assert.ok(!/\d{2,}/.test(target.paragraph), target.paragraph);
  assert.ok(surroundingText(context, selected).includes('lowering latency. It scales'));
});

test('a long paragraph is windowed around a clean selection', () => {
  const long = el({}, [], text(`${'Background sentence. '.repeat(60)}SPANN lowers latency.`), pill(1), pill(2), text(` ${'More text here. '.repeat(60)}`));
  const window = surroundingText(readableText(long), readableText(fragment(text('SPANN lowers latency.'), pill(1))));
  assert.ok(window.includes('SPANN lowers latency. More'), window);
});

test('a selection range reads through its cloned contents (Ask about this and the pop-out share it)', () => {
  let clones = 0;
  const range = {
    cloneContents: () => {
      clones += 1;
      return fragment(text('lowering latency.'), pill(1), invalid(4), text(' It scales'));
    },
  };
  assert.equal(readableSelection(range), 'lowering latency. It scales');
  assert.equal(clones, 1);
  assert.equal(readableSelection({ cloneContents: () => fragment() }), '');
});
