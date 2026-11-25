/**
 * Which key sends a message from the composer (Settings → General, `preferences.sendKey`
 * in `app_settings.rs`): Enter, with Shift+Enter for a new line; or Ctrl+Enter (⌘ on a
 * Mac), with Enter for a new line. The same key steers a running answer and approves a
 * pending step from an empty box.
 *
 * Pure (type-only imports and the shortcut registry) so it is unit-tested directly with
 * Node (`app/tests/sendKey.test.ts`) and the floating chat can load it.
 */
import { matchesShortcut } from './shortcuts.ts';
import type { KeyChord, ShortcutId } from './shortcuts.ts';

export type SendKey = 'enter' | 'modEnter';

export const SEND_KEYS: readonly SendKey[] = ['enter', 'modEnter'];
export const DEFAULT_SEND_KEY: SendKey = 'enter';

/** A value from the settings file; anything this version does not know is Enter. */
export function parseSendKey(value: unknown): SendKey {
  return value === 'modEnter' ? 'modEnter' : DEFAULT_SEND_KEY;
}

/**
 * The registry's shortcut for `key` (its hint in tooltips and Settings). Ctrl+Enter is the
 * registry's one Ctrl+Enter chord, `saveField`: the composer's send is the same key.
 */
export function sendShortcut(key: SendKey): ShortcutId {
  return key === 'modEnter' ? 'saveField' : 'send';
}

/** What a key press in the composer does with `key` as the send key. */
export type ComposerKey = 'send' | 'newline' | 'other';

export function composerKey(e: KeyChord & { isComposing?: boolean }, key: SendKey, mac: boolean): ComposerKey {
  if (e.key !== 'Enter' || e.isComposing) return 'other';
  if (matchesShortcut(e, sendShortcut(key), mac)) return 'send';
  // Shift+Enter in Enter mode, and Enter in Ctrl+Enter mode, write a new line.
  if (key === 'enter' ? e.shiftKey && !e.ctrlKey && !e.metaKey && !e.altKey : matchesShortcut(e, 'send', mac)) return 'newline';
  return 'other';
}
