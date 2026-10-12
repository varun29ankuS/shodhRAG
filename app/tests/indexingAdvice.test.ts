/**
 * Indexing failures: one line on what to do and the action that helps.
 *   node --experimental-strip-types --test app/tests/indexingAdvice.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { adviceForFailures, explainFailure, groupFailures, indexingAdvice, relativeToFolder } from '../src/features/library/indexingAdvice.ts';

test('reasons map to the action that helps', () => {
  assert.equal(indexingAdvice('Search models are not installed; search needs first-run setup').action, 'install_search_models');
  assert.equal(indexingAdvice('The process cannot access the file because it is being used by another process. (os error 32)').action, 'reindex');
  assert.equal(indexingAdvice('Access is denied. (os error 5)').action, 'show_in_folder');
  assert.equal(indexingAdvice('The system cannot find the path specified. No such file or directory').action, 'show_in_folder');
  assert.equal(indexingAdvice('PDF is encrypted with a password').action, 'show_in_folder');
  assert.equal(indexingAdvice('Unsupported file type: .xyz').action, 'none');
  assert.equal(indexingAdvice('Unsupported file type: .xyz').actionLabel, null);
  assert.equal(indexingAdvice('File too large (900 MB exceeds the limit)').action, 'none');
  assert.equal(indexingAdvice('There is not enough space on the disk').action, 'reindex');
});

test('a scan waiting for the scan reader offers to install it, not search setup or conversion', () => {
  // The reason the indexer gives (shodh_rag::processing::ocr::ScanReaderMissing).
  const reason = 'Failed to process file: /docs/lease-scan.pdf: this file is a scan, and the scan reader (31 MB) that reads scans is not installed';
  const advice = indexingAdvice(reason);
  assert.equal(advice.action, 'install_search_models');
  assert.equal(advice.actionLabel, 'Install the scan reader');
  assert.match(advice.hint, /scan reader/);
  assert.equal(adviceForFailures([reason, reason, 'Access is denied.'])?.actionLabel, 'Install the scan reader');
  // A scan with no text at all is still a file Shodh cannot read.
  assert.equal(indexingAdvice('PDF contains no extractable text (scanned/image-based and OCR found no text)').action, 'none');
});

test('unknown and empty reasons suggest indexing again', () => {
  for (const reason of ['', '   ', null, undefined, 'something odd happened']) {
    const advice = indexingAdvice(reason);
    assert.equal(advice.action, 'reindex');
    assert.equal(advice.actionLabel, 'Index again');
    assert.ok(advice.hint.length > 0);
  }
});

test('a folder gets the advice of its most common failure', () => {
  assert.equal(adviceForFailures([]), null);
  const advice = adviceForFailures(['Unsupported file type: .bin', 'Access is denied.', 'Unsupported file type: .dat']);
  assert.equal(advice?.action, 'none');
  assert.match(advice?.hint ?? '', /cannot read text/);
});

// Reasons as the indexer writes them (crates/shodh-rag/src/processing/parser.rs
// tests pin the phrases; indexing.rs prefixes "Failed to process file: ").
const LOCKED = 'Failed to process file: C:\\Docs\\Bank\\statement.pdf: this PDF is protected with a password, so its text cannot be read';
const DAMAGED = 'Failed to process file: C:\\Docs\\Downloads\\partial.pdf: this PDF is damaged or incomplete (Invalid file trailer)';
const DOCX = 'Failed to process file: Failed to read DOCX as ZIP: C:\\Docs\\report.docx: invalid Zip archive: Could not find EOCD';
const SCAN = 'Failed to process file: C:\\Docs\\lease-scan.pdf: this file is a scan, and the scan reader (31 MB) that reads scans is not installed';

test('common indexer errors read as plain reasons, with the raw reason kept as detail', () => {
  const cases: [string, string, string][] = [
    [LOCKED, 'encrypted', 'Protected with a password'],
    [DAMAGED, 'damaged', 'Damaged or incomplete'],
    [DOCX, 'damaged', 'Damaged or incomplete'],
    ['Failed to process file: failed to open spreadsheet C:\\Docs\\a.xlsx: Zip error', 'damaged', 'Damaged or incomplete'],
    ['Unsupported file type: xyz', 'unsupported', 'This file type is not supported'],
    ['Failed to process file: memory allocation of 4294967296 bytes failed', 'too_large', 'Too large to index'],
    [SCAN, 'scan_reader_missing', 'A scan, and the scan reader is not installed'],
    ['Failed to process file: Lance error: IO: Es ist nicht genug Speicherplatz auf dem Datentr\u00e4ger verf\u00fcgbar. (os error 112)', 'disk_full', 'The disk is full'],
    ['Failed to process file: No space left on device (os error 28)', 'disk_full', 'The disk is full'],
    ['Failed to process file: PDF contains no extractable text (scanned/image-based and OCR found no text): C:\\a.pdf', 'no_text', 'No readable text in the file'],
  ];
  for (const [reason, kind, plain] of cases) {
    const explained = explainFailure(reason);
    assert.equal(explained.kind, kind, reason);
    assert.equal(explained.reason, plain, reason);
    assert.equal(explained.detail, reason, 'the raw reason stays available');
    assert.ok(!/Failed to process|os error|Zip/.test(explained.reason), 'no internal wording in the plain reason');
  }
  assert.deepEqual(explainFailure(null), { ...explainFailure(''), detail: '' });
});

test("a file's own name is not taken for the reason", () => {
  const file = 'C:\\Users\\me\\Password docs\\encrypted-notes.pdf';
  const reason = `Failed to process file: ${file}: this PDF is damaged or incomplete (Invalid file trailer)`;
  assert.equal(explainFailure(reason, file).kind, 'damaged');
  const scan = `Failed to process file: ${file}: this file is a scan, and the scan reader (31 MB) that reads scans is not installed`;
  assert.equal(explainFailure(scan, file).kind, 'scan_reader_missing');
  // Forward slashes in the reason and backslashes in the file name are the same path.
  assert.equal(explainFailure('Failed to process file: C:/Users/me/Password docs/encrypted-notes.pdf: No such file or directory (os error 2)', file).kind, 'missing');
});

test('hundreds of failures read as a few reasons with the exact files under each', () => {
  const root = 'C:\\Docs';
  const failures = [
    ...Array.from({ length: 240 }, (_, i) => ({ file: `C:\\Docs\\scans\\page-${String(i).padStart(3, '0')}.pdf`, reason: SCAN })),
    { file: 'C:\\Docs\\Bank\\statement.pdf', reason: LOCKED },
    { file: 'C:\\Docs\\2023\\statement.pdf', reason: LOCKED },
    { file: 'C:\\Docs\\Downloads\\partial.pdf', reason: DAMAGED },
  ];
  const groups = groupFailures(failures, root);
  assert.deepEqual(groups.map(g => [g.kind, g.files.length]), [['scan_reader_missing', 240], ['encrypted', 2], ['damaged', 1]]);
  assert.deepEqual(groups[1].files.map(f => f.relative), ['2023/statement.pdf', 'Bank/statement.pdf'], 'same names told apart by folder');
  assert.equal(groups[1].files[0].detail, LOCKED);
  assert.equal(groups[0].advice.actionLabel, 'Install the scan reader');
});

test('paths are shown below their folder', () => {
  assert.equal(relativeToFolder('C:\\Docs\\a\\b.pdf', 'c:\\docs\\'), 'a/b.pdf');
  assert.equal(relativeToFolder('/home/me/docs/a/b.pdf', '/home/me/docs'), 'a/b.pdf');
  assert.equal(relativeToFolder('/home/me/docs2/b.pdf', '/home/me/docs'), '/home/me/docs2/b.pdf');
  assert.equal(relativeToFolder('C:\\Elsewhere\\b.pdf', 'C:\\Docs'), 'C:/Elsewhere/b.pdf');
  assert.equal(relativeToFolder('C:\\Docs\\b.pdf', null), 'C:/Docs/b.pdf');
});