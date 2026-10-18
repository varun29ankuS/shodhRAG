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

test('ancestorsFor: outer levels with the exchange each next level came from', async () => {
  const { ancestorsFor } = await import('../src/features/focus/threadTree.ts');
  const turn = (id: string, role: 'user' | 'assistant', content: string) => ({ id, role, content, timestamp: id });
  const root = {
    id: 'r',
    anchor: { conversationId: 'c', parentMessageId: 'm', target: eq },
    turns: [turn('q1', 'user', 'First?'), turn('a1', 'assistant', 'First answer'), turn('q2', 'user', 'Second?'), turn('a2', 'assistant', 'Second answer')],
    createdAt: '0',
    updatedAt: '0',
  };
  const mid = {
    id: 'm1',
    parentThreadId: 'r',
    parentTurnId: 'a1',
    anchor: { conversationId: 'c', parentMessageId: 'm', target: diagram },
    turns: [turn('q3', 'user', 'Node B?'), turn('a3', 'assistant', 'B is the knot')],
    createdAt: '0',
    updatedAt: '0',
  };
  const chain = ancestorsFor([root, mid], 'm1', 'a3');
  assert.equal(chain.length, 2);
  assert.deepEqual(chain[0], { target: eq, question: 'First?', answer: 'First answer' }, 'root: the answer the child came from, not the latest');
  assert.deepEqual(chain[1], { target: diagram, question: 'Node B?', answer: 'B is the knot' });
  assert.deepEqual(ancestorsFor([root, mid], null, null), []);
  // Unknown turn: the latest answer is used.
  assert.equal(ancestorsFor([root], 'r', 'gone')[0].answer, 'Second answer');
  // Missing parent thread: no chain rather than a crash.
  assert.deepEqual(ancestorsFor([mid], 'missing', 'x'), []);
});

test('brought-back notes travel with the next question, capped', async () => {
  const { MAX_NOTES_CHARS, notesBlock } = await import('../src/features/focus/contextBlock.ts');
  const q = composeSideQuestion(eq, 'Next?', {}, { notes: [{ label: 'Spline', text: 'Summary with $x^2$' }] });
  assert.match(q, /Context — brought back from the nested discussion about "Spline":\n```markdown\nSummary with \$x\^2\$\n```/);
  assert.ok(q.indexOf('brought back') < q.indexOf('Question: Next?'));
  const big = notesBlock([{ label: 'old', text: 'o'.repeat(5000) }, { label: 'new', text: 'n'.repeat(5000) }]);
  assert.ok(!big.includes('"old"'), 'newest kept first');
  assert.match(big, /more characters not included/);
  assert.ok((big.match(/n/g) ?? []).length <= MAX_NOTES_CHARS + 10);
  assert.equal(notesBlock([]), '');
});

test('replayed history alternates: unanswered brought-back notes merge into the next question', async () => {
  const { alternateTurns, threadHistory } = await import('../src/features/focus/threadStore.ts');
  const out = alternateTurns(threadHistory(
    [{ role: 'user', content: 'main q' }, { role: 'assistant', content: 'main a' }],
    [{ role: 'user', content: 'note' }, { role: 'user', content: 'side q' }, { role: 'assistant', content: 'side a\n```followups\n["x"]\n```' }],
  ));
  assert.deepEqual(out, [
    { role: 'user', content: 'main q' },
    { role: 'assistant', content: 'main a' },
    { role: 'user', content: 'note\n\nside q' },
    { role: 'assistant', content: 'side a' },
  ]);
});

test("a nested question does not inherit an outer answer's flagged citations", async () => {
  const { ancestorsFor } = await import('../src/features/focus/threadTree.ts');
  // The reported Q1 answer: [3] flagged (passage 2 states it), [1] supported.
  const flaggedAnchor = 'SPANN expands posting lists with the points in the closure, reducing disk-access latency. [3]';
  const supportedAnchor = 'Closure augmentation adds boundary points to nearby clusters [1].';
  const claim = (anchor: string, outcome: string, cited: number[], extra: Record<string, unknown> = {}) => ({
    messageId: 'm1', text: anchor.replace(/\s*\[\d+\]/, ''), anchor, kind: 'sentence', outcome, cited, invalid: [],
    support: 0.3, contradiction: null, missingNumbers: [], closest: null, closestScore: null, ...extra,
  });
  const answer = `${supportedAnchor} ${flaggedAnchor}`;
  const transcript = {
    runId: 'run-q1', startedAtMs: 0, status: 'completed', blocks: [{ kind: 'text', id: 'm1', text: answer }], steps: {},
    groundings: [{
      round: 0, isFinal: true, method: 'cross_encoder', needs: [], messageIds: ['m1'], supersededMessageIds: [],
      summary: { checked: 2, supported: 1, weak: 0, unsupported: 1, uncited: 0, invalid: 0, unchecked: 0, score: 0.5 },
      claims: [
        claim(supportedAnchor, 'supported', [1]),
        claim(flaggedAnchor, 'unsupported', [3], { closest: 2, closestScore: 0.97, suggested: 2 }),
      ],
    }],
  };
  const q1 = {
    id: 'q1',
    anchor: { conversationId: 'c', parentMessageId: 'm', target: eq },
    turns: [
      { id: 'u1', role: 'user' as const, content: 'What is closure augmentation?', timestamp: '1' },
      { id: 'a1', role: 'assistant' as const, content: answer, timestamp: '2', transcript },
    ],
    createdAt: '0',
    updatedAt: '0',
  };
  const chain = ancestorsFor([q1], 'q1', 'a1');
  assert.equal(
    chain[0].answer,
    'Closure augmentation adds boundary points to nearby clusters [1]. SPANN expands posting lists with the points in the closure, reducing disk-access latency.',
  );
  const nested = composeSideQuestion(diagram, 'How is posting list quality improved?', {}, { ancestors: chain, followups: true });
  assert.ok(nested.includes('reducing disk-access latency.'), nested);
  assert.ok(!nested.includes('latency. [3]') && !nested.includes('[3]'), nested);
  assert.ok(nested.includes('nearby clusters [1].'), 'a supported citation is kept');
  // A turn saved without a transcript is passed on as written.
  const plain = { ...q1, turns: [q1.turns[0], { ...q1.turns[1], transcript: undefined }] };
  assert.equal(ancestorsFor([plain], 'q1', 'a1')[0].answer, answer);
});

