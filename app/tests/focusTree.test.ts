/**
 * Focus pop-out drill-down: thread tree (build, flatten, keyboard), and
 * backward-compatible parsing of threads saved before nesting existed.
 *   node --experimental-strip-types --test app/tests/focusTree.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  MAX_DEPTH,
  buildThreadTree,
  descendantCount,
  effectiveParents,
  findChildThread,
  flattenTree,
  rootThreads,
  threadPath,
  treeKey,
} from '../src/features/focus/threadTree.ts';
import { readThread, readThreads } from '../src/features/focus/threadStore.ts';
import type { FocusTarget, FocusThread } from '../src/features/focus/focusTypes.ts';

const eq = (label: string): FocusTarget => ({ kind: 'equation', label, tex: label });

function thread(id: string, parentThreadId?: string, turns = 0): FocusThread {
  const t: FocusThread = {
    id,
    anchor: { conversationId: 'c', parentMessageId: 'm', target: eq(`T ${id}`) },
    turns: Array.from({ length: turns }, (_, i) => ({
      id: `${id}-${i}`,
      role: i % 2 === 0 ? 'user' : 'assistant',
      content: `turn ${i}`,
      timestamp: String(i),
    })),
    createdAt: '0',
    updatedAt: '0',
  };
  if (parentThreadId) t.parentThreadId = parentThreadId;
  return t;
}

test('old threads (no parent fields) parse unchanged as roots', () => {
  const stored = {
    id: 'old',
    anchor: { conversationId: 'c', parentMessageId: 'm', target: { kind: 'equation', label: 'E', tex: 'e' } },
    turns: [{ id: 't', role: 'user', content: 'q', timestamp: '1' }],
    createdAt: '1',
    updatedAt: '2',
  };
  const read = readThread(stored);
  assert.ok(read);
  assert.equal(read.parentThreadId, undefined);
  assert.equal(read.parentTurnId, undefined);
  assert.deepEqual(rootThreads([read]).map(t => t.id), ['old']);
});

test('nested threads keep their parent ids; bad ones are dropped', () => {
  const base = { anchor: { conversationId: 'c', parentMessageId: 'm', target: { kind: 'equation', label: 'E', tex: 'e' } }, turns: [] };
  const [a, b, c, d] = readThreads([
    { ...base, id: 'a' },
    { ...base, id: 'b', parentThreadId: 'a', parentTurnId: 'a-1' },
    { ...base, id: 'c', parentThreadId: 'c', parentTurnId: 'x' },
    { ...base, id: 'd', parentThreadId: 42, parentTurnId: 'x' },
  ]);
  assert.equal(a.parentThreadId, undefined);
  assert.equal(b.parentThreadId, 'a');
  assert.equal(b.parentTurnId, 'a-1');
  assert.equal(c.parentThreadId, undefined, 'self-parent is ignored');
  assert.equal(d.parentThreadId, undefined, 'non-string parent is ignored');
});

test('turn followups and summary markers parse with caps', () => {
  const read = readThread({
    id: 'x',
    anchor: { conversationId: 'c', parentMessageId: null, target: { kind: 'equation', label: 'E', tex: 'e' } },
    turns: [
      { id: 'u', role: 'user', content: 's', timestamp: '1', summaryOf: { threadId: 'child', label: 'Spline' } },
      { id: 'a', role: 'assistant', content: 'x', timestamp: '2', followups: ['One?', 7, 'Two?', 'Three?', 'Four?', 'x'.repeat(500)] },
      { id: 'u2', role: 'user', content: 'q', timestamp: '3', followups: ['ignored on user turns'] },
    ],
  });
  assert.ok(read);
  assert.deepEqual(read.turns[0].summaryOf, { threadId: 'child', label: 'Spline' });
  assert.deepEqual(read.turns[1].followups, ['One?', 'Two?', 'Three?']);
  assert.equal(read.turns[2].followups, undefined);
});

test('tree: children under parents, orphans and loops become roots', () => {
  const list = [
    thread('root'),
    thread('child', 'root'),
    thread('grand', 'child'),
    thread('orphan', 'gone'),
    thread('loopA', 'loopB'),
    thread('loopB', 'loopA'),
    thread('underLoop', 'loopA'),
  ];
  const parents = effectiveParents(list);
  assert.equal(parents.get('orphan'), null);
  assert.equal(parents.get('loopA'), null);
  assert.equal(parents.get('loopB'), null);
  assert.equal(parents.get('underLoop'), 'loopA');
  const roots = buildThreadTree(list);
  assert.deepEqual(roots.map(r => r.thread.id), ['root', 'orphan', 'loopA', 'loopB']);
  assert.deepEqual(roots[0].children.map(c => c.thread.id), ['child']);
  assert.deepEqual(roots[0].children[0].children.map(c => c.thread.id), ['grand']);
  assert.deepEqual(rootThreads(list).map(t => t.id), ['root', 'orphan', 'loopA', 'loopB']);
  assert.equal(descendantCount(list, 'root'), 2);
  assert.equal(descendantCount(list, 'missing'), 0);
});

test('threadPath walks from the root and survives loops', () => {
  const list = [thread('r'), thread('a', 'r'), thread('b', 'a'), thread('x', 'y'), thread('y', 'x')];
  assert.deepEqual(threadPath(list, 'b').map(t => t.id), ['r', 'a', 'b']);
  assert.deepEqual(threadPath(list, 'x').map(t => t.id), ['x']);
  assert.deepEqual(threadPath(list, 'nope'), []);
});

test('findChildThread matches target and parent, so roots are not hijacked', () => {
  const root = thread('r');
  const child = { ...thread('c', 'r'), anchor: { ...root.anchor } };
  const list = [root, child];
  assert.equal(findChildThread(list, root.anchor.target, null)?.id, 'r');
  assert.equal(findChildThread(list, root.anchor.target, 'r')?.id, 'c');
  assert.equal(findChildThread(list, eq('other'), 'r'), null);
});

test('flatten respects collapsed nodes and reports aria positions', () => {
  const list = [thread('r', undefined, 2), thread('a', 'r'), thread('b', 'r'), thread('a1', 'a')];
  const roots = buildThreadTree(list);
  const all = flattenTree(roots);
  assert.deepEqual(all.map(n => [n.id, n.level, n.posInSet, n.setSize]), [
    ['r', 1, 1, 1],
    ['a', 2, 1, 2],
    ['a1', 3, 1, 1],
    ['b', 2, 2, 2],
  ]);
  assert.equal(all[0].label, '1 reply about T r');
  assert.equal(all[1].label, 'T a', 'a thread with no turns shows its object');
  const folded = flattenTree(roots, new Set(['a']));
  assert.deepEqual(folded.map(n => n.id), ['r', 'a', 'b']);
  assert.equal(folded[1].expanded, false);
  assert.equal(folded[1].hasChildren, true);
});

test('tree keyboard map', () => {
  const list = [thread('r'), thread('a', 'r'), thread('a1', 'a'), thread('b', 'r')];
  const roots = buildThreadTree(list);
  const rows = flattenTree(roots);
  assert.deepEqual(treeKey(rows, 'r', 'ArrowDown'), { type: 'focus', id: 'a' });
  assert.deepEqual(treeKey(rows, 'a', 'ArrowUp'), { type: 'focus', id: 'r' });
  assert.equal(treeKey(rows, 'r', 'ArrowUp'), null);
  assert.deepEqual(treeKey(rows, 'a', 'ArrowLeft'), { type: 'collapse', id: 'a' });
  assert.deepEqual(treeKey(rows, 'a1', 'ArrowLeft'), { type: 'focus', id: 'a' });
  assert.deepEqual(treeKey(rows, 'a', 'ArrowRight'), { type: 'focus', id: 'a1' });
  const folded = flattenTree(roots, new Set(['a']));
  assert.deepEqual(treeKey(folded, 'a', 'ArrowRight'), { type: 'expand', id: 'a' });
  assert.equal(treeKey(rows, 'b', 'ArrowRight'), null);
  assert.deepEqual(treeKey(rows, 'a1', 'Home'), { type: 'focus', id: 'r' });
  assert.deepEqual(treeKey(rows, 'r', 'End'), { type: 'focus', id: 'b' });
  assert.deepEqual(treeKey(rows, 'b', 'Enter'), { type: 'activate', id: 'b' });
  assert.deepEqual(treeKey(rows, 'b', ' '), { type: 'activate', id: 'b' });
  assert.deepEqual(treeKey(rows, 'gone', 'ArrowDown'), { type: 'focus', id: 'r' });
  assert.equal(treeKey(rows, 'r', 'x'), null);
});

test('depth cap is six nested levels', () => {
  assert.equal(MAX_DEPTH, 6);
});
