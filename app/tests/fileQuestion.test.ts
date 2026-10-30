/**
 * "Ask about this file": the question names the document by its title only; the file's
 * path goes with the question as its search limit, is kept with the user message for a
 * retry, and never enters the text shown, titled, exported or sent to a connector. Run
 * with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/fileQuestion.test.ts
 */
import { test } from 'node:test';
import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
import {
  ABOUT_FILES_METADATA_KEY,
  FileRefStore,
  cleanFiles,
  fileNameOfPath,
  fileQuestionDraft,
  limitedToFiles,
  metadataWithAboutFiles,
  readAboutFiles,
} from '../src/features/ask/fileQuestion.ts';

const PATH = 'c:/users/someone/documents/research papers/2605.04308 - memory as a markov matrix.pdf';
const NAME = '2605.04308 - memory as a markov matrix.pdf';

test('the draft names the file by its title and never carries its path', () => {
  const draft = fileQuestionDraft(NAME);
  assert.equal(draft, `About ${NAME}: `);
  assert.ok(!draft.includes('c:/users'));
  assert.ok(!draft.includes('someone'));
});

test('a title with line breaks or padding stays one line in the draft', () => {
  assert.equal(fileQuestionDraft('  Attention\n is all\tyou need  '), 'About Attention is all you need: ');
});

test('the file name of a path, with either separator', () => {
  assert.equal(fileNameOfPath(PATH), NAME);
  assert.equal(fileNameOfPath('C:\\papers\\a.pdf'), 'a.pdf');
  assert.equal(fileNameOfPath('a.pdf'), 'a.pdf');
});

test('the file goes with the question as its search limit', () => {
  const options = { spaceId: null, spaceName: null, searchAll: false };
  assert.deepEqual(limitedToFiles(options, [PATH]), { ...options, sourceFiles: [PATH] });
  // No file: the options are the caller's, unchanged.
  assert.equal(limitedToFiles(options, []), options);
  assert.equal(limitedToFiles(options, undefined), options);
  assert.equal(limitedToFiles(options, ['  ']), options);
});

test('the user message keeps the files in its metadata, beside every other key', () => {
  const metadata = metadataWithAboutFiles({ focusThreads: [] }, [PATH, ` ${PATH} `, '']);
  assert.deepEqual(metadata, { focusThreads: [], [ABOUT_FILES_METADATA_KEY]: [PATH] });
  assert.deepEqual(readAboutFiles(metadata), [PATH]);
  // Removed when there are none; nothing left means no metadata.
  assert.deepEqual(metadataWithAboutFiles(metadata, []), { focusThreads: [] });
  assert.equal(metadataWithAboutFiles({ [ABOUT_FILES_METADATA_KEY]: [PATH] }, []), undefined);
});

test('malformed stored files are ignored', () => {
  assert.deepEqual(readAboutFiles(null), []);
  assert.deepEqual(readAboutFiles({ [ABOUT_FILES_METADATA_KEY]: 'a.pdf' }), []);
  assert.deepEqual(readAboutFiles({ [ABOUT_FILES_METADATA_KEY]: [1, '', 'a.pdf', 'a.pdf'] }), ['a.pdf']);
});

test('at most as many files as the backend accepts', () => {
  const many = Array.from({ length: 60 }, (_, i) => `f${i}.pdf`);
  assert.equal(cleanFiles(many).length, 50);
});

test('the store keeps one file per conversation until the question is sent', () => {
  const store = new FileRefStore();
  let changes = 0;
  const unsubscribe = store.subscribe(() => {
    changes += 1;
  });
  store.set('c1', { path: PATH, name: NAME });
  const sent = store.get('c1');
  assert.deepEqual(sent, { path: PATH, name: NAME });
  assert.equal(store.get('c2'), null);
  // Setting the same file again is no change, and keeps the same object.
  store.set('c1', { path: PATH, name: NAME });
  assert.equal(store.get('c1'), sent);
  assert.equal(changes, 1);
  store.consume('c1', sent);
  assert.equal(store.get('c1'), null);
  assert.equal(changes, 2);
  unsubscribe();
});

test('a file chosen while a question was being sent stays for the next one', () => {
  const store = new FileRefStore();
  store.set('c1', { path: 'a.pdf', name: 'A' });
  const sent = store.get('c1');
  store.set('c1', { path: 'b.pdf', name: 'B' });
  store.consume('c1', sent);
  assert.deepEqual(store.get('c1'), { path: 'b.pdf', name: 'B' });
});

test('a blank path is no file; a blank name falls back to the file name', () => {
  const store = new FileRefStore();
  store.set('c1', { path: '  ', name: 'A' });
  assert.equal(store.get('c1'), null);
  store.set('c1', { path: PATH, name: ' ' });
  assert.deepEqual(store.get('c1'), { path: PATH, name: NAME });
  store.set('c1', null);
  assert.equal(store.get('c1'), null);
});

test('every "ask about" entry point names the document and sends the path beside the text', () => {
  const read = (file: string) => readFileSync(new URL(file, import.meta.url), 'utf8');
  const app = read('../src/App-SplitView.tsx');
  assert.match(app, /text: fileQuestionDraft\(file\.name\)/);
  assert.match(app, /file: \{ path: file\.path, name: file\.name \}/);
  assert.doesNotMatch(app, /\(\$\{file\.path\}\)/);
  for (const file of ['../src/features/research/PaperPage.tsx', '../src/features/research/PaperFigures.tsx']) {
    const source = read(file);
    // No path interpolated into a composer text: `(${p.filePath})`, `(${parts.filePath})`.
    assert.doesNotMatch(source, /insertIntoComposer\(`[^`]*\$\{[^}]*filePath[^}]*\}/, file);
  }
  // Ask and the dock send the chosen file as the question's search limit.
  for (const file of ['../src/features/ask/AskView.tsx', '../src/components/shell/ConversationDock.tsx']) {
    assert.match(read(file), /limitedToFiles\(/, file);
  }
});
