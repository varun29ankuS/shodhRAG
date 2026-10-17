/**
 * Data file notice tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/storeNotices.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { noticeDescription, parseStoreNotice, unseenNotices } from '../src/lib/storeNotices.ts';

const restored = {
  id: 'settings:restored:C:/data/app_settings.json',
  store: 'settings',
  kind: 'restored',
  title: 'Settings restored from the backup',
  message: 'The settings file could not be read (the file is empty).',
  path: 'C:/data/app_settings.json',
};

test('a notice from the backend is read, and a malformed one is not', () => {
  assert.deepEqual(parseStoreNotice(restored), restored);
  assert.equal(parseStoreNotice(null), null);
  assert.equal(parseStoreNotice('x'), null);
  assert.equal(parseStoreNotice({ ...restored, kind: 'exploded' }), null);
  assert.equal(parseStoreNotice({ ...restored, id: '' }), null);
  assert.equal(parseStoreNotice({ ...restored, message: 3 }), null);
});

test('each notice is shown once, whether it came from the listing or the event', () => {
  const shown = new Set<string>();
  const reset = { ...restored, id: 'settings:reset:C:/data/app_settings.json', kind: 'reset' };
  assert.deepEqual(unseenNotices([restored, { bad: true }], shown).map(n => n.id), [restored.id]);
  assert.deepEqual(unseenNotices([restored, reset], shown).map(n => n.id), [reset.id]);
  assert.deepEqual(unseenNotices([reset], shown), []);
  assert.deepEqual(unseenNotices('not a list', shown), []);
});

test('the toast says where the file is', () => {
  const notice = parseStoreNotice(restored);
  assert.ok(notice);
  assert.equal(noticeDescription(notice), `${restored.message} File: ${restored.path}`);
  const named = { ...notice, message: `${restored.path} could not be read.` };
  assert.equal(noticeDescription(named), named.message, 'not repeated');
});
