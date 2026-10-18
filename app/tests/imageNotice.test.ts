/**
 * An image pasted into the composer is handled during the paste (a text paste is
 * left alone), and every image notice is a notice, never an assistant answer that
 * would be replayed to the model. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/imageNotice.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { failureNotice, imageNotice, pasteDecision, readImageText } from '../src/features/ask/imageNotice.ts';

test('text on the clipboard is a text paste, also with a bitmap beside it', () => {
  assert.deepEqual(pasteDecision('Revenue 2024', []), { kind: 'text' });
  assert.deepEqual(pasteDecision('Revenue 2024', ['image/png']), { kind: 'text' });
});

test('an image file without text is pasted as an image; nothing exposed asks the system clipboard', () => {
  assert.deepEqual(pasteDecision('', ['image/png']), { kind: 'file', index: 0 });
  assert.deepEqual(pasteDecision('  ', ['application/pdf', 'image/jpeg']), { kind: 'file', index: 1 });
  assert.deepEqual(pasteDecision('', []), { kind: 'clipboard' });
  assert.deepEqual(pasteDecision('', ['application/pdf']), { kind: 'text' });
});

test('the OCR result is read from its camelCase fields; anything else reads as empty', () => {
  assert.deepEqual(
    readImageText({ extractedText: 'Total 42', wordCount: 2, confidence: 0.9, imageData: 'data:image/png;base64,AA' }),
    { text: 'Total 42', wordCount: 2, confidence: 0.9, imageData: 'data:image/png;base64,AA' },
  );
  assert.deepEqual(readImageText(null), { text: '', wordCount: 0, confidence: 0, imageData: '' });
});

test('image and failure notices are system notices, with the image when there is one', () => {
  const found = imageNotice('n1', 't', 'pasted', { text: 'Total 42', wordCount: 2, confidence: 0.9, imageData: 'data:x' });
  assert.equal(found.role, 'system');
  assert.equal(found.image, 'data:x');
  assert.match(found.content, /Image pasted.*2 words, 90% confidence/s);
  // The text goes with the next question as the user's words, not in the notice.
  assert.match(found.content, /is attached to your next question\.$/);
  assert.doesNotMatch(found.content, /Total 42/);
  const empty = imageNotice('n2', 't', 'dropped', { text: '', wordCount: 0, confidence: 0, imageData: '' });
  assert.equal(empty.role, 'system');
  assert.equal('image' in empty, false);
  assert.match(empty.content, /Image dropped.*No text was found/);
  assert.equal(failureNotice('n3', 't', 'It failed', 'disk full').role, 'system');
});

test('no view appends an assistant-role notice and the global Ctrl+V listener is gone', () => {
  const app = readFileSync(new URL('../src/App-SplitView.tsx', import.meta.url), 'utf8');
  assert.doesNotMatch(app, /role: 'assistant'/);
  assert.doesNotMatch(app, /addEventListener\('keydown', handleGlobalPaste/);
  const composer = readFileSync(new URL('../src/features/agent/AgentComposer.tsx', import.meta.url), 'utf8');
  assert.match(composer, /onPaste=\{handlePaste\}/);
});
