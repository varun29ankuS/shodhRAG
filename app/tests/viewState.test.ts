/**
 * Remembered viewer state tests (serialization, corrupt or missing storage).
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/viewState.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  cleanPdfTitle,
  parsePdfMeta,
  parsePdfViewState,
  parseStoredEntries,
  parseTextViewState,
  PersistentStore,
  readNumberPreference,
  serializeEntries,
  writeNumberPreference,
} from '../src/features/ask/viewer/viewState.ts';
import type { KeyValueStorage, PdfViewState } from '../src/features/ask/viewer/viewState.ts';

function memoryStorage(initial: Record<string, string> = {}) {
  const data = new Map(Object.entries(initial));
  const storage: KeyValueStorage = {
    getItem: key => data.get(key) ?? null,
    setItem: (key, value) => {
      data.set(key, value);
    },
  };
  return { storage, data };
}

const KEY = 'test.viewState';

function pdfStore(provider: () => KeyValueStorage | null, maxEntries = 3) {
  return new PersistentStore<PdfViewState>({ storageKey: KEY, storage: provider, validate: parsePdfViewState, maxEntries });
}

const fit = (page: number, offset = 0): PdfViewState => ({ page, offset, zoom: { mode: 'fit' } });

test('round-trips through storage into a fresh store (next session)', () => {
  const { storage } = memoryStorage();
  const first = pdfStore(() => storage);
  first.set('c:/papers/a.pdf', { page: 7, offset: 0.25, zoom: { mode: 'manual', scale: 1.44 } });
  const second = pdfStore(() => storage);
  assert.deepEqual(second.get('c:/papers/a.pdf'), { page: 7, offset: 0.25, zoom: { mode: 'manual', scale: 1.44 } });
  assert.equal(second.get('c:/papers/missing.pdf'), null);
});

test('keeps only the most recently written entries', () => {
  const { storage } = memoryStorage();
  const store = pdfStore(() => storage, 2);
  store.set('a', fit(1));
  store.set('b', fit(2));
  store.set('a', fit(3)); // a becomes most recent
  store.set('c', fit(4));
  const reloaded = pdfStore(() => storage, 2);
  assert.equal(reloaded.get('b'), null);
  assert.deepEqual(reloaded.get('a'), fit(3));
  assert.deepEqual(reloaded.get('c'), fit(4));
});

test('corrupt JSON is ignored and overwritten on the next write', () => {
  const { storage, data } = memoryStorage({ [KEY]: '{not json' });
  const store = pdfStore(() => storage);
  assert.equal(store.get('a'), null);
  store.set('a', fit(2));
  assert.deepEqual(parseStoredEntries(data.get(KEY)!, parsePdfViewState), [['a', fit(2)]]);
});

test('wrong version or shape yields nothing; invalid entries are skipped individually', () => {
  assert.deepEqual(parseStoredEntries(JSON.stringify({ v: 99, entries: [['a', fit(1)]] }), parsePdfViewState), []);
  assert.deepEqual(parseStoredEntries(JSON.stringify([1, 2]), parsePdfViewState), []);
  assert.deepEqual(parseStoredEntries('null', parsePdfViewState), []);
  const mixed = JSON.stringify({
    v: 1,
    entries: [
      ['good', fit(2, 0.5)],
      ['bad-page', { page: 0, offset: 0, zoom: { mode: 'fit' } }],
      ['bad-zoom', { page: 1, offset: 0, zoom: { mode: 'huge' } }],
      ['', fit(1)],
      'garbage',
      ['nan', { page: 1, offset: 'x', zoom: { mode: 'fit' } }],
    ],
  });
  assert.deepEqual(parseStoredEntries(mixed, parsePdfViewState), [['good', fit(2, 0.5)]]);
});

test('storage that throws on access still gives a working session store', () => {
  const throwing: KeyValueStorage = {
    getItem: () => {
      throw new Error('SecurityError');
    },
    setItem: () => {
      throw new Error('QuotaExceededError');
    },
  };
  const store = pdfStore(() => throwing);
  store.set('a', fit(5));
  assert.deepEqual(store.get('a'), fit(5));
  const noProvider = pdfStore(() => {
    throw new Error('localStorage is not available');
  });
  noProvider.set('b', fit(6));
  assert.deepEqual(noProvider.get('b'), fit(6));
  const nullProvider = pdfStore(() => null);
  nullProvider.set('c', fit(1));
  assert.deepEqual(nullProvider.get('c'), fit(1));
});

test('serialization format is stable', () => {
  assert.equal(serializeEntries([['k', { ratio: 0.5 }]]), '{"v":1,"entries":[["k",{"ratio":0.5}]]}');
});

test('PDF view state values are clamped to sane ranges', () => {
  assert.deepEqual(parsePdfViewState({ page: 3, offset: 4, zoom: { mode: 'manual', scale: 50 } }), {
    page: 3,
    offset: 1,
    zoom: { mode: 'manual', scale: 5 },
  });
  assert.deepEqual(parsePdfViewState({ page: 3, offset: -1, zoom: { mode: 'manual', scale: 0.01 } }), {
    page: 3,
    offset: 0,
    zoom: { mode: 'manual', scale: 0.25 },
  });
  assert.equal(parsePdfViewState({ page: 2.5, offset: 0, zoom: { mode: 'fit' } }), null);
  assert.equal(parsePdfViewState({ page: 2, offset: 0, zoom: { mode: 'manual', scale: Infinity } }), null);
  assert.equal(parsePdfViewState(null), null);
});

test('text view state', () => {
  assert.deepEqual(parseTextViewState({ ratio: 0.3 }), { ratio: 0.3 });
  assert.deepEqual(parseTextViewState({ ratio: 3 }), { ratio: 1 });
  assert.equal(parseTextViewState({ ratio: NaN }), null);
  assert.equal(parseTextViewState('0.3'), null);
});

test('PDF metadata validation', () => {
  const meta = { size: 1200, modified: 1790000000000, title: 'Attention Is All You Need', pages: 15, width: 612, height: 792 };
  assert.deepEqual(parsePdfMeta(meta), meta);
  // Remembered before modification times were known: no time, still valid.
  const { modified: _modified, ...older } = meta;
  assert.deepEqual(parsePdfMeta(older), { ...meta, modified: null });
  assert.deepEqual(parsePdfMeta({ ...meta, modified: 'yesterday' }), { ...meta, modified: null });
  assert.deepEqual(parsePdfMeta({ ...meta, title: null }), { ...meta, title: null });
  assert.deepEqual(parsePdfMeta({ ...meta, title: 'untitled' }), { ...meta, title: null });
  assert.equal(parsePdfMeta({ ...meta, pages: 0 }), null);
  assert.equal(parsePdfMeta({ ...meta, width: -1 }), null);
  assert.equal(parsePdfMeta({ ...meta, title: 4 }), null);
  assert.equal(parsePdfMeta({ ...meta, size: undefined }), null);
});

test('document info titles: real titles kept, placeholders and file names rejected', () => {
  assert.equal(cleanPdfTitle('  Attention   Is All\nYou Need '), 'Attention Is All You Need');
  assert.equal(cleanPdfTitle('Microsoft Word - Draft Report'), 'Draft Report');
  assert.equal(cleanPdfTitle('untitled'), null);
  assert.equal(cleanPdfTitle('Slide 1'), null);
  assert.equal(cleanPdfTitle('main.dvi'), null);
  assert.equal(cleanPdfTitle('paper_v3_final.pdf'), null);
  assert.equal(cleanPdfTitle('C:\\Users\\me\\thesis.docx'), null);
  assert.equal(cleanPdfTitle('ab'), null);
  assert.equal(cleanPdfTitle('---'), null);
  assert.equal(cleanPdfTitle('A\u0000B\u0000C'), 'A B C');
  assert.equal(cleanPdfTitle(undefined), null);
  assert.equal(cleanPdfTitle('BERT: Pre-training of Deep Bidirectional Transformers'), 'BERT: Pre-training of Deep Bidirectional Transformers');
  assert.equal(cleanPdfTitle('x'.repeat(400))!.length, 300);
});

test('number preferences are clamped and survive bad storage', () => {
  const { storage, data } = memoryStorage();
  assert.equal(readNumberPreference(() => storage, 'w', 360, 240, 600), 360);
  writeNumberPreference(() => storage, 'w', 412.6);
  assert.equal(data.get('w'), '413');
  assert.equal(readNumberPreference(() => storage, 'w', 360, 240, 600), 413);
  data.set('w', '9999');
  assert.equal(readNumberPreference(() => storage, 'w', 360, 240, 600), 600);
  data.set('w', 'wide');
  assert.equal(readNumberPreference(() => storage, 'w', 360, 240, 600), 360);
  data.set('w', '');
  assert.equal(readNumberPreference(() => storage, 'w', 360, 240, 600), 360);
  const throwing = () => {
    throw new Error('denied');
  };
  assert.equal(readNumberPreference(throwing, 'w', 360, 240, 600), 360);
  writeNumberPreference(throwing, 'w', 300);
  writeNumberPreference(() => storage, 'w', NaN);
  assert.equal(data.get('w'), '');
});
