/**
 * Why a file or folder could not be indexed, in plain words, and the one
 * action that helps, read from the indexer's reason. The raw reason stays
 * available as the detail.
 *
 * The phrases matched here are the ones the indexer writes; the Rust tests
 * pin them on synthetic broken files (`locked_and_damaged_pdfs_are_not_reported_as_scans`
 * and `damaged_office_files_say_what_failed` in crates/shodh-rag/src/processing/parser.rs).
 * Operating-system errors are matched by their code (`os error 112`), not by
 * their text, which Windows translates.
 *
 * Pure module (no React), unit-tested with Node (`app/tests/indexingAdvice.test.ts`).
 */

export type IndexingAction =
  /** Run the indexing again (transient: locked file, interrupted, unknown). */
  | 'reindex'
  /** Open the folder in the file manager (moved, missing or unreadable files). */
  | 'show_in_folder'
  /** Install the search models (nothing can be indexed without them), or the scan reader installed with them. */
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

/** What went wrong, in kinds a person can act on. */
export type FailureKind =
  | 'scan_reader_missing'
  | 'search_models_missing'
  | 'encrypted'
  | 'disk_full'
  | 'too_large'
  | 'in_use'
  | 'permission'
  | 'damaged'
  | 'no_text'
  | 'unsupported'
  | 'missing'
  | 'other';

/** A failure explained: its kind, a plain reason, what to do, and the raw detail. */
export interface FailureExplanation {
  kind: FailureKind;
  /** Short plain reason, e.g. "Protected with a password". */
  reason: string;
  advice: IndexingAdvice;
  /** What the indexer said, for a "details" view; empty when it said nothing. */
  detail: string;
}

interface Rule {
  kind: FailureKind;
  test: RegExp;
  reason: string;
  advice: IndexingAdvice;
}

const REINDEX: IndexingAdvice = {
  hint: 'Index again. If it keeps failing, the details say what the indexer could not do.',
  action: 'reindex',
  actionLabel: 'Index again',
};

const CANNOT_READ: IndexingAdvice = {
  hint: 'Shodh cannot read text from this file. Convert it (for example to PDF with text, or Markdown), then index again.',
  action: 'none',
  actionLabel: null,
};

/** In order: the first rule that matches decides. */
const RULES: readonly Rule[] = [
  {
    // Before the search-model rule: the reason also says the reader is not installed.
    kind: 'scan_reader_missing',
    test: /scan reader/i,
    reason: 'A scan, and the scan reader is not installed',
    advice: { hint: 'This file is a scan. Install the scan reader (31 MB); scans are then read automatically.', action: 'install_search_models', actionLabel: 'Install the scan reader' },
  },
  {
    kind: 'search_models_missing',
    test: /search_models_missing|search models?|embedding model|needs? (first-run )?setup|model (is )?not installed/i,
    reason: 'Search is not set up yet',
    advice: { hint: 'Search is not set up yet: install the search models, then index again.', action: 'install_search_models', actionLabel: 'Set up search' },
  },
  {
    kind: 'encrypted',
    test: /password|encrypted|\bdrm\b/i,
    reason: 'Protected with a password',
    advice: { hint: 'The file is password-protected. Save an unlocked copy into the folder, then index again.', action: 'show_in_folder', actionLabel: 'Show in folder' },
  },
  {
    // ERROR_DISK_FULL (112), ERROR_HANDLE_DISK_FULL (39), ENOSPC (28).
    kind: 'disk_full',
    test: /os error (112|39|28)\b|not enough space|no space left|disk (is )?full|storage full/i,
    reason: 'The disk is full',
    advice: { hint: 'The disk is full. Free some space, then index again.', action: 'reindex', actionLabel: 'Index again' },
  },
  {
    // EFBIG (27), ERROR_FILE_TOO_LARGE (223), running out of memory on one file, and
    // the indexer's own limits ("... too large to index (limit 50 MB for text files)",
    // crates/shodh-rag/src/processing/limits.rs).
    kind: 'too_large',
    test: /os error (27|223)\b|too large|file size|out of memory|memory allocation|capacity overflow/i,
    reason: 'Too large to index',
    advice: { hint: 'The file is larger than Shodh can index. Split it into smaller files, then index again.', action: 'none', actionLabel: null },
  },
  {
    // ERROR_SHARING_VIOLATION (32), ERROR_LOCK_VIOLATION (33), EBUSY (16).
    kind: 'in_use',
    test: /os error (32|33|16)\b|being used by another process|\blocked\b|sharing violation|resource busy/i,
    reason: 'Open in another app',
    advice: { hint: 'Another app has the file open. Close it there, then index again.', action: 'reindex', actionLabel: 'Index again' },
  },
  {
    // ERROR_ACCESS_DENIED (5), EACCES (13), EPERM (1).
    kind: 'permission',
    test: /os error (5|13|1)\b|permission denied|access (is )?denied|not permitted|unauthori[sz]ed/i,
    reason: 'Shodh may not read this file',
    advice: { hint: 'Shodh may not read this location. Check its permissions (or move the files somewhere you own), then index again.', action: 'show_in_folder', actionLabel: 'Show in folder' },
  },
  {
    kind: 'damaged',
    test: /damaged|incomplete|corrupt|as zip|zip archive|invalid zip|failed to open spreadsheet|malformed|unexpected end of (file|stream)|not valid xml|nests more than \d+ elements deep|cannot open pdf|missing word\/document\.xml|invalid (file )?header/i,
    reason: 'Damaged or incomplete',
    advice: { hint: 'The file is damaged or was not fully saved or downloaded. Open it in its app to check, save a fresh copy into the folder, then index again.', action: 'show_in_folder', actionLabel: 'Show in folder' },
  },
  {
    kind: 'no_text',
    test: /no extractable text|no text|contains no tabular data|empty (file|document)|scanned/i,
    reason: 'No readable text in the file',
    advice: CANNOT_READ,
  },
  {
    kind: 'unsupported',
    test: /unsupported|not supported|unknown (file )?(type|format)/i,
    reason: 'This file type is not supported',
    advice: CANNOT_READ,
  },
  {
    // ERROR_FILE_NOT_FOUND (2), ERROR_PATH_NOT_FOUND (3), ENOENT (2).
    kind: 'missing',
    test: /os error (2|3)\b|no such file|not found|cannot find|does not exist|path .*invalid|is not available/i,
    reason: 'Moved or deleted',
    advice: { hint: 'The file or folder moved or was deleted. Remove it from the library, or add its new location.', action: 'show_in_folder', actionLabel: 'Show in folder' },
  },
];

