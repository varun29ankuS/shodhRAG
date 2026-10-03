/**
 * Viewer keyboard map and find-in-document tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/viewerKeys.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { findInText, viewerCommand } from '../src/features/ask/viewer/viewerKeys.ts';
import type { KeyInput } from '../src/features/ask/viewer/viewerKeys.ts';

const press = (key: string, opts: Partial<KeyInput> = {}) =>
  viewerCommand({ key, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, editable: false, ...opts });

test('page navigation keys', () => {
  assert.equal(press('PageDown'), 'nextPage');
  assert.equal(press('PageUp'), 'prevPage');
  assert.equal(press('Home'), 'firstPage');
  assert.equal(press('End'), 'lastPage');
  assert.equal(press('Home', { shiftKey: true }), null);
});

test('zoom keys: + arrives as = with or without Shift', () => {
  assert.equal(press('='), 'zoomIn');
  assert.equal(press('+', { shiftKey: true }), 'zoomIn');
  assert.equal(press('-'), 'zoomOut');
  assert.equal(press('_', { shiftKey: true }), 'zoomOut');
  assert.equal(press('0'), 'fitWidth');
});

test('Ctrl/Cmd zoom chords zoom the document instead of the window', () => {
  assert.equal(press('=', { ctrlKey: true }), 'zoomIn');
  assert.equal(press('+', { ctrlKey: true, shiftKey: true }), 'zoomIn');
  assert.equal(press('-', { metaKey: true }), 'zoomOut');
  assert.equal(press('0', { ctrlKey: true }), 'fitWidth');
});

test('focus mode and find', () => {
  assert.equal(press('f'), 'toggleFocus');
  assert.equal(press('F', { shiftKey: true }), 'toggleFocus');
  assert.equal(press('f', { ctrlKey: true }), 'find');
  assert.equal(press('F', { metaKey: true }), 'find');
  assert.equal(press('F3'), 'findNext');
  assert.equal(press('F3', { shiftKey: true }), 'findPrev');
  assert.equal(press('g', { ctrlKey: true }), 'findNext');
  assert.equal(press('G', { ctrlKey: true, shiftKey: true }), 'findPrev');
});

test('single keys do nothing while typing; chords still work', () => {
  for (const key of ['f', '0', '=', '+', '-', 'PageDown', 'Home', 'End']) {
    assert.equal(press(key, { editable: true }), null, key);
  }
  assert.equal(press('f', { editable: true, ctrlKey: true }), 'find');
  assert.equal(press('=', { editable: true, ctrlKey: true }), 'zoomIn');
  assert.equal(press('F3', { editable: true }), 'findNext');
});

test('Alt combinations and unrelated keys are left alone', () => {
  assert.equal(press('f', { altKey: true }), null);
  assert.equal(press('PageDown', { altKey: true }), null);
  assert.equal(press('a'), null);
  assert.equal(press('ArrowDown'), null);
  assert.equal(press(' '), null);
  assert.equal(press('c', { ctrlKey: true }), null);
  assert.equal(press('F', { ctrlKey: true, shiftKey: true }), null);
});

test('find ignores case and treats whitespace runs as one space', () => {
  const text = 'The Transformer  uses\nself-attention. the transformer';
  assert.deepEqual(findInText(text, 'transformer'), [
    { start: 4, end: 15 },
    { start: 42, end: 53 },
  ]);
  const [hit] = findInText(text, 'transformer uses');
  assert.equal(text.slice(hit.start, hit.end), 'Transformer  uses');
  assert.deepEqual(findInText(text, '   '), []);
  assert.deepEqual(findInText(text, 'missing'), []);
});

test('find does not overlap matches and keeps offsets aligned for unusual case mappings', () => {
  assert.deepEqual(findInText('aaaa', 'aa'), [
    { start: 0, end: 2 },
    { start: 2, end: 4 },
  ]);
  // "İ" lowercases to two code units; offsets must still index the original.
  const text = 'İstanbul and Ankara';
  const [hit] = findInText(text, 'ankara');
  assert.equal(text.slice(hit.start, hit.end), 'Ankara');
});
