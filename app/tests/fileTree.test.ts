/**
 * Library file browser model tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/fileTree.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  baseName,
  buildFileTree,
  countTree,
  displayPath,
  extensionOf,
  findDir,
  folderEntries,
  pathKey,
  relativeDir,
  searchFiles,
  typeBadge,
  typeFamily,
} from '../src/features/library/fileTree.ts';

const ROOT = 'C:\\Users\\Asha\\Contracts';

// The index stores Windows paths lowercased with forward slashes.
const row = (path: string, name = 'ignored title') => ({ name, file_path: path, file_type: extensionOf(path), status: 'indexed' });

const ROWS = [
  row('c:/users/asha/contracts/msa.pdf'),
  row('c:/users/asha/contracts/2024/q1/invoice-10.xlsx'),
  row('c:/users/asha/contracts/2024/q1/invoice-2.xlsx'),
  row('c:/users/asha/contracts/2024/notes.md'),
  row('c:/users/asha/contracts/archive/old.docx'),
];

const FAILURES = [
  { file: 'C:\\Users\\Asha\\Contracts\\2024\\Scan.pdf', reason: 'No text layer' },
  // Failed once, indexed later: the indexed row wins.
  { file: 'C:\\Users\\Asha\\Contracts\\MSA.pdf', reason: 'Locked' },
];

test('path helpers', () => {
  assert.equal(pathKey('C:\\A\\B\\'), 'c:/a/b');
  assert.equal(pathKey('/home/a/B/'), '/home/a/B');
  assert.equal(baseName('c:/x/y/report.final.PDF'), 'report.final.PDF');
  assert.equal(extensionOf('report.final.PDF'), 'pdf');
  assert.equal(extensionOf('.gitignore'), '');
  assert.equal(extensionOf('Makefile'), '');
});

test('relative folders are found case-insensitively on Windows and not elsewhere', () => {
  assert.deepEqual(relativeDir(ROOT, 'c:/users/asha/contracts/2024/q1/a.pdf'), ['2024', 'q1']);
  assert.deepEqual(relativeDir(ROOT, 'C:\\Users\\Asha\\Contracts\\Top.pdf'), []);
  assert.equal(relativeDir(ROOT, 'c:/users/asha/contracts-old/a.pdf'), null);
  assert.equal(relativeDir('/home/asha/Docs', '/home/asha/docs/a.pdf'), null);
  assert.deepEqual(relativeDir('/home/asha/Docs', '/home/asha/Docs/Sub/a.pdf'), ['Sub']);
});

test('builds folders from indexed files and failures, deduplicated', () => {
  const tree = buildFileTree(ROOT, ROWS, FAILURES);
  assert.equal(tree.name, 'Contracts');
  assert.deepEqual([...tree.dirs.values()].map(d => d.name).sort(), ['2024', 'archive']);
  assert.deepEqual(tree.files.map(f => [f.name, f.status]), [['msa.pdf', 'indexed']]);
  const y2024 = findDir(tree, ['2024'])!;
  assert.deepEqual(y2024.files.map(f => f.name).sort(), ['Scan.pdf', 'notes.md']);
  const scan = y2024.files.find(f => f.name === 'Scan.pdf')!;
  assert.equal(scan.status, 'failed');
  assert.equal(scan.reason, 'No text layer');
  assert.deepEqual(scan.dir, ['2024']);
  assert.deepEqual(countTree(tree), { files: 6, indexed: 5, failed: 1 });
});

test('file names come from the path, not the document title', () => {
  const tree = buildFileTree(ROOT, [row('c:/users/asha/contracts/msa.pdf', 'Master Services Agreement')], []);
  assert.equal(tree.files[0].name, 'msa.pdf');
});

test('files outside the root are listed at the top level', () => {
  const tree = buildFileTree(ROOT, [row('d:/elsewhere/a.txt')], []);
  assert.deepEqual(tree.files.map(f => f.name), ['a.txt']);
});

test('findDir is case-insensitive and null for missing folders', () => {
  const tree = buildFileTree(ROOT, ROWS, []);
  assert.ok(findDir(tree, ['2024', 'Q1']));
  assert.equal(findDir(tree, ['2025']), null);
  assert.equal(findDir(tree, []), tree);
});

test('folder entries: folders first, natural name order, sort keys', () => {
  const tree = buildFileTree(ROOT, ROWS, FAILURES);
  const q1 = findDir(tree, ['2024', 'q1'])!;
  // Natural order: invoice-2 before invoice-10.
  assert.deepEqual(folderEntries(q1, 'name').map(e => e.name), ['invoice-2.xlsx', 'invoice-10.xlsx']);
  assert.deepEqual(folderEntries(q1, 'name', true).map(e => e.name), ['invoice-10.xlsx', 'invoice-2.xlsx']);
  const y2024 = findDir(tree, ['2024'])!;
  assert.deepEqual(folderEntries(y2024, 'name').map(e => e.name), ['q1', 'notes.md', 'Scan.pdf']);
  assert.deepEqual(folderEntries(y2024, 'type').map(e => e.name), ['q1', 'notes.md', 'Scan.pdf']);
  // Failed files first when sorting by status.
  assert.deepEqual(folderEntries(y2024, 'status').map(e => e.name), ['q1', 'Scan.pdf', 'notes.md']);
});

test('search finds files below the folder by every word', () => {
  const tree = buildFileTree(ROOT, ROWS, FAILURES);
  assert.deepEqual(searchFiles(tree, 'invoice', 'name').map(f => f.name), ['invoice-2.xlsx', 'invoice-10.xlsx']);
  assert.deepEqual(searchFiles(tree, 'INVOICE 10', 'name').map(f => f.name), ['invoice-10.xlsx']);
  assert.deepEqual(searchFiles(findDir(tree, ['archive'])!, 'invoice', 'name'), []);
  assert.deepEqual(searchFiles(tree, '   ', 'name'), []);
});

test('type badges and families', () => {
  assert.equal(typeBadge('pdf'), 'PDF');
  assert.equal(typeBadge('markdown'), 'MARK');
  assert.equal(typeBadge(''), 'FILE');
  assert.equal(typeFamily('xlsx'), 'sheet');
  assert.equal(typeFamily('rs'), 'code');
  assert.equal(typeFamily('zzz'), 'other');
});

test('display paths keep the root spelling and separator', () => {
  const tree = buildFileTree(ROOT, ROWS, []);
  const invoice = findDir(tree, ['2024', 'q1'])!.files[0];
  assert.equal(displayPath(ROOT, invoice), ['C:\\Users\\Asha\\Contracts', '2024', 'q1', invoice.name].join('\\'));
  assert.equal(displayPath('/home/asha/Docs', { path: '/home/asha/Docs/a.md', dir: [], name: 'a.md' }), '/home/asha/Docs/a.md');
  // Outside the root: the path as the index reported it.
  assert.equal(displayPath(ROOT, { path: 'd:/elsewhere/a.txt', dir: [], name: 'a.txt' }), 'd:/elsewhere/a.txt');
});