const OTHER: Omit<FailureExplanation, 'detail'> = {
  kind: 'other',
  reason: 'Could not be read',
  advice: REINDEX,
};

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * `reason` without the path of `file` (and its name), so a file called
 * "encrypted-notes.pdf" or a folder called "Password docs" is not read as
 * the reason itself.
 */
export function withoutPath(reason: string, file?: string | null): string {
  if (!file) return reason;
  const forms = new Set<string>();
  const slashed = file.replace(/\\/g, '/');
  forms.add(file);
  forms.add(slashed);
  forms.add(slashed.replace(/\//g, '\\'));
  const name = slashed.split('/').filter(Boolean).pop();
  if (name) forms.add(name);
  let out = reason;
  // Longest first, so a full path goes before its file name.
  for (const form of [...forms].sort((a, b) => b.length - a.length)) {
    if (form.length === 0) continue;
    out = out.replace(new RegExp(escapeRegExp(form), 'gi'), '');
  }
  return out;
}

/** Explain why `file` (when known) could not be indexed, from the indexer's `reason`. */
export function explainFailure(reason: string | null | undefined, file?: string | null): FailureExplanation {
  const detail = (reason ?? '').trim();
  if (!detail) return { ...OTHER, detail: '' };
  const text = withoutPath(detail, file);
  const rule = RULES.find(r => r.test.test(text));
  return rule ? { kind: rule.kind, reason: rule.reason, advice: rule.advice, detail } : { ...OTHER, detail };
}

/** Advice for one failure reason. */
export function indexingAdvice(reason: string | null | undefined, file?: string | null): IndexingAdvice {
  return explainFailure(reason, file).advice;
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

/** One failed file in a group: where it is, and what the indexer said. */
export interface FailedFile {
  file: string;
  /** Path below the folder ("2024/Q3/statement.pdf"), or the whole path outside it. */
  relative: string;
  detail: string;
}

/** Failed files that failed for the same kind of reason. */
export interface FailureGroup {
  kind: FailureKind;
  reason: string;
  advice: IndexingAdvice;
  files: FailedFile[];
}

/** `file` below `root`, with forward slashes; the whole path when it is not below it. */
export function relativeToFolder(file: string, root: string | null | undefined): string {
  const slashed = file.replace(/\\/g, '/');
  if (!root) return slashed;
  const base = root.replace(/\\/g, '/').replace(/\/+$/, '');
  const windows = /^[a-zA-Z]:\//.test(base) || base.startsWith('//');
  const same = (a: string, b: string) => (windows ? a.toLowerCase() === b.toLowerCase() : a === b);
  if (slashed.length > base.length + 1 && same(slashed.slice(0, base.length), base) && slashed[base.length] === '/') {
    return slashed.slice(base.length + 1);
  }
  return slashed;
}

/**
 * A folder's failed files grouped by kind of reason, largest group first, each
 * group's files in path order, so hundreds of failures read as a few reasons
 * with the exact files under each.
 */
export function groupFailures(
  failures: readonly { file: string; reason: string }[],
  root?: string | null,
): FailureGroup[] {
  const groups = new Map<FailureKind, FailureGroup>();
  for (const failure of failures) {
    const explained = explainFailure(failure.reason, failure.file);
    let group = groups.get(explained.kind);
    if (!group) {
      group = { kind: explained.kind, reason: explained.reason, advice: explained.advice, files: [] };
      groups.set(explained.kind, group);
    }
    group.files.push({ file: failure.file, relative: relativeToFolder(failure.file, root), detail: explained.detail });
  }
  const out = [...groups.values()];
  for (const group of out) group.files.sort((a, b) => a.relative.localeCompare(b.relative));
  return out.sort((a, b) => b.files.length - a.files.length || a.reason.localeCompare(b.reason));
}
