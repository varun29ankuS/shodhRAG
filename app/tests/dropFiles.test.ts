/**
 * Dropped files: images are read, old .doc/.ppt files are named with what to do,
 * and everything else (PowerPoint .pptx and folders included) goes to the Library.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/dropFiles.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  DROP_DOCUMENT_EXTENSIONS,
  extensionOf,
  isIndexableDocument,
  legacyOfficeNotice,
  sortDroppedPaths,
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

test('every document a drop accepts is one the indexer reads; .doc and .ppt are not', () => {
  const supported = indexerExtensions();
  assert.ok(supported.length > 20);
  for (const ext of DROP_DOCUMENT_EXTENSIONS) assert.ok(supported.includes(ext), ext);
  assert.ok(!supported.includes('doc') && !supported.includes('ppt'));
  assert.ok(isIndexableDocument('deck.pptx'));
});

test('a mixed drop keeps PowerPoint files and folders, reads images, names old Office files', () => {
  const sorted = sortDroppedPaths(['a.pdf', 'b.pptx', 'C:\\Research', 'shot.PNG', 'old.doc', 'older.ppt', 'data.xlsx']);
  assert.deepEqual(sorted.images, ['shot.PNG']);
  assert.deepEqual(sorted.legacy, ['old.doc', 'older.ppt']);
  assert.deepEqual(sorted.library, ['a.pdf', 'b.pptx', 'C:\\Research', 'data.xlsx']);
});

test('the old-format notice says what to save the files as', () => {
  assert.equal(legacyOfficeNotice(['C:\\a\\old.doc']), 'old.doc is in an older Office format Shodh cannot read. Save it as .docx and drop it again.');
  assert.match(legacyOfficeNotice(['x.doc', 'y.ppt']), /x\.doc, y\.ppt are .* Save them as \.docx or \.pptx and drop them again\./);
});
