/**
 * What to do about a file or folder that could not be indexed: one line and
 * the one action that helps, read from the indexer's reason.
 *
 * Pure module (no React), unit-tested with Node (`app/tests/indexingAdvice.test.ts`).
 */

export type IndexingAction =
  /** Run the indexing again (transient: locked file, interrupted, unknown). */
  | 'reindex'
  /** Open the folder in the file manager (moved, missing or unreadable files). */
  | 'show_in_folder'
  /** Install the search models (nothing can be indexed without them). */
  | 'install_search_models'
  /** Nothing in the app helps (the file itself must change). */
  | 'none';

export interface IndexingAdvice {
  /** One line on what to do. */
  hint: string;
  action: IndexingAction;
  /** The button's label; null when `action` is `none`. */
  actionLabel: string | null;
}

interface Rule {
  test: RegExp;
  advice: IndexingAdvice;
}

const RULES: readonly Rule[] = [
  {
    test: /search models?|embedding model|needs? (first-run )?setup|model (is )?not installed/i,
    advice: { hint: 'Search is not set up yet: install the search models, then index again.', action: 'install_search_models', actionLabel: 'Set up search' },
  },
  {
    test: /password|encrypted|drm/i,
    advice: { hint: 'The file is password-protected. Save an unlocked copy into the folder, then index again.', action: 'show_in_folder', actionLabel: 'Show in folder' },
  },
  {
    test: /being used by another process|locked|sharing violation|resource busy/i,
    advice: { hint: 'Another app has the file open. Close it there, then index again.', action: 'reindex', actionLabel: 'Index again' },
  },
  {
    test: /permission denied|access (is )?denied|not permitted|unauthori[sz]ed/i,
    advice: { hint: 'Shodh may not read this location. Check its permissions (or move the files somewhere you own), then index again.', action: 'show_in_folder', actionLabel: 'Show in folder' },
  },
  {
    test: /no such file|not found|cannot find|does not exist|path .*invalid/i,
    advice: { hint: 'The file or folder moved or was deleted. Remove it from the library, or add its new location.', action: 'show_in_folder', actionLabel: 'Show in folder' },
  },
  {
    test: /unsupported|not supported|unknown (file )?(type|format)|no text|empty (file|document)|scanned/i,
    advice: { hint: 'Shodh cannot read text from this file. Convert it (for example to PDF with text, or Markdown), then index again.', action: 'none', actionLabel: null },
  },
  {
    test: /too large|file size|exceeds/i,
    advice: { hint: 'The file is larger than Shodh indexes. Split it into smaller files, then index again.', action: 'none', actionLabel: null },
  },
  {
    test: /disk|no space|storage full/i,
    advice: { hint: 'The disk is full. Free some space, then index again.', action: 'reindex', actionLabel: 'Index again' },
  },
];

const DEFAULT_ADVICE: IndexingAdvice = {
  hint: 'Index again. If it keeps failing, the reason above says what the indexer could not do.',
  action: 'reindex',
  actionLabel: 'Index again',
};

/** Advice for one failure reason. */
export function indexingAdvice(reason: string | null | undefined): IndexingAdvice {
  const text = (reason ?? '').trim();
  if (!text) return DEFAULT_ADVICE;
  return RULES.find(rule => rule.test.test(text))?.advice ?? DEFAULT_ADVICE;
}

/** Advice for a folder's failed files: that of the most common kind of reason. */
export function adviceForFailures(reasons: readonly (string | null | undefined)[]): IndexingAdvice | null {
  if (reasons.length === 0) return null;
  const counts = new Map<IndexingAdvice, number>();
  for (const reason of reasons) {
    const advice = indexingAdvice(reason);
    counts.set(advice, (counts.get(advice) ?? 0) + 1);
  }
  let best: IndexingAdvice | null = null;
  let bestCount = 0;
  for (const [advice, count] of counts) {
    if (count > bestCount) {
      best = advice;
      bestCount = count;
    }
  }
  return best;
}
