/**
 * Snippets in the UI: focus target round trip through the thread store,
 * the chat context block, record validation, and the gallery union.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/snippetModel.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  appendToDraft,
  parseTags,
  readSnippet,
  snippetChatContext,
  snippetLabel,
  snippetTarget,
} from '../src/features/research/snippetModel.ts';
import { readTarget, sameTarget, sideScope } from '../src/features/focus/threadStore.ts';
import { buildContextBlock } from '../src/features/focus/contextBlock.ts';
import { paperOf } from '../src/features/focus/targets.ts';
import { entryCounts, filterEntries, mergeGallery, replaceSnippet, snippetMatches } from '../src/features/research/galleryUnion.ts';
import type { Snippet } from '../src/features/research/types.ts';
import type { VisualSummary } from '../src/features/visuals/model.ts';

function snippet(overrides: Partial<Snippet> = {}): Snippet {
  return {
    id: 'snippet:1',
    statementId: 'st-1',
    filePath: 'C:/papers/deltanet.pdf',
    fileName: 'deltanet.pdf',
    page: 4,
    rect: { x: 72, y: 92, width: 228, height: 100 },
    text: 'Table 2: Results\nDeltaNet 17.7',
    title: '',
    note: '',
    tags: [],
    kind: 'table',
    hasImage: true,
    latex: null,
    latexModel: null,
    scope: 'global',
    createdAt: '2026-10-01T10:00:00Z',
    updatedAt: '2026-10-01T10:00:00Z',
    ...overrides,
  };
}

test('a snippet target keeps id, place and text and survives the thread store', () => {
  const target = snippetTarget(snippet());
  assert.equal(target.kind, 'snippet');
  assert.equal(target.label, 'Table · deltanet.pdf p.4');
  const stored = JSON.parse(JSON.stringify(target));
  assert.deepEqual(readTarget(stored), target);
  assert.equal(JSON.stringify(target).includes('data:'), false);
  // Invalid stored snippets are dropped.
  assert.equal(readTarget({ ...stored, rect: { x: 1, y: 1, width: 0, height: 5 } }), null);
  assert.equal(readTarget({ ...stored, page: 0 }), null);
  assert.equal(readTarget({ ...stored, snippetId: '' }), null);
  // Same snippet = same id, even after its title changed.
  assert.equal(sameTarget(target, snippetTarget(snippet({ title: 'Renamed' }))), true);
  assert.equal(sameTarget(target, snippetTarget(snippet({ id: 'snippet:2' }))), false);
});

test('Expand & ask gets the region text and its source; questions stay on the paper', () => {
  const target = snippetTarget(snippet());
  const block = buildContextBlock(target);
  assert.match(block, /snippet from deltanet\.pdf page 4, text of the region/);
  assert.match(block, /DeltaNet 17\.7/);
  const scope = sideScope(target, {});
  assert.deepEqual(scope, { sourceIds: [], sourceFiles: ['C:/papers/deltanet.pdf'], pages: [3, 4, 5] });
  const paper = paperOf(target);
  assert.deepEqual(paper?.rects, [{ page: 4, rect: { x: 72, y: 92, width: 228, height: 100 } }]);
});

test('Add to chat quotes the text with its source and id; LaTeX wins when transcribed', () => {
  const block = snippetChatContext(snippet({ title: 'Main results' }));
  assert.equal(block, 'Snippet “Main results” (table) from deltanet.pdf, page 4 [id snippet:1]:\n> Table 2: Results\n> DeltaNet 17.7\n\n');
  const eq = snippetChatContext(snippet({ kind: 'equation', latex: 'E = mc^2' }));
  assert.match(eq, /> \$\$\n> E = mc\^2\n> \$\$/);
  assert.equal(appendToDraft('', 'x'), 'x');
  assert.equal(appendToDraft('Compare these  \n', 'block'), 'Compare these\n\nblock');
});

test('records are validated and labels fall back to kind and place', () => {
  assert.equal(readSnippet({ id: 'a' }), null);
  const read = readSnippet({ ...snippet(), kind: 'unknown', tags: ['x', 3] });
  assert.equal(read?.kind, 'passage');
  assert.deepEqual(read?.tags, ['x']);
  assert.equal(snippetLabel(snippet({ title: '  Figure 3 ' })), 'Figure 3');
  assert.deepEqual(parseTags('ann, Graphs ,graphs;; recall@10'), ['ann', 'Graphs', 'recall@10']);
});

function visual(rootId: string, updatedAt: string, pinned = false): VisualSummary {
  return {
    id: `${rootId}-v1`,
    rootId,
    parentId: null,
    version: 1,
    conversationId: 'c1',
    messageId: null,
    threadId: null,
    turnId: null,
    kind: 'chart',
    title: rootId,
    source: '{}',
    params: {},
    contentHash: rootId,
    pinned,
    note: '',
    instruction: null,
    createdBy: 'capture',
    createdAt: updatedAt,
    updatedAt,
    versionCount: 1,
    firstCreatedAt: updatedAt,
  };
}

test('snippets join the gallery as their own kind, sorted with visuals', () => {
  const entries = mergeGallery(
    [visual('old', '2026-09-01T00:00:00Z'), visual('pinned', '2026-08-01T00:00:00Z', true)],
    [snippet({ updatedAt: '2026-09-15T00:00:00Z' })],
  );
  assert.deepEqual(entries.map(e => e.key), ['visual:pinned', 'snippet:snippet:1', 'visual:old']);
  assert.deepEqual(filterEntries(entries, 'snippet').map(e => e.kind), ['snippet']);
  assert.deepEqual(entryCounts(entries), { chart: 2, snippet: 1 });
  assert.equal(snippetMatches(snippet(), 'deltanet results'), true);
  assert.equal(snippetMatches(snippet(), 'mamba'), false);
  const renamed = replaceSnippet(entries, 'snippet:1', snippet({ title: 'New', updatedAt: '2026-10-02T00:00:00Z' }));
  assert.equal(renamed.length, 3);
  assert.equal(replaceSnippet(entries, 'snippet:1', null).length, 2);
});
