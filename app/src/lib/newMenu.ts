/**
 * The sidebar's New menu: one place to start a chat, workspace, task, event
 * or Library folder. The ▾ beside "New chat" opens it, and so does
 * Ctrl+Shift+N (⌘⇧N on a Mac) from anywhere but a modal dialog, text fields
 * included. Each item runs the same handler as its command-palette entry.
 *
 * Pure module (its one import has its extension) so it is unit-tested directly with
 * Node (`app/tests/newMenu.test.ts`). The shortcuts come from the registry
 * (`shortcuts.ts`), so the hints and the bindings are the same.
 */

import { IS_MAC_PLATFORM, ariaKeyShortcuts, matchesShortcut, shortcutText } from './shortcuts.ts';

export type NewMenuItemId = 'new-chat' | 'new-workspace' | 'new-task' | 'new-event' | 'add-folder';

export interface NewMenuItem {
  id: NewMenuItemId;
  label: string;
  /** Keyboard hint shown at the end of the item. */
  hint?: string;
}

/** Accessible name of the ▾ trigger. */
export const NEW_MENU_LABEL = 'New…';

/** The items, in menu order. Labels ending in … open a dialog or picker. */
export function newMenuItems(isMac: boolean): NewMenuItem[] {
  return [
    { id: 'new-chat', label: 'New chat', hint: shortcutText('newChat', isMac) },
    { id: 'new-workspace', label: 'New workspace…' },
    { id: 'new-task', label: 'New task…' },
    { id: 'new-event', label: 'New event…' },
    { id: 'add-folder', label: 'Add folder to Library…' },
  ];
}

/** The shortcut that opens the menu, as shown to people. */
export function newMenuShortcutHint(isMac: boolean): string {
  return shortcutText('newMenu', isMac);
}

/** The same shortcut for `aria-keyshortcuts`. */
export function newMenuKeyShortcuts(isMac: boolean): string {
  return ariaKeyShortcuts('newMenu', isMac);
}

export interface ShortcutKey {
  key: string;
  /** The physical key (`KeyN` is where QWERTY has N), whatever it types. */
  code: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  repeat: boolean;
}

/**
 * Whether the key is N. On a layout that types Latin letters the letter
 * decides, wherever it sits (Dvorak and Colemak move N); on any other
 * layout (Cyrillic, Devanagari, …) the key in QWERTY's N position does.
 */
export function isNKey(e: Pick<ShortcutKey, 'key' | 'code'>): boolean {
  if (e.key.toLowerCase() === 'n') return true;
  return !/^[a-z]$/i.test(e.key) && e.code === 'KeyN';
}

/** Ctrl/⌘+N, without Shift or Alt: New chat. */
export function isNewChatChord(e: ShortcutKey, mac: boolean = IS_MAC_PLATFORM): boolean {
  return matchesShortcut(e, 'newChat', mac);
}

/** Whether Ctrl/⌘+Shift+N is pressed, whatever the context. */
export function isNewMenuChord(e: ShortcutKey, mac: boolean = IS_MAC_PLATFORM): boolean {
  return matchesShortcut(e, 'newMenu', mac);
}

/**
 * Whether a key press opens the menu: the chord, not repeated, and not
 * behind a modal dialog. It also works from a text field: the chord types
 * nothing there.
 */
export function opensNewMenu(e: ShortcutKey, context: { modalOpen: boolean; mac?: boolean }): boolean {
  return isNewMenuChord(e, context.mac ?? IS_MAC_PLATFORM) && !e.repeat && !context.modalOpen;
}
