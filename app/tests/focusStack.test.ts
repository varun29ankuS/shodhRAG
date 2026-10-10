/**
 * Focus pop-out levels: stack navigation, breadcrumbs and the keyboard map.
 *   node --experimental-strip-types --test app/tests/focusStack.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { canPush, crumbs, initialStack, stackKey, stackReducer } from '../src/features/focus/focusStack.ts';
import type { StackState } from '../src/features/focus/focusStack.ts';
import { MAX_DEPTH } from '../src/features/focus/threadTree.ts';

const push = (s: StackState<string>, level: string) => stackReducer(s, { type: 'push', level });

test('push, back, forward, jump', () => {
  let s = initialStack('answer');
  s = push(s, 'eq');
  s = push(s, 'diagram');
  assert.deepEqual(s, { levels: ['answer', 'eq', 'diagram'], index: 2 });
  s = stackReducer(s, { type: 'back' });
  assert.equal(s.index, 1);
  s = stackReducer(s, { type: 'forward' });
  assert.equal(s.index, 2);
  assert.equal(stackReducer(s, { type: 'forward' }), s, 'forward at the end is a no-op');
  s = stackReducer(s, { type: 'jump', index: 0 });
  assert.equal(s.index, 0);
  assert.equal(stackReducer(s, { type: 'back' }), s, 'back at the root is a no-op');
  assert.equal(stackReducer(s, { type: 'jump', index: 9 }), s);
  assert.equal(stackReducer(s, { type: 'jump', index: -1 }), s);
  assert.equal(stackReducer(s, { type: 'jump', index: 0 }), s);
});

test('push from an earlier level drops the levels ahead', () => {
  let s = initialStack('a');
  s = push(push(s, 'b'), 'c');
  s = stackReducer(s, { type: 'jump', index: 0 });
  s = push(s, 'x');
  assert.deepEqual(s, { levels: ['a', 'x'], index: 1 });
});

test('depth cap: no level beyond MAX_DEPTH nested levels', () => {
  let s = initialStack('root');
  for (let i = 0; i < MAX_DEPTH; i++) {
    assert.ok(canPush(s));
    s = push(s, `l${i}`);
  }
  assert.equal(s.index, MAX_DEPTH);
  assert.equal(canPush(s), false);
  assert.equal(push(s, 'too deep'), s);
});

test('reset replaces the levels (map jump), capped', () => {
  const s = stackReducer(initialStack('a'), { type: 'reset', levels: ['r', 'x', 'y'] });
  assert.deepEqual(s, { levels: ['r', 'x', 'y'], index: 2 });
  const long = Array.from({ length: MAX_DEPTH + 4 }, (_, i) => `l${i}`);
  const capped = stackReducer(initialStack('a'), { type: 'reset', levels: long });
  assert.equal(capped.levels.length, MAX_DEPTH + 1);
  assert.equal(capped.index, MAX_DEPTH);
  const empty = initialStack('a');
  assert.equal(stackReducer(empty, { type: 'reset', levels: [] }), empty);
  assert.equal(stackReducer(empty, { type: 'reset', levels: ['p', 'q'], index: 0 }).index, 0);
});

test('keyboard: Alt+arrows anywhere, Backspace only outside text fields', () => {
  const k = (key: string, over: Partial<Parameters<typeof stackKey>[0]> = {}) =>
    stackKey({ key, altKey: false, ctrlKey: false, metaKey: false, shiftKey: false, editable: false, ...over });
  assert.equal(k('ArrowLeft', { altKey: true }), 'back');
  assert.equal(k('ArrowRight', { altKey: true }), 'forward');
  assert.equal(k('ArrowLeft', { altKey: true, editable: true }), 'back');
  assert.equal(k('ArrowLeft'), null, 'plain arrows pan the stage');
  assert.equal(k('ArrowLeft', { altKey: true, shiftKey: true }), null);
  assert.equal(k('ArrowLeft', { altKey: true, ctrlKey: true }), null);
  assert.equal(k('Backspace'), 'back');
  assert.equal(k('Backspace', { editable: true }), null);
  assert.equal(k('Backspace', { ctrlKey: true }), null);
  assert.equal(k('Escape'), null, 'Esc is handled by the dialog');
});

test('breadcrumbs: current level last, long trails collapse the middle', () => {
  assert.deepEqual(crumbs(['Answer eq', 'Spline'], 1), [
    { type: 'level', index: 0, label: 'Answer eq', current: false },
    { type: 'level', index: 1, label: 'Spline', current: true },
  ]);
  assert.deepEqual(crumbs(['a', 'b', 'c'], 0).map(c => c.type === 'level' && c.label), ['a'], 'levels ahead are not shown');
  const long = crumbs(['a', 'b', 'c', 'd', 'e', 'f', 'g'], 6, 5);
  assert.deepEqual(long.map(c => (c.type === 'level' ? c.label : `gap:${c.hidden.join(',')}`)), ['a', 'gap:1,2,3', 'e', 'f', 'g']);
  assert.equal(long[long.length - 1].type === 'level' && long[long.length - 1].current, true);
});
