/**
 * The shortcut registry: hints written from the same entry as the binding, on
 * Windows and macOS, and every shortcut that has a visible button shows its hint
 * there. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/shortcuts.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  SHORTCUTS,
  ariaKeyShortcuts,
  isMacPlatform,
  matchesShortcut,
  shortcutText,
  withShortcut,
} from '../src/lib/shortcuts.ts';
import type { KeyChord, ShortcutId } from '../src/lib/shortcuts.ts';
import { newMenuItems, newMenuKeyShortcuts, newMenuShortcutHint } from '../src/lib/newMenu.ts';

const srcDir = join(dirname(fileURLToPath(import.meta.url)), '../src');
const IDS = Object.keys(SHORTCUTS) as ShortcutId[];

function chord(key: string, code: string, mods: Partial<KeyChord> = {}): KeyChord {
  return { key, code, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, ...mods };
}

test('hints are platform-correct', () => {
  assert.equal(shortcutText('newChat', false), 'Ctrl+N');
  assert.equal(shortcutText('newChat', true), '⌘N');
  assert.equal(shortcutText('newMenu', false), 'Ctrl+Shift+N');
  assert.equal(shortcutText('newMenu', true), '⌘⇧N');
  assert.equal(ariaKeyShortcuts('newMenu', false), 'Control+Shift+N');
  assert.equal(ariaKeyShortcuts('textSmaller', true), 'Meta+-');
  assert.equal(withShortcut('New chat', 'newChat', false), 'New chat (Ctrl+N)');
  assert.equal(isMacPlatform('MacIntel'), true);
  assert.equal(isMacPlatform('Win32'), false);
  // The New menu reads the same registry.
  assert.equal(newMenuItems(false)[0].hint, shortcutText('newChat', false));
  assert.equal(newMenuShortcutHint(true), shortcutText('newMenu', true));
  assert.equal(newMenuKeyShortcuts(false), ariaKeyShortcuts('newMenu', false));
});

test('each shortcut matches its own binding, with Ctrl or ⌘, and nothing else', () => {
  for (const id of IDS) {
    const s = SHORTCUTS[id];
    for (const mod of [{ ctrlKey: true }, { metaKey: true }]) {
      const e = chord(s.keys[0], s.codes[0], { ...mod, shiftKey: s.shift, altKey: s.alt });
      assert.ok(matchesShortcut(e, id), `${id} with ${JSON.stringify(mod)}`);
      // Without the modifier it is plain typing.
      assert.ok(!matchesShortcut(chord(s.keys[0], s.codes[0], { shiftKey: s.shift }), id), `${id} without Ctrl`);
      // Alt never matches.
      assert.ok(!matchesShortcut({ ...e, altKey: !s.alt }, id), `${id} with Alt`);
    }
    // No other shortcut takes its exact chord.
    const own = chord(s.keys[0], s.codes[0], { ctrlKey: true, shiftKey: s.shift, altKey: s.alt });
    const others = IDS.filter(other => other !== id && matchesShortcut(own, other));
    assert.deepEqual(others, [], `${id} is also ${others}`);
  }
});

test('letters follow the layout, and keys with symbols work from the keypad too', () => {
  // Dvorak types N from another key; a Cyrillic layout types т from the N key.
  assert.ok(matchesShortcut(chord('n', 'KeyB', { ctrlKey: true }), 'newChat'));
  assert.ok(matchesShortcut(chord('т', 'KeyN', { ctrlKey: true }), 'newChat'));
  assert.ok(!matchesShortcut(chord('b', 'KeyN', { ctrlKey: true }), 'newChat'));
  // Shift+N is the New menu, not New chat.
  assert.ok(!matchesShortcut(chord('N', 'KeyN', { ctrlKey: true, shiftKey: true }), 'newChat'));
  assert.ok(matchesShortcut(chord('N', 'KeyN', { ctrlKey: true, shiftKey: true }), 'newMenu'));
  // Ctrl and "+" is typed with Shift on many layouts, or on the keypad.
  assert.ok(matchesShortcut(chord('+', 'Equal', { ctrlKey: true, shiftKey: true }), 'textLarger'));
  assert.ok(matchesShortcut(chord('=', 'Equal', { ctrlKey: true }), 'textLarger'));
  assert.ok(matchesShortcut(chord('+', 'NumpadAdd', { ctrlKey: true }), 'textLarger'));
  assert.ok(matchesShortcut(chord('-', 'NumpadSubtract', { ctrlKey: true }), 'textSmaller'));
  assert.ok(matchesShortcut(chord('0', 'Numpad0', { ctrlKey: true }), 'textReset'));
});

test('every shortcut with a visible button shows its hint there', () => {
  for (const id of IDS) {
    for (const file of SHORTCUTS[id].shownIn) {
      const text = readFileSync(join(srcDir, file), 'utf8');
      assert.ok(text.includes(`<Kbd shortcut="${id}"`), `${file} shows no hint for ${id}`);
    }
  }
  // Hints are written only through the registry: no hand-written Ctrl+ strings remain
  // where a registered shortcut is shown.
  for (const file of ['components/shell/Sidebar.tsx', 'components/shell/ConversationDock.tsx', 'lib/newMenu.ts']) {
    const text = readFileSync(join(srcDir, file), 'utf8');
    assert.doesNotMatch(text, /'(⌘|Ctrl)[+ ]?[A-Z]'/, file);
    assert.doesNotMatch(text, /\(Ctrl\+J\)/, file);
  }
});
