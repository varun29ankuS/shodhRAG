/**
 * The app's keyboard shortcuts, in one registry: what each one is bound to, how it
 * is written for people (Ctrl on Windows and Linux, ⌘ on macOS) and for
 * `aria-keyshortcuts`, and whether a key event is it. Every handler matches with
 * `matchesShortcut` and every hint is written from the same entry (the `<Kbd>`
 * component, `withShortcut` in tooltips and copy), so a hint can never drift from its
 * binding; a test fails on any hint written by hand anywhere in `app/src`.
 *
 * On macOS the modifier is ⌘ only: Ctrl+K, Ctrl+B, Ctrl+F and Ctrl+N are the text
 * fields' own editing keys there, and stay theirs.
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
  | 'findNext'
  | 'findPrev'
  | 'selectionAsk'
  | 'textLarger'
  | 'textSmaller'
  | 'textReset'
  | 'saveField'
  | 'send'
  | 'steer'
  | 'interrupt'
  | 'nextMatch'
  | 'prevMatch'
  | 'close';

export interface Shortcut {
  /** What it does, for lists of shortcuts. */
  label: string;
  /** Ctrl (Windows, Linux) or ⌘ (macOS). */
  mod: boolean;
  shift: boolean;
  alt: boolean;
  /** Shift may be held or not (keys whose symbol is typed with Shift on some layouts). */
  shiftFree: boolean;
  /** The key as written in the hint ("N", "+", "Enter"). */
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
    shiftFree: false,
    display: key.toUpperCase(),
    keys: [key.toLowerCase()],
    codes: [`Key${key.toUpperCase()}`],
    shownIn: options.shownIn ?? [],
  };
}

/** A key pressed on its own (or with Shift): it acts in one place (a field, a panel). */
function plain(label: string, display: string, keys: readonly string[], codes: readonly string[], shift = false): Shortcut {
  return { label, mod: false, shift, alt: false, shiftFree: false, display, keys, codes, shownIn: [] };
}

const ENTER = ['Enter'] as const;
const ENTER_CODES = ['Enter', 'NumpadEnter'] as const;

export const SHORTCUTS: Record<ShortcutId, Shortcut> = {
  newChat: letter('New chat', 'n', { shownIn: ['components/shell/Sidebar.tsx'] }),
  newMenu: letter('New…', 'n', { shift: true }),
  commandPalette: letter('Search and commands', 'k'),
  toggleSidebar: letter('Show or hide the sidebar', 'b'),
  toggleDock: letter('Open or minimise the conversation', 'j', { shownIn: ['components/shell/ConversationDock.tsx'] }),
  findInChat: letter('Find', 'f'),
  findNext: letter('Next match', 'g'),
  findPrev: letter('Previous match', 'g', { shift: true }),
  selectionAsk: letter('Ask about the selected text', 'a', { shift: true }),
  textLarger: {
    label: 'Larger text',
    mod: true,
    shift: false,
    alt: false,
    // Ctrl and the "=" key (which has "+" on it), with or without Shift, or the keypad's +.
    shiftFree: true,
    display: '+',
    keys: ['+', '='],
    codes: ['Equal', 'NumpadAdd'],
    shownIn: ['components/AppearanceSettings.tsx'],
  },
  textSmaller: {
    label: 'Smaller text',
    mod: true,
    shift: false,
    alt: false,
    // As for +: with or without Shift ("_" is Shift and "-"), or the keypad's −.
    shiftFree: true,
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
    shiftFree: false,
    display: '0',
    keys: ['0'],
    codes: ['Digit0', 'Numpad0'],
    shownIn: ['components/AppearanceSettings.tsx'],
  },
  saveField: { ...plain('Save', 'Enter', ENTER, ENTER_CODES), mod: true },
  send: plain('Send', 'Enter', ENTER, ENTER_CODES),
  steer: plain('Steer the answer', 'Enter', ENTER, ENTER_CODES),
  interrupt: plain('Interrupt the answer', 'Esc', ['Escape'], ['Escape']),
  nextMatch: plain('Next match', 'Enter', ENTER, ENTER_CODES),
  prevMatch: plain('Previous match', 'Enter', ENTER, ENTER_CODES, true),
  close: plain('Close', 'Esc', ['Escape'], ['Escape']),
};

/** Whether this computer is a Mac (⌘ instead of Ctrl). */
export function isMacPlatform(platform: string | undefined): boolean {
  return /Mac|iPhone|iPad/i.test(platform ?? '');
}

/** This computer's platform, as the app's handlers and hints use it. */
export const IS_MAC_PLATFORM = typeof navigator !== 'undefined' && isMacPlatform(navigator.platform);

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

/**
 * The shortcut as one string: "Ctrl+Shift+N" on Windows, "⌘⇧N" on a Mac. A key that is
 * itself a plus or minus sign is set apart with a space ("Ctrl +", not "Ctrl++").
 */
export function shortcutText(id: ShortcutId, mac: boolean): string {
  const parts = shortcutParts(id, mac);
  const key = parts[parts.length - 1];
  const sign = key === '+' || key === '−';
  if (mac) return sign ? `${parts.slice(0, -1).join('')} ${key}` : parts.join('');
  return sign ? `${parts.slice(0, -1).join('+')} ${key}` : parts.join('+');
}

/** `aria-keyshortcuts` value: "Control+Shift+N", "Meta+Plus". */
export function ariaKeyShortcuts(id: ShortcutId, mac: boolean): string {
  const s = SHORTCUTS[id];
  const parts: string[] = [];
  if (s.mod) parts.push(mac ? 'Meta' : 'Control');
  if (s.alt) parts.push('Alt');
  if (s.shift) parts.push('Shift');
  const key = s.display === '+' ? 'Plus' : s.display === '−' ? 'Minus' : s.display === 'Esc' ? 'Escape' : s.display;
  parts.push(key);
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
 * Whether `e` is shortcut `id` on this platform (`mac`): ⌘ alone on a Mac, Ctrl alone
 * elsewhere (so a Mac's Ctrl editing keys never trigger it), Shift and Alt exactly
 * (except for keys typed with Shift on some layouts), and the key. A letter on a Latin
 * layout is matched by what it types (Dvorak and Colemak move it); on any other layout
 * by where QWERTY has it.
 */
export function matchesShortcut(e: KeyChord, id: ShortcutId, mac: boolean = IS_MAC_PLATFORM): boolean {
  const s = SHORTCUTS[id];
  const mod = mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  const none = !e.ctrlKey && !e.metaKey;
  if (s.mod ? !mod : !none) return false;
  if (e.altKey !== s.alt) return false;
  if (!s.shiftFree && e.shiftKey !== s.shift) return false;
  const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
  if (s.keys.includes(key)) return true;
  const typesLatinLetter = /^[a-z]$/i.test(e.key);
  const isLetterShortcut = /^[a-z]$/.test(s.keys[0] ?? '');
  if (isLetterShortcut) return !typesLatinLetter && s.codes.includes(e.code);
  return s.codes.includes(e.code);
}
