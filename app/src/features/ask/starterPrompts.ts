/**
 * The starter questions Ask offers on an empty conversation, fitted to what
 * the library holds (`library_profile`: research papers or other
 * documents). No model is asked: the kind is read from the indexed text.
 *
 * Pure module so it is unit-tested with Node (`app/tests/starterPrompts.test.ts`).
 */

export type LibraryKind = 'papers' | 'documents';

/** For contracts, invoices, notes and mixed files (and before the kind is known). */
export const DOCUMENT_PROMPTS: readonly string[] = [
  'Summarize the key points across my documents',
  'What dates and deadlines are mentioned in my files?',
  'Which documents mention payments or amounts owed?',
  'List the people and organizations that appear most often',
];

/** For a library of research papers: methods, results, comparisons. */
export const PAPER_PROMPTS: readonly string[] = [
  'Compare the methods these papers propose',
  'What results do the papers report, and on which datasets?',
  'Which baselines does each paper compare against?',
  'What limitations or open problems do the authors point out?',
];

/** The `kind` of a `library_profile` answer, or null when it is not one. */
export function readLibraryKind(value: unknown): LibraryKind | null {
  if (typeof value !== 'object' || value === null) return null;
  const kind = (value as Record<string, unknown>).kind;
  return kind === 'papers' || kind === 'documents' ? kind : null;
}

/** The starter questions for a library of `kind` (the general ones while it is unknown). */
export function starterPrompts(kind: LibraryKind | null): readonly string[] {
  return kind === 'papers' ? PAPER_PROMPTS : DOCUMENT_PROMPTS;
}
