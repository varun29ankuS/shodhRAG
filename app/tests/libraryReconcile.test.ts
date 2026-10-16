/**
 * Library / index reconciliation tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/libraryReconcile.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { addableFolders, isDriveRoot, librarySourcesChip, nativePath, reconcileSources, searchedSourceCount } from '../src/features/library/reconcile.ts';

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

test('the home folder and every folder holding it are never added for you', () => {
  const groups = [
    { folder: 'c:/users/me', files: 4 },
    { folder: 'c:/users', files: 1 },
    { folder: 'c:/users/me/papers', files: 2 },
    { folder: 'c:/users/mel/docs', files: 1 },
  ];
  assert.deepEqual(addableFolders(groups, [], ['C:\\Users\\Me']), ['c:/users/me/papers', 'c:/users/mel/docs']);
  // An unknown home folder protects nothing beyond drive roots.
  assert.deepEqual(addableFolders(groups, [], []), groups.map(g => g.folder));
});

test('a group folder is handed to the folder indexer in the platform form', () => {
  assert.equal(nativePath('c:/users/me/papers', true), 'c:\\users\\me\\papers');
  assert.equal(nativePath('//server/share/docs', true), '\\\\server\\share\\docs');
  assert.equal(nativePath('/Users/me/Papers', false), '/Users/me/Papers');
});

test('the sources chip: one label for the top bar and the composer', () => {
  assert.equal(librarySourcesChip([], 0).label, 'No sources yet');
  assert.equal(librarySourcesChip([], 4).label, 'All sources · 4');
  assert.match(librarySourcesChip([], 4).title, /4 indexed files in no folder/);
  const all = librarySourcesChip([{ selected: true }, { selected: true }], 3);
  assert.equal(all.label, 'All sources · 5');
  assert.match(all.title, /2 folders and 3 indexed files in no folder/);
  assert.equal(librarySourcesChip([{ selected: true }], 0).title, 'Answers search everything you have indexed. Manage sources in Library.');
  // Only some folders included in Ask: answers search just those.
  const limited = librarySourcesChip([{ selected: true }, { selected: false }, { selected: true }], 3);
  assert.equal(limited.label, '2 of 3 folders');
  assert.match(limited.title, /only the 2 folders included in Ask/);
});
