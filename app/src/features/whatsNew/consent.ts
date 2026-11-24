/**
 * The consent line a paper alert shows before anything is sent. One constant, pinned
 * by tests on both sides (`shodh_rag::whats_new::alerts::CONSENT_TEMPLATE`).
 */

export const CONSENT_TEMPLATE =
  'Shodh sends these words: {words} to arXiv, OpenAlex and Semantic Scholar to look for new papers. Your files, your library and who you are stay on this computer.';

/** Whitespace as Rust's `char::is_whitespace` reads it (`split_whitespace` in `clean_words`). */
const RUST_WHITESPACE: ReadonlySet<number> = new Set([
  0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x20, 0x85, 0xa0, 0x1680,
  0x2000, 0x2001, 0x2002, 0x2003, 0x2004, 0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200a,
  0x2028, 0x2029, 0x202f, 0x205f, 0x3000,
]);

/** The words an alert sends: trimmed, one space between words (as `clean_words`). */
export function cleanWords(words: string): string {
  const out: string[] = [];
  let word = '';
  for (const ch of words) {
    if (RUST_WHITESPACE.has(ch.codePointAt(0) ?? 0)) {
      if (word) out.push(word);
      word = '';
    } else {
      word += ch;
    }
  }
  if (word) out.push(word);
  return out.join(' ');
}

/** Longest words an alert sends, in characters (`MAX_WORDS_CHARS`). */
export const MAX_WORDS_CHARS = 200;

/** Characters as Rust counts them (`chars().count()`): code points, not UTF-16 units. */
export function charCount(text: string): number {
  return [...text].length;
}

/** The consent line for `words`, exactly as they will be sent. */
export function consentLine(words: string): string {
  return CONSENT_TEMPLATE.replace('{words}', cleanWords(words));
}

/** The words of ticked suggestions, in the order ticked, as one query. */
export function wordsFromChips(ticked: readonly string[]): string {
  return cleanWords(ticked.join(' '));
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * The words after ticking (`on`) or unticking a chip, keeping whatever the user typed:
 * a ticked chip is added at the end unless its words are already there; an unticked
 * one takes out its words, and nothing else.
 */
export function toggleChip(words: string, chip: string, on: boolean): string {
  const current = cleanWords(words);
  const phrase = cleanWords(chip);
  if (!phrase) return current;
  const pattern = new RegExp(`(^| )${escapeRegExp(phrase)}(?= |$)`, 'i');
  if (on) return pattern.test(current) ? current : cleanWords(`${current} ${phrase}`);
  return cleanWords(current.replace(pattern, ' '));
}
