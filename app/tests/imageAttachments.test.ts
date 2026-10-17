/**
 * Text read from an image waits as a removable chip and goes out with the next
 * question, fenced inside the user's own message. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/imageAttachments.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { AttachmentStore, attachmentLabel, composeQuestion } from '../src/features/ask/imageAttachments.ts';
import type { ImageTextAttachment } from '../src/features/ask/imageAttachments.ts';

const pasted = (id: string, text: string): ImageTextAttachment => ({ id, origin: 'pasted', text });

test('the chip names where the text came from and how long it is', () => {
  assert.equal(attachmentLabel(pasted('a', 'x'.repeat(1204))), 'Text from pasted image · 1,204 characters');
  assert.equal(attachmentLabel({ id: 'b', origin: 'added', text: 'x' }), 'Text from added image · 1 character');
});

test('the sent question carries each attachment fenced as text the user provided', () => {
  const sent = composeQuestion('  What is the total? ', [pasted('a', 'Revenue 42\nCost 12'), { id: 'b', origin: 'added', text: 'Q3' }]);
  assert.equal(
    sent,
    'What is the total?\n\nText from an image I pasted:\n```\nRevenue 42\nCost 12\n```\n\nText from an image I added:\n```\nQ3\n```',
  );
  assert.equal(composeQuestion('Hi', []), 'Hi');
  assert.equal(composeQuestion('Hi', [pasted('a', '   ')]), 'Hi');
});

test('text with backticks cannot close the fence around it', () => {
  const sent = composeQuestion('Explain', [pasted('a', 'run ```rm``` now')]);
  assert.ok(sent.includes('\n````\nrun ```rm``` now\n````'));
});

test('attachments are per conversation, removable, and taken once by the send', () => {
  const store = new AttachmentStore();
  let changes = 0;
  store.subscribe(() => { changes += 1; });
  store.add('c1', pasted('a', 'one'));
  store.add('c1', pasted('b', 'two'));
  store.add('c1', pasted('empty', '  '));
  store.add('c2', pasted('c', 'other chat'));
  assert.deepEqual(store.get('c1').map(a => a.id), ['a', 'b']);
  const before = store.get('c1');
  assert.equal(store.get('c1'), before, 'same array until it changes');
  store.remove('c1', 'a');
  store.remove('c1', 'missing');
  assert.deepEqual(store.get('c1').map(a => a.id), ['b']);
  assert.deepEqual(store.take('c1').map(a => a.id), ['b']);
  assert.deepEqual(store.get('c1'), []);
  assert.deepEqual(store.take('c1'), []);
  assert.deepEqual(store.get('c2').map(a => a.id), ['c']);
  assert.equal(changes, 5);
});

test('Ask and the dock send the attachments with the question; images feed them, never an assistant turn', () => {
  for (const file of ['../src/features/ask/AskView.tsx', '../src/components/shell/ConversationDock.tsx']) {
    const source = readFileSync(new URL(file, import.meta.url), 'utf8');
    assert.match(source, /composeQuestion\(text, composerAttachments\.take\(session\.activeConversationId\)\)/, file);
    assert.match(source, /<AttachmentChips conversationId=\{session\.activeConversationId\}/, file);
  }
  const app = readFileSync(new URL('../src/App-SplitView.tsx', import.meta.url), 'utf8');
  for (const origin of ['pasted', 'added', 'dropped']) assert.ok(app.includes(`showImageText('${origin}', result)`), origin);
  assert.match(app, /composerAttachments\.add\(activeConversationIdRef\.current/);
  const chips = readFileSync(new URL('../src/features/ask/AttachmentChips.tsx', import.meta.url), 'utf8');
  assert.match(chips, /onClick=\{\(\) => composerAttachments\.remove\(conversationId, attachment\.id\)\}/);
  assert.match(chips, /aria-label=\{`Remove: \$\{label\}`\}/);
});
