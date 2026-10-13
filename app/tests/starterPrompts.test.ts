/**
 * Starter questions fit the library. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/starterPrompts.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DOCUMENT_PROMPTS, PAPER_PROMPTS, readLibraryKind, starterPrompts } from '../src/features/ask/starterPrompts.ts';

test('a library of papers is offered questions about methods, results and comparisons', () => {
  const prompts = starterPrompts(readLibraryKind({ kind: 'papers', files: 12, papers: 12 }));
  assert.deepEqual(prompts, PAPER_PROMPTS);
  assert.ok(prompts.some(p => /method/i.test(p)));
  assert.ok(prompts.some(p => /result/i.test(p)));
  assert.ok(prompts.some(p => /compare|baseline/i.test(p)));
  assert.ok(!prompts.some(p => /payment|deadline/i.test(p)));
});

test('other libraries, and an unknown kind, keep the general questions', () => {
  assert.deepEqual(starterPrompts(readLibraryKind({ kind: 'documents', files: 3, papers: 0 })), DOCUMENT_PROMPTS);
  assert.deepEqual(starterPrompts(null), DOCUMENT_PROMPTS);
  assert.equal(readLibraryKind({ kind: 'spreadsheets' }), null);
  assert.equal(readLibraryKind(null), null);
});
