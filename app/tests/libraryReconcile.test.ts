/**
 * Library / index reconciliation tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/libraryReconcile.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { addableFolders, isDriveRoot, reconcileSources, safeRemovals, searchedSourceCount } from '../src/features/library/reconcile.ts';

const folders = (...paths: string[]) => paths.map(path => ({ path }));

test('files inside listed folders are not unlisted', () => {
  const result = reconcileSources(folders('C:\\Docs', 'D:\\Papers'), ['c:/docs/a.pdf', 'c:/docs/sub/b.pdf', 'd:/papers/c.pdf']);
  assert.deepEqual(result, { unlisted: [], groups: [] });
});

test('with no folders every file is unlisted and grouped by folder', () => {
  const result = reconcileSources([], ['c:/docs/a.pdf', 'c:/docs/b.pdf', 'c:/notes/c.md']);
  assert.deepEqual(result.unlisted, ['c:/docs/a.pdf', 'c:/docs/b.pdf', 'c:/notes/c.md']);
  assert.deepEqual(result.groups, [
    { folder: 'c:/docs', files: 2 },
    { folder: 'c:/notes', files: 1 },
  ]);
});

test('nested folders: a listed subfolder covers its files, the rest group under their parents', () => {
  const result = reconcileSources(folders('C:\\Work\\Contracts'), [
    'c:/work/contracts/a.pdf',
    'c:/work/contracts/2024/b.pdf',
    'c:/work/notes/c.md',
    'c:/work/notes/old/d.md',
    'c:/work/e.txt',
  ]);
  assert.deepEqual(result.unlisted, ['c:/work/notes/c.md', 'c:/work/notes/old/d.md', 'c:/work/e.txt']);
  // c:/work/notes and c:/work/notes/old collapse into c:/work, which holds e.txt.
  assert.deepEqual(result.groups, [{ folder: 'c:/work', files: 3 }]);
});

test('collapsing only merges real subfolders, not names sharing a prefix', () => {
  const result = reconcileSources([], ['c:/doc/a.pdf', 'c:/docs/b.pdf', 'c:/docs/c.pdf']);
  assert.deepEqual(result.groups, [
    { folder: 'c:/docs', files: 2 },
    { folder: 'c:/doc', files: 1 },
  ]);
});

test('a folder name that prefixes another does not cover it', () => {
  const result = reconcileSources(folders('C:\\Doc'), ['c:/docs/b.pdf']);
  assert.deepEqual(result.unlisted, ['c:/docs/b.pdf']);
});

test('files in a drive root group under the drive, which takes in every group on it', () => {
  const result = reconcileSources([], ['C:/a.pdf', 'c:/docs/b.pdf', 'd:/x/c.pdf']);
  assert.deepEqual(result.groups, [
    { folder: 'C:', files: 2 },
    { folder: 'd:/x', files: 1 },
  ]);
  assert.equal(isDriveRoot('C:'), true);
  assert.equal(isDriveRoot('c:/docs'), false);
});

test('a folder picked at a drive root, or with a trailing slash, covers its files', () => {
  assert.deepEqual(reconcileSources(folders('C:\\'), ['c:/a.pdf', 'c:/docs/b.pdf']).unlisted, []);
  assert.deepEqual(reconcileSources(folders('C:\\Docs\\'), ['c:/docs/b.pdf']).unlisted, []);
});

test('paths compare across slashes and case', () => {
  assert.deepEqual(reconcileSources(folders('c:/docs'), ['C:\\Docs\\A.pdf']).unlisted, []);
  const result = reconcileSources(folders('C:\\Other'), ['C:\\Docs\\A.pdf']);
  assert.deepEqual(result.unlisted, ['C:/Docs/A.pdf']);
  assert.deepEqual(result.groups, [{ folder: 'C:/Docs', files: 1 }]);
});

test('a single-file source covers that file', () => {
  assert.deepEqual(reconcileSources(folders('C:\\Downloads\\paper.pdf'), ['c:/downloads/paper.pdf']).unlisted, []);
});

test('note, calendar and other scheme sources are not folder files', () => {
  const result = reconcileSources([], ['note://abc', 'calendar://task/1', 'calendar://event/2', 'https://example.com/a', '', 'c:/docs/a.pdf']);
  assert.deepEqual(result.unlisted, ['c:/docs/a.pdf']);
  assert.deepEqual(result.groups, [{ folder: 'c:/docs', files: 1 }]);
});

test('a file indexed under several documents counts once', () => {
  const result = reconcileSources([], ['c:/docs/a.pdf', 'C:\\Docs\\a.pdf', 'c:/docs/b.pdf']);
  assert.deepEqual(result.unlisted, ['c:/docs/a.pdf', 'c:/docs/b.pdf']);
  assert.deepEqual(result.groups, [{ folder: 'c:/docs', files: 2 }]);
});

test('POSIX paths group under their folders and the root', () => {
  const result = reconcileSources(folders('/Users/me/Papers'), ['/Users/me/Papers/a.pdf', '/Users/me/Notes/b.md', '/c.txt']);
  assert.deepEqual(result.unlisted, ['/Users/me/Notes/b.md', '/c.txt']);
  assert.deepEqual(result.groups, [{ folder: '/', files: 2 }]);
});

test('UNC paths are files, not schemes', () => {
  const result = reconcileSources([], ['\\\\server\\share\\docs\\a.pdf']);
  assert.deepEqual(result.groups, [{ folder: '//server/share/docs', files: 1 }]);
});

test('an empty index has nothing unlisted', () => {
  assert.deepEqual(reconcileSources(folders('C:\\Docs'), []), { unlisted: [], groups: [] });
  assert.deepEqual(reconcileSources([], []), { unlisted: [], groups: [] });
});

test('groups sort by file count, then by path', () => {
  const result = reconcileSources([], ['c:/b/1.pdf', 'c:/a/1.pdf', 'c:/c/1.pdf', 'c:/c/2.pdf']);
  assert.deepEqual(result.groups.map(g => g.folder), ['c:/c', 'c:/a', 'c:/b']);
});

test('the sources count includes unlisted files unless answers are limited to some folders', () => {
  assert.equal(searchedSourceCount([], 4), 4);
  assert.equal(searchedSourceCount([{ selected: true }, { selected: true }], 3), 5);
  // Only some folders included: answers search just those, not the unlisted files.
  assert.equal(searchedSourceCount([{ selected: true }, { selected: false }], 3), 1);
  // None included: answers search everything.
  assert.equal(searchedSourceCount([{ selected: false }], 2), 3);
});

test('folders offered for adding leave out drive roots and folders holding a listed folder', () => {
  const groups = [
    { folder: 'C:', files: 3 },
    { folder: 'd:/work', files: 2 },
    { folder: 'd:/papers', files: 1 },
    { folder: '//server/share', files: 1 },
  ];
  assert.deepEqual(addableFolders(groups, folders('D:\\Work\\Contracts')), ['d:/papers']);
  assert.deepEqual(addableFolders([], folders('D:\\Work')), []);
});

test('files are removed one by one, never one whose path starts another kept file', () => {
  const indexed = ['c:/x/a.pdf', 'c:/x/a.pdf.md', 'c:/y/b.pdf', 'c:/y/b.pdf.bak'];
  // a.pdf.md is listed (kept): removing a.pdf by prefix would take it too.
  assert.deepEqual(safeRemovals(['c:/x/a.pdf', 'c:/y/b.pdf', 'c:/y/b.pdf.bak'], indexed), ['c:/y/b.pdf', 'c:/y/b.pdf.bak']);
  assert.deepEqual(safeRemovals(['C:/X/A.pdf'], ['c:/x/a.pdf']), ['C:/X/A.pdf']);
  assert.deepEqual(safeRemovals([], indexed), []);
});
