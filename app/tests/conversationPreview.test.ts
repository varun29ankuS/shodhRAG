/**
 * Sidebar chat previews: what a conversation is about, from its messages.
 *   node --experimental-strip-types --test app/tests/conversationPreview.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { clip, conversationPreview, plainText } from '../src/lib/conversationPreview.ts';

test('plain text drops diagrams, code and markup but keeps inline math symbols', () => {
  const md = '## KAN\nEach edge has $\\phi_{q,p}$ [1].\n$$f(x)=\\sum_q \\Phi_q$$\n```mermaid\nflowchart LR\nA-->B\n```\n**Done** with [docs](http://x).';
  assert.equal(plainText(md), 'KAN Each edge has phi_q,p . [equation] Done with docs.');
});

test('clip cuts at a word boundary with an ellipsis', () => {
  assert.equal(clip('short', 10), 'short');
  assert.equal(clip('the quick brown fox jumps', 15), 'the quick brown…');
});

test('preview: first question, latest non-empty answer, ranked sources, count', () => {
  const p = conversationPreview([
    { role: 'user', content: 'Explain the **KAN** paper' },
    {
      role: 'assistant',
      content: 'KANs put learnable splines on edges.',
      transcript: { passages: [{ path: 'c:/papers/KAN - Kolmogorov-Arnold Networks.pdf' }, { path: 'c:/papers/KAN - Kolmogorov-Arnold Networks.pdf' }] },
    },
    { role: 'user', content: 'Compare with MLPs' },
    {
      role: 'assistant',
      content: 'MLPs use fixed activations on nodes.',
      searchResults: [{ sourceFile: 'c:/papers/KAN - Kolmogorov-Arnold Networks.pdf' }, { sourceFile: 'calendar://task/1' }],
    },
    { role: 'assistant', content: '   ' },
  ]);
  assert.equal(p.firstQuestion, 'Explain the KAN paper');
  assert.equal(p.latestAnswer, 'MLPs use fixed activations on nodes.');
  assert.deepEqual(p.sources, ['KAN - Kolmogorov-Arnold Networks', 'Tasks']);
  assert.equal(p.questionCount, 2);
});

test('preview of an empty conversation', () => {
  assert.deepEqual(conversationPreview([]), { firstQuestion: null, latestAnswer: null, sources: [], questionCount: 0 });
});

test('web sources are labelled by host', () => {
  const p = conversationPreview([{ role: 'assistant', content: 'x', searchResults: [{ sourceFile: 'https://www.arxiv.org/abs/2404.19756' }] }]);
  assert.deepEqual(p.sources, ['arxiv.org']);
});
