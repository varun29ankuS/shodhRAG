/**
 * The sidebar's New menu: items, labels, per-platform hints and the
 * Ctrl/⌘+Shift+N shortcut. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/newMenu.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  NEW_MENU_LABEL,
  isNKey,
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
const free = { modalOpen: false, mac: false };

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
  assert.equal(opensNewMenu(key({ ctrlKey: false, metaKey: true }), { ...free, mac: true }), true);
  assert.equal(opensNewMenu(key({ ctrlKey: false, metaKey: true }), { ...free, mac: false }), false);
  assert.equal(opensNewMenu(key(), { ...free, mac: true }), false, "a Mac's Ctrl chords are not the menu's");
});

test('N is the letter on Latin layouts and the physical key otherwise', () => {
  assert.equal(isNKey({ key: 'n', code: 'KeyN' }), true, 'QWERTY');
  assert.equal(isNKey({ key: 'N', code: 'KeyN' }), true, 'Shift gives an uppercase N');
  assert.equal(isNKey({ key: 'n', code: 'KeyL' }), true, 'Dvorak: N sits where QWERTY has L');
  assert.equal(isNKey({ key: 'b', code: 'KeyN' }), false, 'Dvorak: the QWERTY N position types b');
  assert.equal(isNKey({ key: 'т', code: 'KeyN' }), true, 'Russian layout');
  assert.equal(isNKey({ key: 'Т', code: 'KeyN' }), true, 'Russian layout with Shift');
  assert.equal(isNKey({ key: 'ल', code: 'KeyN' }), true, 'Devanagari layout');
  assert.equal(isNKey({ key: 'т', code: 'KeyB' }), false, 'another key on a Russian layout');
});

test('both shortcuts use the N rule', () => {
  assert.equal(opensNewMenu(key({ key: 'Т' }), free), true);
  assert.equal(isNewChatChord(key({ key: 'т', shiftKey: false })), true);
  assert.equal(opensNewMenu(key({ key: 'N', code: 'KeyL' }), free), true, 'Dvorak');
  assert.equal(isNewChatChord(key({ key: 'n', code: 'KeyL', shiftKey: false })), true, 'Dvorak');
  assert.equal(opensNewMenu(key({ key: 'B', code: 'KeyN' }), free), false);
  assert.equal(isNewChatChord(key({ key: 'b', code: 'KeyN', shiftKey: false })), false);
});

test('Ctrl+N and Cmd+N, without Shift or Alt, are New chat', () => {
  assert.equal(isNewChatChord(key({ shiftKey: false })), true);
  assert.equal(isNewChatChord(key({ shiftKey: false, ctrlKey: false, metaKey: true }), true), true);
  assert.equal(isNewChatChord(key({ shiftKey: false }), true), false, "a Mac's Ctrl+N stays the text field's");
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