/** An answer whose check flagged its [3] (passage 2 states it) and supported its [1]. */
function flaggedAnswer() {
  const flaggedAnchor = 'Posting lists are expanded with the closure points. [3]';
  const supportedAnchor = 'Centroids stay in memory [1].';
  const claim = (anchor: string, outcome: string, cited: number[]) => ({
    messageId: 'm1', text: anchor.replace(/\s*\[\d+\]/, ''), anchor, kind: 'sentence', outcome, cited, invalid: [],
    support: 0.3, contradiction: null, missingNumbers: [], closest: null, closestScore: null,
  });
  const content = `${supportedAnchor} ${flaggedAnchor}`;
  const transcript = {
    runId: 'r', startedAtMs: 0, status: 'completed', blocks: [{ kind: 'text', id: 'm1', text: content }], steps: {},
    groundings: [{
      round: 0, isFinal: true, method: 'entailment', needs: [], messageIds: ['m1'], supersededMessageIds: [],
      summary: { checked: 2, supported: 1, weak: 0, unsupported: 1, uncited: 0, invalid: 0, unchecked: 0, score: 0.5 },
      claims: [claim(supportedAnchor, 'supported', [1]), claim(flaggedAnchor, 'unsupported', [3])],
    }],
  };
  return { content, transcript, cleaned: 'Centroids stay in memory [1]. Posting lists are expanded with the closure points.' };
}

test("replayed history drops an answer's flagged citations, in the thread and in the main conversation", async () => {
  const { threadHistory } = await import('../src/features/focus/threadStore.ts');
  const { content, transcript, cleaned } = flaggedAnswer();
  const history = threadHistory(
    [{ role: 'user', content: 'What is SPANN?' }, { role: 'assistant', content, transcript }],
    [{ role: 'user', content: 'And closure?' }, { role: 'assistant', content, transcript }],
  );
  assert.deepEqual(history, [
    { role: 'user', content: 'What is SPANN?' },
    { role: 'assistant', content: cleaned },
    { role: 'user', content: 'And closure?' },
    { role: 'assistant', content: cleaned },
  ]);
  // Without a stored transcript the answer is replayed as written.
  assert.equal(threadHistory([], [{ role: 'assistant', content }])[0].content, content);
});

test('a discussion brought back does not carry its flagged citations up', async () => {
  const { composeSummaryRequest, discussionText } = await import('../src/features/focus/summary.ts');
  const { threadSummary } = await import('../src/features/focus/threadStore.ts');
  const { content, transcript, cleaned } = flaggedAnswer();
  const thread = {
    anchor: { conversationId: 'c', parentMessageId: 'm', target: eq },
    turns: [
      { id: 'u', role: 'user' as const, content: 'And closure?', timestamp: '1' },
      { id: 'a', role: 'assistant' as const, content, timestamp: '2', transcript },
    ],
  };
  assert.equal(discussionText(thread), `Q: And closure?\n\nA: ${cleaned}`);
  const request = composeSummaryRequest(thread, { kind: 'conversation' });
  assert.ok(request.includes(cleaned) && !request.includes('[3]'), request);
  // The excerpt offered when no summary could be written: without the flagged citation, and
  // without the supported one too, whose passage this transcript did not keep (a marker
  // brought into another conversation would cite its passages).
  assert.equal(threadSummary(thread), '**Asked:** And closure?\n\nCentroids stay in memory. Posting lists are expanded with the closure points.');
});

test("the main conversation replayed into a fresh session drops an answer's flagged citations", async () => {
  const { replayedContent } = await import('../src/features/ask/chatHistory.ts');
  const { content, transcript, cleaned } = flaggedAnswer();
  assert.equal(replayedContent({ role: 'assistant', content, transcript }), cleaned);
  assert.equal(replayedContent({ role: 'assistant', content }), content, 'no transcript: as written');
  assert.equal(replayedContent({ role: 'user', content: 'See [3].' }), 'See [3].', 'what the user wrote is never changed');
  assert.equal(
    replayedContent({ role: 'user', content: 'A note', sideSummary: { threadId: 't', label: 'Closure', parentMessageId: null } }),
    'From a side discussion about "Closure":\n\nA note',
  );
});
