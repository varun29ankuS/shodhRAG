/**
 * The consent line a paper alert shows before anything is sent. One constant, pinned
 * by tests on both sides (`shodh_rag::whats_new::alerts::CONSENT_TEMPLATE`).
 */

export const CONSENT_TEMPLATE =
  'Shodh sends these words: {words} to arXiv, OpenAlex and Semantic Scholar to look for new papers. Your files, your library and who you are stay on this computer.';

/** The words an alert sends: trimmed, one space between words. */
export function cleanWords(words: string): string {
  return words.split(/\s+/).filter(Boolean).join(' ');
}

/** The consent line for `words`, exactly as they will be sent. */
export function consentLine(words: string): string {
  return CONSENT_TEMPLATE.replace('{words}', cleanWords(words));
}

/** The words of ticked suggestions, in the order ticked, as one query. */
export function wordsFromChips(ticked: readonly string[]): string {
  return cleanWords(ticked.join(' '));
}
