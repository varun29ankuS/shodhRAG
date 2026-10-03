/**
 * Focus pop-out drill-down context: ancestor chain caps, selection targets
 * and their context block, and the nearest document place ("Show in paper").
 *   node --experimental-strip-types --test app/tests/focusDrill.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  ANCESTOR_ANSWER_CHARS,
  MAX_ANCESTOR_CHARS,
  MAX_PARAGRAPH_CHARS,
  ancestorChain,
  buildContextBlock,
  composeSideQuestion,
  contextLabel,
} from '../src/features/focus/contextBlock.ts';
import type { AncestorInfo } from '../src/features/focus/contextBlock.ts';
import {
  MAX_SELECTED_CHARS,
  SURROUNDING_RADIUS,
  nearestPaper,
  paperOf,
  selectionTarget,
  surroundingText,
} from '../src/features/focus/targets.ts';
import { readTarget } from '../src/features/focus/threadStore.ts';
import type { FocusTarget } from '../src/features/focus/focusTypes.ts';

const eq: FocusTarget = { kind: 'equation', label: 'Equation 2', tex: 'y = a + b x + c x^2' };
const diagram: FocusTarget = { kind: 'mermaid', label: 'Spline diagram', source: 'flowchart LR\nA-->B' };

test('ancestor chain lists outer levels outermost first', () => {
  const chain = ancestorChain([
    { target: eq, question: 'What is c?', answer: 'The curvature.\n```followups\n["x"]\n```' },
    { target: diagram, question: 'And B?' },
  ]);
  const lines = chain.split('\n');
  assert.match(lines[0], /^1\. Equation 2 \(equation\): y = a \+ b x \+ c x\^2$/);
  assert.match(chain, /Asked: What is c\?/);
  assert.match(chain, /Answer excerpt: The curvature\.$/m);
  assert.ok(!chain.includes('followups'));
  assert.match(chain, /^2\. Spline diagram \(mermaid\)/m);
  assert.equal(ancestorChain([]), '');
});

test('ancestor chain is capped; nearest levels kept', () => {
  const big: AncestorInfo[] = Array.from({ length: 12 }, (_, i) => ({
    target: { kind: 'equation', label: `Level ${i}`, tex: 'z'.repeat(1000) },
    question: 'q'.repeat(1000),
    answer: 'a'.repeat(5000),
  }));
  const chain = ancestorChain(big);
  assert.ok(Array.from(chain).length <= MAX_ANCESTOR_CHARS + 60, `size ${chain.length}`);
  assert.match(chain, /outer levels not included/);
  assert.match(chain, /Level 11/);
  assert.ok(!chain.includes('Level 0 '));
  assert.ok(!chain.includes('a'.repeat(ANCESTOR_ANSWER_CHARS + 1)));
});

test('nested question carries the chain; root question unchanged', () => {
  const root = composeSideQuestion(diagram, 'Explain', {});
  assert.ok(!root.includes('how the reader got here'));
  const nested = composeSideQuestion(diagram, 'Explain', {}, { ancestors: [{ target: eq, question: 'Why?' }], followups: true });
  assert.match(nested, /Context — how the reader got here \(outermost first\):\n```text\n1\. Equation 2/);
  assert.match(nested, /Question: Explain/);
  assert.match(nested, /```followups/);
  assert.ok(nested.indexOf('Question: Explain') < nested.indexOf('```followups'));
});

test('selection target from an answer', () => {
  const t = selectionTarget({ text: '  the  knot vector ', context: 'A B-spline is defined by the knot vector and control points.', origin: 'answer' });
  assert.ok(t && t.kind === 'selection');
  assert.equal(t.label, '“the knot vector”');
  assert.equal(t.text, 'the knot vector');
  assert.equal(t.paragraph, 'A B-spline is defined by the knot vector and control points.');
  assert.equal(t.document, null);
  assert.equal(selectionTarget({ text: '   ', context: 'x', origin: 'answer' }), null);
});

test('selection target from a document keeps file and page; caps apply', () => {
  const page = `${'lead '.repeat(400)}THE PASSAGE${' tail'.repeat(400)}`;
  const t = selectionTarget({
    text: 'THE PASSAGE',
    context: page,
    origin: 'document',
    document: { sourceFile: '/docs/p.pdf', fileName: 'p.pdf', page: 7 },
  });
  assert.ok(t && t.kind === 'selection');
  assert.deepEqual(t.document, { sourceFile: '/docs/p.pdf', fileName: 'p.pdf', page: 7 });
  assert.ok(t.paragraph.includes('THE PASSAGE'));
  assert.ok(t.paragraph.length <= SURROUNDING_RADIUS * 2 + 'THE PASSAGE'.length + 2);
  assert.ok(t.paragraph.startsWith('…') && t.paragraph.endsWith('…'));
  const huge = selectionTarget({ text: 'x'.repeat(MAX_SELECTED_CHARS * 2), context: '', origin: 'answer' });
  assert.equal(huge?.kind === 'selection' && huge.text.length, MAX_SELECTED_CHARS);
  const badPage = selectionTarget({ text: 'a', context: '', origin: 'document', document: { sourceFile: 'f', fileName: 'f', page: 0 } });
  assert.equal(badPage?.kind === 'selection' && badPage.document?.page, null);
});

test('surroundingText falls back when the selection is not found', () => {
  assert.equal(surroundingText('short block', 'missing'), 'short block');
  const long = 'w '.repeat(2000);
  const out = surroundingText(long, 'zzz');
  assert.ok(out.endsWith('…'));
  assert.ok(out.length <= SURROUNDING_RADIUS * 2 + 1);
});

test('selection context block and label', () => {
  const t = selectionTarget({ text: 'knot', context: `Para about knot. ${'more '.repeat(800)}`, origin: 'document', document: { sourceFile: 'f.pdf', fileName: 'f.pdf', page: 3 } });
  assert.ok(t);
  const block = buildContextBlock(t);
  assert.match(block, /Context — f\.pdf page 3, selected text:\n```text\nknot\n```/);
  assert.match(block, /Context — f\.pdf page 3, surrounding text:/);
  const para = block.split('surrounding text:\n```text\n')[1].split('\n```')[0];
  assert.ok(Array.from(para).length <= MAX_PARAGRAPH_CHARS + 1);
  assert.equal(contextLabel(t), 'selected text and its context (page 3)');
  const plain = selectionTarget({ text: 'same', context: 'same', origin: 'answer' });
  assert.ok(plain);
  assert.ok(!buildContextBlock(plain).includes('surrounding'), 'no duplicate paragraph');
  assert.match(buildContextBlock(plain), /an answer, selected text/);
});

test('selection targets round-trip through the thread store reader', () => {
  const t = selectionTarget({ text: 'abc', context: 'xx abc yy', origin: 'document', document: { sourceFile: 'f.pdf', fileName: 'f.pdf', page: 2 } });
  assert.deepEqual(readTarget(JSON.parse(JSON.stringify(t))), t);
  assert.equal(readTarget({ kind: 'selection', label: 'x', text: '  ' }), null);
  const loose = readTarget({ kind: 'selection', label: 'x', text: 'y', origin: 'weird', document: { sourceFile: '' } });
  assert.deepEqual(loose, { kind: 'selection', label: 'x', text: 'y', paragraph: '', origin: 'answer', document: null });
});

test('nearest paper: own target first, then outer levels', () => {
  const sel = selectionTarget({ text: 'abc', context: '', origin: 'document', document: { sourceFile: 'f.pdf', fileName: 'f.pdf', page: 4 } });
  assert.ok(sel);
  const source: FocusTarget = {
    kind: 'source',
    label: 'Paper',
    hit: { number: 1, sourceFile: 'p.pdf', fileName: 'p.pdf', title: '', text: 'cited', snippet: '', score: 0, page: { start: 9, end: 10 }, lineRange: null, url: null },
  };
  assert.deepEqual(nearestPaper([source, eq, diagram]), { sourceFile: 'p.pdf', fileName: 'p.pdf', page: 9, passage: 'cited' });
  assert.deepEqual(nearestPaper([source, sel]), { sourceFile: 'f.pdf', fileName: 'f.pdf', page: 4, passage: 'abc' });
  assert.equal(nearestPaper([eq, diagram]), null);
  const web: FocusTarget = { ...source, hit: { ...source.hit, url: 'https://x' } } as FocusTarget;
  assert.equal(paperOf(web), null);
});
