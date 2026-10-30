/**
 * "Ask about this file": the question names the document by its title only; the file's
 * path goes with the question as its search limit, is kept with the user message for a
 * retry, and never enters the text shown, titled, exported or sent to a connector. Run
 * with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/fileQuestion.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  ABOUT_FILES_METADATA_KEY,
  FileRefStore,
  cleanFiles,
  figureQuestion,
  fileNameOfPath,
  fileQuestionDraft,
  limitedToFiles,
  metadataWithAboutFiles,
  missingFileNotice,
  missingFiles,
  paperQuestion,
  readAboutFiles,
  replaceFilePrefix,
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
  assert.deepEqual(limitedToFiles(options, [PATH]), { ...options, sourceIds: [], sourceFiles: [PATH] });
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

test('a question about a file searches only that file, whatever sources are included', () => {
  // The backend searches the union of sources and files: included sources would widen it.
  const options = { spaceId: 'a', spaceName: 'Folder a', sourceIds: ['a', 'c'], searchAll: false };
  const limited = limitedToFiles(options, [PATH]);
  assert.deepEqual(limited.sourceIds, []);
  assert.deepEqual(limited.sourceFiles, [PATH]);
  // Without a file the included sources stay.
  assert.deepEqual(limitedToFiles(options, []).sourceIds, ['a', 'c']);
});

test('a paper or figure question names the paper; its PDF goes beside the text', () => {
  const paper = paperQuestion('Memory as a  Markov matrix', PATH);
  assert.equal(paper.text, 'About "Memory as a Markov matrix": ');
  assert.deepEqual(paper.file, { path: PATH, name: 'Memory as a Markov matrix' });
  assert.ok(!paper.text.includes('someone'));
  // A paper not in the library has no file to limit to.
  assert.deepEqual(paperQuestion('Elsewhere', null), { text: 'About "Elsewhere": ' });
  const figure = figureQuestion('Figure 3', 'Memory as a Markov matrix', PATH);
  assert.equal(figure.text, 'Explain Figure 3 of "Memory as a Markov matrix": ');
  assert.equal(figure.file?.path, PATH);
  assert.ok(!figure.text.includes('someone'));
});

test('asking about another file replaces the question about the previous one', () => {
  const previous = { path: 'a.pdf', name: 'A', prefix: 'About "A": ' };
  // The user went on typing after the prefix: their words stay.
  assert.equal(replaceFilePrefix('About "A": what is eq 3?', previous, 'About "B": '), 'About "B": what is eq 3?');
  // The draft no longer starts with the old question, or there was no file: append instead.
  assert.equal(replaceFilePrefix('My own question', previous, 'About "B": '), null);
  assert.equal(replaceFilePrefix('About "A": ', null, 'About "B": '), null);
  assert.equal(replaceFilePrefix('About "A": ', { path: 'a.pdf', name: 'A' }, 'About "B": '), null);
});

test('a file chosen before any conversation is open goes to the first one shown', () => {
  const store = new FileRefStore();
  store.set(null, { path: PATH, name: NAME });
  store.adoptUnassigned('c1');
  assert.deepEqual(store.get('c1'), { path: PATH, name: NAME });
  assert.equal(store.get(null), null);
  // A conversation that already has a file keeps it.
  store.set(null, { path: 'b.pdf', name: 'B' });
  store.adoptUnassigned('c1');
  assert.equal(store.get('c1')?.path, PATH);
  assert.equal(store.get(null), null);
});

test('the store keeps the text a file was asked about with', () => {
  const store = new FileRefStore();
  store.set('c1', { path: PATH, name: NAME, prefix: `About ${NAME}: ` });
  assert.equal(store.get('c1')?.prefix, `About ${NAME}: `);
  // The same file with another question text is a change.
  const first = store.get('c1');
  store.set('c1', { path: PATH, name: NAME, prefix: 'Explain Figure 1 of "X": ' });
  assert.notEqual(store.get('c1'), first);
});

test('files no longer indexed are found before the question is sent', async () => {
  const indexed = new Set(['kept.pdf']);
  const check = async (path: string) => indexed.has(path);
  assert.deepEqual(await missingFiles(['kept.pdf', 'gone.pdf'], check), ['gone.pdf']);
  assert.deepEqual(await missingFiles(['kept.pdf'], check), []);
  // A check that fails counts the file as present: search then says what it could not find.
  assert.deepEqual(await missingFiles(['x.pdf'], async () => { throw new Error('index busy'); }), []);
});

test('a missing file is said by name, never by path', () => {
  const notice = missingFileNotice([PATH], false);
  assert.equal(notice.title, 'This file is no longer in your library');
  assert.ok(notice.description.startsWith(`${NAME} was removed or renamed.`), notice.description);
  assert.ok(!notice.description.includes('someone'));
  const retry = missingFileNotice(['a/x.pdf', 'b/y.pdf'], true);
  assert.equal(retry.title, 'These files are no longer in your library');
  assert.ok(retry.description.startsWith('x.pdf, y.pdf were removed or renamed, so this question cannot be asked about them again.'), retry.description);
});
