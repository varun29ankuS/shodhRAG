/**
 * Focus pop-out: bringing side discussions back (safe truncation, summary
 * request, summary message marker, user-message Markdown detection).
 *   node --experimental-strip-types --test app/tests/focusSummary.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { protectedSpans, safeTruncate } from '../src/features/focus/markdownSafe.ts';
import {
  MAX_SUMMARY_SOURCE_CHARS,
  SUMMARY_METADATA_KEY,
  cleanSummary,
  composeSummaryRequest,
  discussionText,
  hardBreaks,
  heldPassageNumbers,
  metadataWithSummary,
  readSideSummary,
  summaryPrompt,
  summaryWithHeldCitations,
  wantsMarkdown,
} from '../src/features/focus/summary.ts';
import { METADATA_KEY, metadataWithThreads, metadataWithoutThreads, threadSummary } from '../src/features/focus/threadStore.ts';
import type { FocusThread } from '../src/features/focus/focusTypes.ts';

/** Every $, $$ and fence in `text` is balanced. */
function balanced(text: string): boolean {
  const body = text.endsWith('…') ? text.slice(0, -1) : text;
  const fences = (body.match(/^```/gm) ?? []).length;
  const stripped = body.replace(/```[\s\S]*?```/g, '');
  const doubles = (stripped.match(/\$\$/g) ?? []).length;
  const singles = (stripped.replace(/\$\$/g, '').match(/\$/g) ?? []).length;
  return fences % 2 === 0 && doubles % 2 === 0 && singles % 2 === 0;
}

test('safeTruncate leaves short text alone', () => {
  assert.equal(safeTruncate('short $x$', 100), 'short $x$');
  assert.equal(safeTruncate('anything', 0), '');
});

test('safeTruncate never splits display math', () => {
  const text = `Intro words here. $$\\int_0^1 f(x)\\,dx = F(1) - F(0)$$ and then the rest.`;
  for (let max = 5; max < text.length; max++) {
    const out = safeTruncate(text, max);
    assert.ok(out.length <= max, `length at ${max}`);
    assert.ok(balanced(out), `balanced at ${max}: ${out}`);
  }
});

test('safeTruncate never splits inline math, \\( \\) or \\[ \\]', () => {
  const text = 'Energy $E = mc^2$ relates mass, see \\(a+b\\) and \\[x^2\\] then plain prose continues for a while.';
  for (let max = 3; max < text.length; max++) {
    const out = safeTruncate(text, max);
    assert.ok(balanced(out), `at ${max}: ${out}`);
    const opens = (out.match(/\\\(/g) ?? []).length;
    const closes = (out.match(/\\\)/g) ?? []).length;
    assert.equal(opens, closes, `\\( at ${max}`);
    assert.equal((out.match(/\\\[/g) ?? []).length, (out.match(/\\\]/g) ?? []).length, `\\[ at ${max}`);
  }
});

test('safeTruncate never splits a code fence (mermaid)', () => {
  const text = 'Here is the flow:\n\n```mermaid\nflowchart LR\n  A --> B\n  B --> C\n```\n\nAnd a conclusion.';
  for (let max = 3; max < text.length; max++) {
    const out = safeTruncate(text, max);
    assert.ok(balanced(out), `at ${max}: ${JSON.stringify(out)}`);
  }
  assert.equal(safeTruncate(text, text.length - 3), 'Here is the flow:\n\n```mermaid\nflowchart LR\n  A --> B\n  B --> C\n```…');
});

test('currency is not mistaken for math', () => {
  assert.deepEqual(protectedSpans('costs $5 and $6 today'), []);
  assert.equal(protectedSpans('a $x$ b').length, 1);
  assert.equal(protectedSpans('escaped \\$x$ no').length, 0);
});

test('safeTruncate does not split a surrogate pair', () => {
  const out = safeTruncate('😀😀😀😀😀😀', 4);
  assert.ok(!/[\ud800-\udbff]…$/.test(out));
});

const thread = (turns: FocusThread['turns']): FocusThread => ({
  id: 't',
  anchor: { conversationId: 'c', parentMessageId: 'm', target: { kind: 'equation', label: 'Spline', tex: 's' } },
  turns,
  createdAt: '0',
  updatedAt: '0',
});

test('summary request: rules, no followups, discussion fenced and capped', () => {
  const t = thread([
    { id: '1', role: 'user', content: 'Why $C^2$?', timestamp: '1' },
    { id: '2', role: 'assistant', content: 'Because $$f\'\'$$ is continuous.\n```followups\n["x"]\n```', timestamp: '2' },
  ]);
  const req = composeSummaryRequest(t, { kind: 'conversation' });
  assert.match(req, /main conversation/);
  assert.match(req, /Do not use any tools/);
  assert.match(req, /LaTeX/);
  assert.match(req, /mermaid/);
  assert.ok(!req.includes('```followups'), 'followups are not sent back');
  assert.match(req, /Q: Why \$C\^2\$\?/);
  const parent = composeSummaryRequest(t, { kind: 'parent', label: 'Equation 2' });
  assert.match(parent, /parent discussion about "Equation 2"/);

  const many = thread(Array.from({ length: 60 }, (_, i) => ({
    id: String(i), role: i % 2 ? 'assistant' : 'user', content: `turn ${i} ${'y'.repeat(400)}`, timestamp: String(i),
  })));
  const text = discussionText(many);
  assert.ok(text.length <= MAX_SUMMARY_SOURCE_CHARS + 60);
  assert.match(text, /earlier turns not included/);
  assert.match(text, /turn 59/, 'latest turns kept');
});

test('cleanSummary strips followups and caps safely', () => {
  assert.equal(cleanSummary('  Sum $x$.\n```followups\n["a"]\n```  '), 'Sum $x$.');
});

test('a summary names the passages the discussion holds and drops made-up citations', () => {
  const passage = (n: number, file: string, page: string | null) => ({ n, file, path: `c:/papers/${file}`, page, heading: null, score: 0.9, text: 'Passage text.' });
  const answer = (id: string, passages: unknown[]): FocusThread['turns'][number] => ({
    id,
    role: 'assistant',
    content: 'An answer that cites its passages [1].',
    timestamp: id,
    transcript: { passages },
  });
  const t = thread([
    { id: '1', role: 'user', content: 'Why a spline?', timestamp: '1' },
    answer('2', [passage(1, 'splines.pdf', '3'), passage(2, 'knots.pdf', '7'), passage(4, 'a.pdf', '1')]),
    { id: '3', role: 'user', content: 'And the knots?', timestamp: '3' },
    // Numbers restart in each answer: 4 names another passage here, so it names none.
    answer('4', [passage(3, 'knots.pdf', null), passage(4, 'b.pdf', '2')]),
    // A question's turn holds no passages, whatever it carries.
    { id: '5', role: 'user', content: 'Thanks', timestamp: '5', transcript: { passages: [passage(9, 'x.pdf', '1')] } },
  ]);
  assert.deepEqual([...heldPassageNumbers(t)].sort(), [1, 2, 3, 4]);
  assert.equal(
    summaryWithHeldCitations('Splines are smooth [1]. Knots set the pieces [3], see [7]. Both [1, 2, 9] and [8-9], not [4].', t),
    'Splines are smooth (side-discussion source: splines.pdf, p. 3). Knots set the pieces (side-discussion source: knots.pdf), see.'
      + ' Both (side-discussion sources: splines.pdf, p. 3; knots.pdf, p. 7) and, not.',
  );
  // Code and math are not citations: an index, an interval, a matrix.
  for (const text of [
    'Index `a[7]` is read.',
    'For $x \\in [1, 5]$ the spline is cubic.',
    'Pick $[2]$ here.',
    'Display $$x \\in [1, 5] \\cup [7]$$ holds.',
    '```\nv = a[1]\n```',
    'No citations here.',
  ]) {
    assert.equal(summaryWithHeldCitations(text, t), text, text);
  }
  // A citation next to math is still one.
  assert.equal(summaryWithHeldCitations('On $[0, 1]$ it is smooth [1].', t), 'On $[0, 1]$ it is smooth (side-discussion source: splines.pdf, p. 3).');
  // A discussion whose answers kept no passages (older transcripts): every marker goes.
  assert.equal(summaryWithHeldCitations('Smooth [1].', thread([answer('a', [])])), 'Smooth.');
  assert.equal(summaryWithHeldCitations('Smooth [1].', thread([{ id: 'b', role: 'assistant', content: 'x', timestamp: 'b' }])), 'Smooth.');
});

test('the agent reads a posted summary without citation markers', () => {
  const ref = { label: 'Spline' };
  assert.equal(
    summaryPrompt(ref, 'Typed later [2]. From splines.pdf (side-discussion source: splines.pdf, p. 3). For $x \\in [1, 5]$.'),
    'From a side discussion about "Spline":\n\nTyped later. From splines.pdf (side-discussion source: splines.pdf, p. 3). For $x \\in [1, 5]$.',
  );
});

test('the fallback summary names the passages of its own answer', () => {
  const t = thread([
    { id: '1', role: 'user', content: 'Why?', timestamp: '1' },
    {
      id: '2',
      role: 'assistant',
      content: 'Splines are smooth [1] and local [5].',
      timestamp: '2',
      transcript: { passages: [{ n: 1, file: 'splines.pdf', path: 'c:/papers/splines.pdf', page: '3' }] },
    },
  ]);
  assert.equal(threadSummary(t), '**Asked:** Why?\n\nSplines are smooth (side-discussion source: splines.pdf, p. 3) and local.');
});

test('summary marker round-trips through metadata alongside threads', () => {
  const ref = { threadId: 'thr', label: 'Spline', parentMessageId: 'm1' };
  const meta = metadataWithSummary({ model: 'x' }, ref);
  assert.deepEqual(readSideSummary(meta), ref);
  assert.equal((meta as Record<string, unknown>).model, 'x');
  // Survives JSON (conversations.json) and the threads merge.
  const stored = JSON.parse(JSON.stringify(metadataWithThreads(meta, [])));
  assert.deepEqual(readSideSummary(stored), ref);
  assert.equal(METADATA_KEY in stored, false);
  assert.deepEqual(readSideSummary(metadataWithoutThreads(stored)), ref);
  assert.equal(metadataWithSummary(meta, null)?.[SUMMARY_METADATA_KEY], undefined);
  assert.equal(metadataWithSummary(undefined, null), undefined);
  assert.equal(readSideSummary({ [SUMMARY_METADATA_KEY]: { threadId: '', label: 'x' } }), null);
  assert.equal(readSideSummary({ [SUMMARY_METADATA_KEY]: 'nope' }), null);
  assert.deepEqual(readSideSummary({ [SUMMARY_METADATA_KEY]: { threadId: 'a', label: 'L', parentMessageId: 5 } }), { threadId: 'a', label: 'L', parentMessageId: null });
});

test('user messages render as Markdown only with math, fences or tables', () => {
  assert.equal(wantsMarkdown('just a question about *stars* and_snakes'), false);
  assert.equal(wantsMarkdown('it costs $5 and $6'), false);
  assert.equal(wantsMarkdown('# not a heading, just a hash'), false);
  assert.equal(wantsMarkdown('what is $\\alpha$?'), true);
  assert.equal(wantsMarkdown('what is $x^2$?'), true);
  assert.equal(wantsMarkdown('display $$E=mc^2$$'), true);
  assert.equal(wantsMarkdown('\\[a+b\\]'), true);
  assert.equal(wantsMarkdown('```\ncode\n```'), true);
  assert.equal(wantsMarkdown('| a | b |\n|---|---|\n| 1 | 2 |'), true);
});

test('hardBreaks keeps single line breaks outside fences and math', () => {
  assert.equal(hardBreaks('line one\nline two'), 'line one  \nline two');
  assert.equal(hardBreaks('para\n\nnext'), 'para\n\nnext');
  assert.equal(hardBreaks('```\na\nb\n```'), '```\na\nb\n```');
  assert.equal(hardBreaks('text\n$$\nx\n$$'), 'text\n$$\nx\n$$');
});
