/**
 * Characters that change how text looks without showing themselves, exactly the set
 * Shodh refuses in a command (`hidden_character` in `harness/run_command/mod.rs`; a test
 * keeps the two equal): control characters other than line breaks and tabs,
 * bidirectional overrides and isolates (which can make "txt.exe" read as "exe.txt"),
 * zero-width and other invisible formatting characters, variation selectors and the
 * tag characters (U+E0000–U+E007F, which carry text a model reads and a person cannot
 * see). An approval prompt, a diff and the code review show text a model or another
 * person wrote, so each such character is shown as a visible ⟨U+XXXX⟩ instead.
 *
 * Pure (no imports), so `node --test` loads it (`app/tests/hiddenChars.test.ts`).
 */

/** The hidden characters as inclusive code point ranges. */
export const HIDDEN_RANGES: readonly (readonly [number, number])[] = [
  [0x0000, 0x0008],
  [0x000b, 0x001f],
  [0x007f, 0x009f],
  [0x00ad, 0x00ad],
  [0x034f, 0x034f],
  [0x061c, 0x061c],
  [0x115f, 0x1160],
  [0x17b4, 0x17b5],
  [0x180b, 0x180e],
  [0x200b, 0x200f],
  [0x202a, 0x202e],
  [0x2060, 0x206f],
  [0x3164, 0x3164],
  [0xfe00, 0xfe0f],
  [0xfeff, 0xfeff],
  [0xffa0, 0xffa0],
  [0xfff9, 0xfffb],
  [0xe0000, 0xe007f],
];

/** Whether code point `cp` is hidden. */
export function isHiddenCodePoint(cp: number): boolean {
  return HIDDEN_RANGES.some(([from, to]) => cp >= from && cp <= to);
}

function visible(cp: number): string {
  return `⟨U+${cp.toString(16).toUpperCase().padStart(4, '0')}⟩`;
}

/** `text` with every hidden character made visible. */
export function revealHidden(text: string): string {
  let out = '';
  for (const ch of text) {
    const cp = ch.codePointAt(0) ?? 0;
    out += isHiddenCodePoint(cp) ? visible(cp) : ch;
  }
  return out;
}

/** Whether `text` holds any such character. */
export function hasHidden(text: string): boolean {
  for (const ch of text) {
    if (isHiddenCodePoint(ch.codePointAt(0) ?? 0)) return true;
  }
  return false;
}
