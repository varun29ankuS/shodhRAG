/**
 * The app's keyboard shortcuts, in one registry: what each one is bound to, how it
 * is written for people (Ctrl on Windows and Linux, ⌘ on macOS) and for
 * `aria-keyshortcuts`, and whether a key event is it. Every handler matches with
 * `matchesShortcut` and every hint is written from the same entry (the `<Kbd>`
 * component, tooltips), so a hint can never drift from its binding.
 *
 * Pure (no imports) so it is unit-tested directly with Node (`app/tests/shortcuts.test.ts`).
 */

export type ShortcutId =
  | 'newChat'
  | 'newMenu'
  | 'commandPalette'
  | 'toggleSidebar'
  | 'toggleDock'
  | 'findInChat'
  | 'textLarger'
  | 'textSmaller'
  | 'textReset';

export interface Shortcut {
  /** What it does, for lists of shortcuts. */
  label: string;
  /** Ctrl (Windows, Linux) or ⌘ (macOS). */
  mod: boolean;
  shift: boolean;
  alt: boolean;
  /** The key as written in the hint ("N", "+"). */
  display: string;
  /** `KeyboardEvent.key` values that are this key (lowercase for letters). */
  keys: readonly string[];
  /**
   * `KeyboardEvent.code` of the physical key: a letter shortcut also works on a layout
   * that types no Latin letters (Cyrillic, Devanagari), from the key where QWERTY has it.
   */
  codes: readonly string[];
  /**
   * The source files (under `app/src`) where a button shows this shortcut as a visible
   * hint (`<Kbd shortcut=…>`); the shortcuts test checks each one does.
   */
  shownIn: readonly string[];
}

function letter(label: string, key: string, options: Partial<Pick<Shortcut, 'shift' | 'alt' | 'shownIn'>> = {}): Shortcut {
  return {
    label,
    mod: true,
    shift: options.shift ?? false,
    alt: options.alt ?? false,
    display: key.toUpperCase(),
    keys: [key.toLowerCase()],
    codes: [`Key${key.toUpperCase()}`],
    shownIn: options.shownIn ?? [],
  };
}

export const SHORTCUTS: Record<ShortcutId, Shortcut> = {
  newChat: letter('New chat', 'n', { shownIn: ['components/shell/Sidebar.tsx'] }),
  newMenu: letter('New…', 'n', { shift: true }),
  commandPalette: letter('Search and commands', 'k'),
  toggleSidebar: letter('Show or hide the sidebar', 'b'),
  toggleDock: letter('Open or minimise the conversation', 'j', { shownIn: ['components/shell/ConversationDock.tsx'] }),
  findInChat: letter('Find in the conversation', 'f'),
  textLarger: {
    label: 'Larger text',
    mod: true,
    shift: false,
    alt: false,
    display: '+',
    // Ctrl and the "=" key (which has "+" on it), with or without Shift, or the keypad's +.
    keys: ['+', '='],
    codes: ['Equal', 'NumpadAdd'],
    shownIn: ['components/AppearanceSettings.tsx'],
  },
  textSmaller: {
    label: 'Smaller text',
    mod: true,
    shift: false,
    alt: false,
    display: '−',
    keys: ['-', '_'],
    codes: ['Minus', 'NumpadSubtract'],
    shownIn: ['components/AppearanceSettings.tsx'],
  },
  textReset: {
    label: 'Default text size',
    mod: true,
    shift: false,
    alt: false,
    display: '0',
    keys: ['0'],
    codes: ['Digit0', 'Numpad0'],
    shownIn: ['components/AppearanceSettings.tsx'],
  },
};

/** Whether this computer is a Mac (⌘ instead of Ctrl). */
export function isMacPlatform(platform: string | undefined): boolean {
  return /Mac|iPhone|iPad/i.test(platform ?? '');
}

/** The keys as shown on a button: ["Ctrl", "Shift", "N"] or ["⌘", "⇧", "N"]. */
export function shortcutParts(id: ShortcutId, mac: boolean): string[] {
  const s = SHORTCUTS[id];
  const parts: string[] = [];
  if (s.mod) parts.push(mac ? '⌘' : 'Ctrl');
  if (s.alt) parts.push(mac ? '⌥' : 'Alt');
  if (s.shift) parts.push(mac ? '⇧' : 'Shift');
  parts.push(s.display);
  return parts;
}

/** The shortcut as one string: "Ctrl+Shift+N" on Windows, "⌘⇧N" on a Mac. */
export function shortcutText(id: ShortcutId, mac: boolean): string {
  return shortcutParts(id, mac).join(mac ? '' : '+');
}

/** `aria-keyshortcuts` value: "Control+Shift+N" or "Meta+Shift+N". */
export function ariaKeyShortcuts(id: ShortcutId, mac: boolean): string {
  const s = SHORTCUTS[id];
  const parts: string[] = [];
  if (s.mod) parts.push(mac ? 'Meta' : 'Control');
  if (s.alt) parts.push('Alt');
  if (s.shift) parts.push('Shift');
  parts.push(s.display === '−' ? '-' : s.display);
  return parts.join('+');
}

/** "New chat (Ctrl+N)": a tooltip or accessible name with its shortcut. */
export function withShortcut(label: string, id: ShortcutId, mac: boolean): string {
  return `${label} (${shortcutText(id, mac)})`;
}

/** The parts of a key event `matchesShortcut` reads. */
export interface KeyChord {
  key: string;
  code: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}

/**
 * Whether `e` is shortcut `id`: Ctrl or ⌘ as the shortcut says, Shift and Alt exactly
 * (except for "+", typed with Shift on many layouts), and the key. A letter on a Latin
 * layout is matched by what it types (Dvorak and Colemak move it); on any other layout by
 * where QWERTY has it.
 */
export function matchesShortcut(e: KeyChord, id: ShortcutId): boolean {
  const s = SHORTCUTS[id];
  const mod = e.ctrlKey || e.metaKey;
  if (mod !== s.mod || e.altKey !== s.alt) return false;
  const shiftFree = id === 'textLarger';
  if (!shiftFree && e.shiftKey !== s.shift) return false;
  const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
  if (s.keys.includes(key)) return true;
  const typesLatinLetter = /^[a-z]$/i.test(e.key);
  const isLetterShortcut = /^[a-z]$/.test(s.keys[0] ?? '');
  if (isLetterShortcut) return !typesLatinLetter && s.codes.includes(e.code);
  return s.codes.includes(e.code);
}
