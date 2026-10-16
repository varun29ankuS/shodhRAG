/**
 * The sidebar's New menu: items, labels, per-platform hints and the
 * Ctrl/⌘+Shift+N shortcut. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/newMenu.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  NEW_MENU_LABEL,
  isNewChatChord,
  newMenuItems,
  newMenuKeyShortcuts,
  newMenuShortcutHint,
  opensNewMenu,
} from '../src/lib/newMenu.ts';

const key = (patch: Partial<Parameters<typeof opensNewMenu>[0]> = {}) => ({
  key: 'N',
  code: 'KeyN',
  ctrlKey: true,
  metaKey: false,
  shiftKey: true,
  altKey: false,
  repeat: false,
  ...patch,
});
const free = { modalOpen: false };

test('the menu lists the five ways to start something, in order', () => {
  assert.deepEqual(newMenuItems(false).map(i => [i.id, i.label]), [
    ['new-chat', 'New chat'],
    ['new-workspace', 'New workspace…'],
    ['new-task', 'New task…'],
    ['new-event', 'New event…'],
    ['add-folder', 'Add folder to Library…'],
  ]);
});

test('labels that open a dialog end with a real ellipsis', () => {
  assert.equal(NEW_MENU_LABEL, 'New…');
  for (const item of newMenuItems(false).slice(1)) assert.ok(item.label.endsWith('…'), item.label);
  assert.ok(!newMenuItems(false)[0].label.includes('...'));
});

test('only New chat carries a shortcut hint, per platform', () => {
  assert.deepEqual(newMenuItems(false).map(i => i.hint ?? null), ['Ctrl+N', null, null, null, null]);
  assert.equal(newMenuItems(true)[0].hint, '⌘N');
});

test('the shortcut that opens the menu is named per platform', () => {
  assert.equal(newMenuShortcutHint(false), 'Ctrl+Shift+N');
  assert.equal(newMenuShortcutHint(true), '⌘⇧N');
  assert.equal(newMenuKeyShortcuts(false), 'Control+Shift+N');
  assert.equal(newMenuKeyShortcuts(true), 'Meta+Shift+N');
});

test('Ctrl+Shift+N and Cmd+Shift+N open the menu', () => {
  assert.equal(opensNewMenu(key(), free), true);
  assert.equal(opensNewMenu(key({ key: 'n' }), free), true);
  assert.equal(opensNewMenu(key({ ctrlKey: false, metaKey: true }), free), true);
});

test('the N key is matched by position, so other keyboard layouts work', () => {
  // Russian layout: the N key types "т".
  assert.equal(opensNewMenu(key({ key: 'Т' }), free), true);
  assert.equal(isNewChatChord(key({ key: 'т', shiftKey: false })), true);
  // Another key that types an N on its layout does not count.
  assert.equal(opensNewMenu(key({ code: 'KeyB' }), free), false);
  assert.equal(isNewChatChord(key({ code: 'KeyB', key: 'n', shiftKey: false })), false);
});

test('Ctrl+N and Cmd+N, without Shift or Alt, are New chat', () => {
  assert.equal(isNewChatChord(key({ shiftKey: false })), true);
  assert.equal(isNewChatChord(key({ shiftKey: false, ctrlKey: false, metaKey: true })), true);
  assert.equal(isNewChatChord(key()), false, 'Ctrl+Shift+N opens the menu');
  assert.equal(isNewChatChord(key({ shiftKey: false, altKey: true })), false);
  assert.equal(isNewChatChord(key({ shiftKey: false, ctrlKey: false })), false);
});

test('other chords do not', () => {
  assert.equal(opensNewMenu(key({ shiftKey: false }), free), false, 'Ctrl+N is New chat');
  assert.equal(opensNewMenu(key({ altKey: true }), free), false);
  assert.equal(opensNewMenu(key({ ctrlKey: false }), free), false);
  assert.equal(opensNewMenu(key({ code: 'KeyM', key: 'M' }), free), false);
});

test('not behind a modal dialog or on key repeat; a text field does not block it', () => {
  assert.equal(opensNewMenu(key(), { modalOpen: true }), false);
  assert.equal(opensNewMenu(key({ repeat: true }), free), false);
  // The context carries no text-field flag: the chord types nothing.
  assert.equal(opensNewMenu(key(), free), true);
});
