/**
 * Text read from an image waits as a removable chip and goes out with the next
 * question beside the typed words (never in them), and a replay reads it after the
 * question, fenced as the user's. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/imageAttachments.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  AttachmentStore,
  attachedContextOf,
  attachmentLabel,
  composeQuestion,
  metadataWithAttachedContext,
  readAttachedContext,
} from '../src/features/ask/imageAttachments.ts';
import type { ImageTextAttachment } from '../src/features/ask/imageAttachments.ts';
import { replayedContent } from '../src/features/ask/chatHistory.ts';

const pasted = (id: string, text: string): ImageTextAttachment => ({ id, origin: 'pasted', text });

test('the chip names where the text came from and how long it is', () => {
  assert.equal(attachmentLabel(pasted('a', 'x'.repeat(1204))), 'Text from pasted image · 1,204 characters');
  assert.equal(attachmentLabel({ origin: 'added', text: 'x' }), 'Text from added image · 1 character');
});

test('what goes out with a question: the attachments with text, without chip ids', () => {
  assert.deepEqual(attachedContextOf([pasted('a', 'Revenue 42'), pasted('b', '  '), { id: 'c', origin: 'dropped', text: 'Q3' }]), [
    { origin: 'pasted', text: 'Revenue 42' },
    { origin: 'dropped', text: 'Q3' },
  ]);
});

test('a replay reads the attached text after the question, fenced as the user provided it', () => {
  const composed = composeQuestion('  What is the total? ', [{ origin: 'pasted', text: 'Revenue 42\nCost 12' }, { origin: 'added', text: 'Q3' }]);
  assert.equal(
    composed,
    'What is the total?\n\nText from an image I pasted:\n```\nRevenue 42\nCost 12\n```\n\nText from an image I added:\n```\nQ3\n```',
  );
  assert.equal(composeQuestion('Hi', []), 'Hi');
  assert.equal(composeQuestion('Hi', [{ origin: 'pasted', text: '   ' }]), 'Hi');
  // Same as the backend's `with_attached_context`: backticks cannot close the fence.
  assert.ok(composeQuestion('Explain', [{ origin: 'added', text: 'run ```rm``` now' }]).includes('\n````\nrun ```rm``` now\n````'));

  const user = { role: 'user' as const, content: 'What is the total?', attachedContext: [{ origin: 'pasted' as const, text: 'Revenue 42' }] };
  assert.equal(replayedContent(user), 'What is the total?\n\nText from an image I pasted:\n```\nRevenue 42\n```');
  assert.equal(replayedContent({ role: 'user', content: 'Plain' }), 'Plain');
});

test('a user message keeps its attached text in metadata, beside other keys', () => {
  const attached = [{ origin: 'pasted' as const, text: 'Revenue 42' }];
  const metadata = metadataWithAttachedContext({ focusThreads: [1] }, attached);
  assert.deepEqual(metadata, { focusThreads: [1], imageText: attached });
  assert.deepEqual(readAttachedContext(metadata), attached);
  assert.deepEqual(metadataWithAttachedContext(metadata, []), { focusThreads: [1] });
  assert.equal(metadataWithAttachedContext({ imageText: attached }, []), undefined);
  assert.deepEqual(readAttachedContext({ imageText: [{ origin: 'typed', text: 'x' }, { origin: 'added', text: '' }, 'junk'] }), []);
  assert.deepEqual(readAttachedContext(null), []);
});

test('attachments are per conversation and removable; a send removes only what it sent', () => {
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

  // The chip's remove button.
  store.remove('c1', 'a');
  store.remove('c1', 'missing');
  assert.deepEqual(store.get('c1').map(a => a.id), ['b']);

  // Read before sending; one attached while the question was being sent stays.
  const sent = store.get('c1');
  store.add('c1', pasted('late', 'three'));
  store.consume('c1', sent);
  assert.deepEqual(store.get('c1').map(a => a.id), ['late']);
  store.consume('c1', sent);
  assert.deepEqual(store.get('c2').map(a => a.id), ['c']);
  assert.equal(changes, 6);
});
