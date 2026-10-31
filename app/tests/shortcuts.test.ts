/**
 * The shortcut registry: hints written from the same entry as the binding, on
 * Windows and macOS (⌘ only on a Mac), every shortcut that has a visible button
 * shows its hint there, and no hint is written by hand anywhere in the app.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/shortcuts.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
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

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) out.push(...sourceFiles(path));
    else if (/\.tsx?$/.test(name)) out.push(path);
  }
  return out;
}

test('hints are platform-correct', () => {
  assert.equal(shortcutText('newChat', false), 'Ctrl+N');
  assert.equal(shortcutText('newChat', true), '⌘N');
  assert.equal(shortcutText('newMenu', false), 'Ctrl+Shift+N');
  assert.equal(shortcutText('newMenu', true), '⌘⇧N');
  assert.equal(shortcutText('textLarger', false), 'Ctrl +');
  assert.equal(shortcutText('textSmaller', true), '⌘ −');
  assert.equal(shortcutText('send', false), 'Enter');
  assert.equal(shortcutText('interrupt', true), 'Esc');
  assert.equal(shortcutText('saveField', true), '⌘Enter');
  assert.equal(ariaKeyShortcuts('newMenu', false), 'Control+Shift+N');
  assert.equal(ariaKeyShortcuts('textLarger', false), 'Control+Plus');
  assert.equal(ariaKeyShortcuts('textSmaller', true), 'Meta+Minus');
  assert.equal(ariaKeyShortcuts('close', false), 'Escape');
  assert.equal(withShortcut('New chat', 'newChat', false), 'New chat (Ctrl+N)');
  assert.equal(isMacPlatform('MacIntel'), true);
  assert.equal(isMacPlatform('Win32'), false);
  // The New menu reads the same registry.
  assert.equal(newMenuItems(false)[0].hint, shortcutText('newChat', false));
  assert.equal(newMenuShortcutHint(true), shortcutText('newMenu', true));
  assert.equal(newMenuKeyShortcuts(false), ariaKeyShortcuts('newMenu', false));
});

test('a chord shortcut is Ctrl alone on Windows and Cmd alone on a Mac', () => {
  for (const id of IDS.filter(id => SHORTCUTS[id].mod)) {
    const s = SHORTCUTS[id];
    const base = { shiftKey: s.shift, altKey: s.alt };
    const win = chord(s.keys[0], s.codes[0], { ...base, ctrlKey: true });
    const mac = chord(s.keys[0], s.codes[0], { ...base, metaKey: true });
    assert.ok(matchesShortcut(win, id, false), `${id} on Windows`);
    assert.ok(matchesShortcut(mac, id, true), `${id} on a Mac`);
    // The other platform's modifier is left to the text fields (a Mac's Ctrl+K kills a line).
    assert.ok(!matchesShortcut(win, id, true), `${id}: a Mac's Ctrl chord`);
    assert.ok(!matchesShortcut(mac, id, false), `${id}: the Windows key`);
    // Both modifiers at once, no modifier, or Alt: never.
    assert.ok(!matchesShortcut({ ...win, metaKey: true }, id, false), `${id} with Ctrl and Win`);
    assert.ok(!matchesShortcut(chord(s.keys[0], s.codes[0], base), id, false), `${id} without Ctrl`);
    assert.ok(!matchesShortcut({ ...win, altKey: !s.alt }, id, false), `${id} with Alt`);
    // No other chord shortcut takes its exact chord.
    const others = IDS.filter(other => other !== id && SHORTCUTS[other].mod && matchesShortcut(win, other, false));
    assert.deepEqual(others, [], `${id} is also ${others}`);
  }
});

test('a plain key shortcut fires only without Ctrl or Cmd', () => {
  for (const id of IDS.filter(id => !SHORTCUTS[id].mod)) {
    const s = SHORTCUTS[id];
    const e = chord(s.keys[0], s.codes[0], { shiftKey: s.shift });
    assert.ok(matchesShortcut(e, id, false), id);
    assert.ok(!matchesShortcut({ ...e, ctrlKey: true }, id, false), `${id} with Ctrl`);
    assert.ok(!matchesShortcut({ ...e, shiftKey: !s.shift }, id, false), `${id} with Shift changed`);
  }
  // Shift+Enter is the previous match, Enter the next.
  assert.ok(matchesShortcut(chord('Enter', 'Enter', { shiftKey: true }), 'prevMatch', false));
  assert.ok(!matchesShortcut(chord('Enter', 'Enter', { shiftKey: true }), 'nextMatch', false));
});

test('letters follow the layout, and keys with symbols work from the keypad and with Shift', () => {
  assert.ok(matchesShortcut(chord('n', 'KeyB', { ctrlKey: true }), 'newChat', false));
  assert.ok(matchesShortcut(chord('т', 'KeyN', { ctrlKey: true }), 'newChat', false));
  assert.ok(!matchesShortcut(chord('b', 'KeyN', { ctrlKey: true }), 'newChat', false));
  assert.ok(!matchesShortcut(chord('N', 'KeyN', { ctrlKey: true, shiftKey: true }), 'newChat', false));
  assert.ok(matchesShortcut(chord('N', 'KeyN', { ctrlKey: true, shiftKey: true }), 'newMenu', false));
  assert.ok(matchesShortcut(chord('+', 'Equal', { ctrlKey: true, shiftKey: true }), 'textLarger', false));
  assert.ok(matchesShortcut(chord('=', 'Equal', { ctrlKey: true }), 'textLarger', false));
  assert.ok(matchesShortcut(chord('+', 'NumpadAdd', { ctrlKey: true }), 'textLarger', false));
  assert.ok(matchesShortcut(chord('-', 'NumpadSubtract', { ctrlKey: true }), 'textSmaller', false));
  assert.ok(matchesShortcut(chord('_', 'Minus', { ctrlKey: true, shiftKey: true }), 'textSmaller', false));
  assert.ok(matchesShortcut(chord('0', 'Numpad0', { ctrlKey: true }), 'textReset', false));
});

test('every shortcut with a visible button shows its hint there', () => {
  for (const id of IDS) {
    for (const file of SHORTCUTS[id].shownIn) {
      const text = readFileSync(join(srcDir, file), 'utf8');
      assert.ok(text.includes(`<Kbd shortcut="${id}"`), `${file} shows no hint for ${id}`);
    }
  }
});

test('no shortcut hint is written by hand anywhere in the app', () => {
  // A hand-written hint: a modifier chord ("Ctrl+K", "Ctrl K", "⌘K") or a key in brackets
  // after a label ("(Esc)", "(Enter)"). Comments are not hints.
  const handWritten = /Ctrl\+|Ctrl [A-Z0-9]|⌘|⇧|\((?:Esc|Enter|Shift\+Enter|Tab)\)/;
  const found: string[] = [];
  for (const file of sourceFiles(srcDir)) {
    const where = relative(srcDir, file).replace(/\\/g, '/');
    if (where === 'lib/shortcuts.ts') continue;
    readFileSync(file, 'utf8').split('\n').forEach((line, i) => {
      const code = line.trim();
      if (code.startsWith('//') || code.startsWith('*') || code.startsWith('/*')) return;
      const withoutComment = code.replace(/\s\/\/.*$/, '');
      if (handWritten.test(withoutComment)) found.push(`${where}:${i + 1}: ${code}`);
    });
  }
  assert.deepEqual(found, []);
});
