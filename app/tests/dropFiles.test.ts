/**
 * Dropped files: images are read, and everything else (Word .doc and .docx,
 * PowerPoint .ppt and .pptx, and folders included) goes to the Library.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/dropFiles.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  DROP_DOCUMENT_EXTENSIONS,
  INDEXABLE_EXTENSIONS,
  extensionOf,
  isIndexableDocument,
  isIndexableFile,
  sortDroppedPaths,
  unsupportedFilesNotice,
} from '../src/features/library/dropFiles.ts';

/** `SUPPORTED_FILE_EXTENSIONS` of the indexer. */
function indexerExtensions(): string[] {
  const rust = readFileSync(new URL('../../crates/shodh-rag/src/indexing.rs', import.meta.url), 'utf8');
  const start = rust.indexOf('pub const SUPPORTED_FILE_EXTENSIONS');
  const body = rust.slice(rust.indexOf('[', start), rust.indexOf('];', start));
  return [...body.matchAll(/"([a-z0-9]+)"/g)].map(m => m[1]);
}

test('extensions are read case-insensitively from paths and names', () => {
  assert.equal(extensionOf('C:\\Papers\\Talk.PPTX'), 'pptx');
  assert.equal(extensionOf('C:\\Papers.v2\\notes'), '');
  assert.equal(extensionOf('/home/me/notes.md'), 'md');
  assert.equal(extensionOf('/home/me/folder'), '');
  assert.equal(extensionOf('/home/me/.hidden'), '');
});

test('every document a drop accepts is one the indexer reads, old .doc and .ppt included', () => {
  const supported = indexerExtensions();
  assert.ok(supported.length > 20);
  for (const ext of DROP_DOCUMENT_EXTENSIONS) assert.ok(supported.includes(ext), ext);
  for (const ext of ['doc', 'ppt', 'odt', 'odp', 'rtf', 'epub', 'eml', 'ipynb', 'log']) assert.ok(supported.includes(ext), ext);
  // Environment files may hold secrets; they are never indexed.
  assert.ok(!supported.includes('env'));
  // The app's list of indexable files is the indexer's, exactly.
  assert.deepEqual([...INDEXABLE_EXTENSIONS].sort(), [...supported].sort());
  assert.ok(isIndexableFile('C:\\src\\main.rs') && !isIndexableFile('setup.exe') && !isIndexableFile('archive.zip'));
  assert.equal(
    unsupportedFilesNotice(['C:\\a\\setup.exe']),
    'setup.exe is not a type Shodh can index (PDF, Word, Excel, PowerPoint, OpenDocument, RTF, EPUB, email, text, Markdown, code or images).',
  );
  assert.ok(isIndexableDocument('deck.pptx') && isIndexableDocument('old.doc') && isIndexableDocument('older.ppt'));
});

test('a mixed drop keeps documents (old Office ones too) and folders, and reads images', () => {
  const sorted = sortDroppedPaths(['a.pdf', 'b.pptx', 'C:\\Research', 'shot.PNG', 'old.doc', 'older.ppt', 'data.xlsx']);
  assert.deepEqual(sorted.images, ['shot.PNG']);
  assert.deepEqual(sorted.library, ['a.pdf', 'b.pptx', 'C:\\Research', 'old.doc', 'older.ppt', 'data.xlsx']);
});
