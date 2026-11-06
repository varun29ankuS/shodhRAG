/**
 * The one diff view's rules (`features/code/diffModel.ts`): the backend's diff read
 * safely, side-by-side rows, the unchanged lines between hunks, long files folded, line
 * selection and the language a path is highlighted as. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/codeDiff.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  changeCounts,
  extendSelection,
  foldedHunks,
  hiddenBefore,
  isSelected,
  languageOf,
  opsAsChange,
  parseFileChanges,
  numberOn,
  selectedLines,
  selectedRun,
  sideOf,
  sideBySide,
  textAsChange,
} from '../src/features/code/diffModel.ts';
import type { DiffLine, FileChange } from '../src/features/code/diffModel.ts';

const same = (old: number, next: number, text: string): DiffLine => ({ kind: 'same', text, old, new: next });
const added = (next: number, text: string): DiffLine => ({ kind: 'added', text, old: null, new: next });
const removed = (old: number, text: string): DiffLine => ({ kind: 'removed', text, old, new: null });

test('a diff from the backend is read only when it has the expected shape', () => {
  const payload = [
    {
      path: 'src/lib.rs',
      status: 'changed',
      binary: false,
      tooLarge: false,
      hunks: [{ lines: [{ kind: 'same', text: 'a', old: 1, new: 1 }, { kind: 'added', text: 'b', old: null, new: 2 }] }],
    },
  ];
  const parsed = parseFileChanges(payload);
  assert.ok(parsed);
  assert.equal(parsed[0].hunks[0].lines[1].kind, 'added');
  assert.equal(parseFileChanges('nope'), null);
  assert.equal(parseFileChanges([{ ...payload[0], status: 'moved' }]), null);
  assert.equal(parseFileChanges([{ ...payload[0], hunks: [{ lines: [{ kind: 'added', text: 'b', old: null, new: 0 }] }] }]), null);
  assert.equal(parseFileChanges([{ ...payload[0], hunks: [{ lines: [{ kind: 'x', text: 'b', old: 1, new: 1 }] }] }]), null);
});

test('side by side pairs each run of removed lines with the added lines after it', () => {
  const rows = sideBySide({
    lines: [same(1, 1, 'a'), removed(2, 'b'), removed(3, 'c'), added(2, 'B'), same(4, 3, 'd'), added(4, 'e')],
  });
  assert.deepEqual(
    rows.map(r => [r.left?.text ?? null, r.right?.text ?? null]),
    [
      ['a', 'a'],
      ['b', 'B'],
      ['c', null],
      ['d', 'd'],
      [null, 'e'],
    ],
  );
});

test('the unchanged lines between hunks are counted on the old side', () => {
  const hunks = [{ lines: [same(4, 4, 'x'), removed(5, 'y')] }, { lines: [same(20, 19, 'z'), added(20, 'w')] }];
  assert.equal(hiddenBefore(hunks, 0), 3, 'lines 1–3 above the first hunk');
  assert.equal(hiddenBefore(hunks, 1), 14, 'lines 6–19 between them');
  assert.equal(hiddenBefore([{ lines: [added(1, 'new')] }], 0), 0, 'a new file hides nothing');
});

test('a long file is folded after whole hunks, and a huge first hunk is cut', () => {
  const big = (n: number, from: number) => ({ lines: Array.from({ length: n }, (_, i) => added(from + i, `l${i}`)) });
  const change: FileChange = { path: 'a.ts', status: 'added', binary: false, tooLarge: false, hunks: [big(100, 1), big(100, 200)] };
  const folded = foldedHunks(change, 160);
  assert.equal(folded.hunks.length, 1);
  assert.equal(folded.hidden, 100);
  const one: FileChange = { ...change, hunks: [big(300, 1)] };
  const cut = foldedHunks(one, 160);
  assert.equal(cut.hunks[0].lines.length, 160);
  assert.equal(cut.hidden, 140);
  assert.equal(foldedHunks({ ...change, hunks: [big(10, 1)] }, 160).hidden, 0);
});

test('lines are selected on their own side, extended with Shift, in one file', () => {
  const lines = [same(200, 230, 'a'), removed(201, 'gone'), added(231, 'new'), same(202, 232, 'b')];
  let selection = extendSelection(null, 'a.rs', lines[3], false);
  assert.deepEqual(selection, { path: 'a.rs', side: 'new', from: 232, to: 232 });
  selection = extendSelection(selection, 'a.rs', lines[0], true);
  assert.deepEqual(selection, { path: 'a.rs', side: 'new', from: 230, to: 232 });
  assert.ok(isSelected(selection, 'a.rs', lines[2]));
  assert.ok(!isSelected(selection, 'a.rs', lines[1]), 'a removed line has no new number');
  assert.ok(!isSelected(selection, 'b.rs', lines[0]));
  // A removed line selects on the old side; Shift on the other side starts again.
  const old = extendSelection(selection, 'a.rs', lines[1], true);
  assert.deepEqual(old, { path: 'a.rs', side: 'old', from: 201, to: 201 });
});

test('a selection takes the lines shown between its ends, never a far line that shares a number', () => {
  // Old line 205 sits far away from new lines 205–206 (thirty lines were added above).
  const change: FileChange = {
    path: 'a.rs',
    status: 'changed',
    binary: false,
    tooLarge: false,
    hunks: [
      { lines: [same(204, 174, 'x'), removed(205, 'far away, old 205')] },
      { lines: [same(234, 204, 'y'), added(205, 'new 205'), removed(236, 'old next to it'), same(237, 206, 'z')] },
    ],
  };
  const picked = selectedLines(change, { path: 'a.rs', side: 'new', from: 205, to: 206 });
  assert.deepEqual(picked.map(l => l.text), ['new 205', 'old next to it', 'z']);
  assert.equal(selectedRun('a.rs', [], { path: 'a.rs', side: 'new', from: 1, to: 2 }).length, 0);
  const file = textAsChange('a.rs', 'one\ntwo\nthree\n', 10);
  assert.deepEqual(selectedLines(file, { path: 'a.rs', side: 'new', from: 11, to: 12 }).map(l => l.text), ['two', 'three']);
  assert.equal(selectedLines(file, null).length, 3);
  assert.equal(numberOn(removed(3, 'r'), 'new'), null);
  assert.equal(sideOf(removed(3, 'r')), 'old');
});

test('a note says why a file is not shown, when it is not the file itself', () => {
  const parsed = parseFileChanges([
    { path: '.env', status: 'added', binary: false, tooLarge: false, hunks: [], note: 'Not shown: this file may hold secrets.' },
  ]);
  assert.equal(parsed?.[0].note, 'Not shown: this file may hold secrets.');
  assert.equal(parseFileChanges([{ path: 'a', status: 'added', binary: false, tooLarge: false, hunks: [] }])?.[0].note, null);
});

test('an instruction diff becomes a numbered change', () => {
  const change = opsAsChange('Instructions', [
    { op: 'same', text: 'a' },
    { op: 'removed', text: 'b' },
    { op: 'added', text: 'c' },
  ]);
  assert.deepEqual(change.hunks[0].lines.map(l => [l.kind, l.old, l.new]), [
    ['same', 1, 1],
    ['removed', 2, null],
    ['added', null, 2],
  ]);
  assert.deepEqual(changeCounts(change), { added: 1, removed: 1 });
  assert.equal(opsAsChange('x', []).hunks.length, 0);
});

test('paths are highlighted by their language', () => {
  assert.equal(languageOf('crates/a/src/lib.rs'), 'rust');
  assert.equal(languageOf('app/src/App.tsx'), 'tsx');
  assert.equal(languageOf('Dockerfile'), 'docker');
  assert.equal(languageOf('README'), 'text');
  assert.equal(languageOf('.gitignore'), 'text');
});
