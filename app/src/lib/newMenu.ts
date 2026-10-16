/**
 * The sidebar's New menu: one place to start a chat, workspace, task, event
 * or Library folder. The ▾ beside "New chat" opens it, and so does
 * Ctrl+Shift+N (⌘⇧N on a Mac) from anywhere but a modal dialog, text fields
 * included. Each item runs the same handler as its command-palette entry.
 *
 * Pure module (no runtime imports) so it is unit-tested directly with Node
 * (`app/tests/newMenu.test.ts`).
 */

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
    { id: 'new-chat', label: 'New chat', hint: isMac ? '⌘N' : 'Ctrl+N' },
    { id: 'new-workspace', label: 'New workspace…' },
    { id: 'new-task', label: 'New task…' },
    { id: 'new-event', label: 'New event…' },
    { id: 'add-folder', label: 'Add folder to Library…' },
  ];
}

/** The shortcut that opens the menu, as shown to people. */
export function newMenuShortcutHint(isMac: boolean): string {
  return isMac ? '⌘⇧N' : 'Ctrl+Shift+N';
}

/** The same shortcut for `aria-keyshortcuts`. */
export function newMenuKeyShortcuts(isMac: boolean): string {
  return isMac ? 'Meta+Shift+N' : 'Control+Shift+N';
}

export interface ShortcutKey {
  key: string;
  /** The physical key: `KeyN` on every layout, whatever it types. */
  code: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  repeat: boolean;
}

/**
 * Ctrl/⌘+N: New chat. Matched on the physical key, so it also works on
 * layouts where that key types another letter (Cyrillic, Greek, …).
 */
export function isNewChatChord(e: ShortcutKey): boolean {
  return (e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.code === 'KeyN';
}

/** Whether Ctrl/⌘+Shift+N is pressed, whatever the context (physical N key). */
export function isNewMenuChord(e: ShortcutKey): boolean {
  return (e.ctrlKey || e.metaKey) && e.shiftKey && !e.altKey && e.code === 'KeyN';
}

/**
 * Whether a key press opens the menu: the chord, not repeated, and not
 * behind a modal dialog. It also works from a text field: the chord types
 * nothing there.
 */
export function opensNewMenu(e: ShortcutKey, context: { modalOpen: boolean }): boolean {
  return isNewMenuChord(e) && !e.repeat && !context.modalOpen;
}
