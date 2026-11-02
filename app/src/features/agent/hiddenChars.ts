/**
 * Characters that change how text looks without showing themselves: bidirectional
 * overrides and isolates (U+202A–U+202E, U+2066–U+2069, which can make "txt.exe" read as
 * "exe.txt"), zero-width characters (U+200B–U+200F) and the byte-order mark (U+FEFF).
 * An approval prompt shows text a model or another person wrote (commit titles, comments,
 * pull request bodies), so each such character is shown as a visible ⟨U+XXXX⟩ instead.
 *
 * Pure (no imports), so `node --test` loads it (`app/tests/hiddenChars.test.ts`).
 */

const HIDDEN = /[​-‏‪-‮⁦-⁩﻿]/g;

/** `text` with every hidden formatting character made visible. */
export function revealHidden(text: string): string {
  return text.replace(HIDDEN, ch => `⟨U+${ch.charCodeAt(0).toString(16).toUpperCase().padStart(4, '0')}⟩`);
}

/** Whether `text` holds any such character. */
export function hasHidden(text: string): boolean {
  HIDDEN.lastIndex = 0;
  const found = HIDDEN.test(text);
  HIDDEN.lastIndex = 0;
  return found;
}
