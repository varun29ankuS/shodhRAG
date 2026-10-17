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
import { ANSWER_UI_ATTR, isAnswerScope, readableSelection, readableText } from '../src/features/focus/focusDom.ts';
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
    getAttribute: name => (Object.prototype.hasOwnProperty.call(attrs, name) ? attrs[name] : null),
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

/**
 * A formula as KaTeX renders it: <span class="katex"><span class="katex-mathml"><math>
 * <semantics>…<annotation encoding="application/x-tex">TeX</annotation></semantics>
 * </math></span><span class="katex-html" aria-hidden="true">glyphs</span></span>.
 */
function katex(tex: string, ...glyphs: string[]): ReadableNode {
  const mathml = el({}, ['katex-mathml'],
    el({}, [],
      el({}, [], ...glyphs.map(g => el({}, [], text(g))),
      el({ encoding: 'application/x-tex' }, [], text(tex)))));
  return el({}, ['katex'], mathml, katexHtml(...glyphs));
}

function katexHtml(...glyphs: string[]): ReadableNode {
  return el({ 'aria-hidden': 'true' }, ['katex-html'], el({}, ['base'], ...glyphs.map(g => el({}, ['mord'], text(g)))));
}

test('an inline formula reads as its TeX source, not its glyphs', () => {
  const p = el({}, [], text('The loss grows as '), katex('x^2', 'x', '2'), text(' near zero.'));
  assert.equal(readableText(p), 'The loss grows as $x^2$ near zero.');
  assert.equal(readableText(el({}, [], katex('\\frac{a}{b}', 'a', 'b'))), '$\\frac{a}{b}$');
});

test('display math reads as $$…$$', () => {
  const display = el({}, ['katex-display'], katex('f(x)=\\sum_q \\Phi_q', 'f', '(', 'x', ')'));
  const p = el({}, [], text('The model is'), display, text('with learnable Φ.'));
  assert.equal(readableText(p), 'The model is$$f(x)=\\sum_q \\Phi_q$$with learnable Φ.');
});

test('symbol wrappers added for meanings are removed from the TeX', () => {
  const wrapped = katex('{\\htmlData{shodh-sym=0}{\\Phi_q}}(x) + {\\htmlData{shodh-sym=1}{x_p}}', 'Φ', 'q');
  assert.equal(readableText(el({}, [], wrapped)), '$\\Phi_q(x) + x_p$');
});

test('part of a formula without its annotation reads as the glyphs shown', () => {
  // Range.cloneContents() of a selection inside the rendered glyphs only.
  const partial = fragment(el({}, ['katex'], katexHtml('x', '2')), text(' near zero'));
  assert.equal(readableText(partial), 'x2 near zero');
  // The MathML copy alone is never read as text.
  assert.equal(readableText(fragment(el({}, ['katex-mathml'], text('x2')))), '');
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

test('a selection range reads through its cloned contents', () => {
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

test('only answer scopes (conversation and side thread) read selections without answer controls', () => {
  assert.equal(isAnswerScope('answer'), true);
  assert.equal(isAnswerScope('thread'), true);
  for (const kind of ['document', '', null, undefined]) assert.equal(isAnswerScope(kind), false, String(kind));
});

test('a miscitation flag and its "Cite N instead" button are not answer text', () => {
  // As ClaimFlag draws it: a wrapper with the flag and a sibling button, all answer UI.
  const flag = el({ [ANSWER_UI_ATTR]: '' }, [],
    el({ [ANSWER_UI_ATTR]: '' }, [],
      el({ 'aria-hidden': 'true' }, [], text('Supported by passage 2 (answer cited 3)')),
      el({}, ['sr-only'], text('Passage 2 supports this statement; the answer cited [3], which does not.'))),
    el({ [ANSWER_UI_ATTR]: '' }, [], text('Cite 2 instead')));
  const p = el({}, [], text('SPANN expands posting lists with the closure, reducing disk-access latency.'), pill(3), flag);
  assert.equal(readableText(p), 'SPANN expands posting lists with the closure, reducing disk-access latency.');
  // The surrounding text of a selection made in that answer carries no citation marker.
  const target = selectionTarget({ text: 'disk-access latency', context: readableText(p), origin: 'answer' });
  assert.ok(target && target.kind === 'selection');
  assert.ok(!/\[\d+\]|Cite 2/.test(target.paragraph), target.paragraph);
});
