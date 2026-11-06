/**
 * The one diff Shodh shows (`shodh_rag::code_review::diff`): its data, and the rules
 * the shared diff component lays it out by: side-by-side rows, the unchanged lines
 * between hunks, long files folded, line selection and the language a path is
 * highlighted as.
 *
 * Pure (type-only imports, no enums) so it is unit-tested directly with Node
 * (`app/tests/codeDiff.test.ts`).
 */

export type DiffKind = 'same' | 'added' | 'removed';

export interface DiffLine {
  kind: DiffKind;
  text: string;
  /** Its number in the old version (null for an added line), from 1. */
  old: number | null;
  /** Its number in the new version (null for a removed line), from 1. */
  new: number | null;
}

export interface DiffHunk {
  lines: DiffLine[];
}

export interface FileChange {
  path: string;
  status: 'added' | 'removed' | 'changed';
  binary: boolean;
  tooLarge: boolean;
  hunks: DiffHunk[];
}

/** Lines a file shows before the rest is folded behind "Show all". */
export const FOLD_AFTER_LINES = 160;

/** Files shown open at once; past it, each starts closed. */
export const OPEN_FILES = 6;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function lineNumber(value: unknown): number | null | undefined {
  if (value === null || value === undefined) return null;
  return typeof value === 'number' && Number.isInteger(value) && value > 0 ? value : undefined;
}

/** A list of file changes from a backend payload (an approval's `codeDiff`), or null. */
export function parseFileChanges(value: unknown): FileChange[] | null {
  if (!Array.isArray(value)) return null;
  const out: FileChange[] = [];
  for (const item of value) {
    if (!isRecord(item) || typeof item.path !== 'string' || !Array.isArray(item.hunks)) return null;
    const status = item.status;
    if (status !== 'added' && status !== 'removed' && status !== 'changed') return null;
    const hunks: DiffHunk[] = [];
    for (const hunk of item.hunks) {
      if (!isRecord(hunk) || !Array.isArray(hunk.lines)) return null;
      const lines: DiffLine[] = [];
      for (const line of hunk.lines) {
        if (!isRecord(line) || typeof line.text !== 'string') return null;
        const kind = line.kind;
        if (kind !== 'same' && kind !== 'added' && kind !== 'removed') return null;
        const old = lineNumber(line.old);
        const next = lineNumber(line.new);
        if (old === undefined || next === undefined) return null;
        lines.push({ kind, text: line.text, old, new: next });
      }
      hunks.push({ lines });
    }
    out.push({ path: item.path, status, binary: item.binary === true, tooLarge: item.tooLarge === true, hunks });
  }
  return out;
}

/** Lines added and removed in a file, for its header ("+12 −3"). */
export function changeCounts(change: FileChange): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const hunk of change.hunks) {
    for (const line of hunk.lines) {
      if (line.kind === 'added') added++;
      else if (line.kind === 'removed') removed++;
    }
  }
  return { added, removed };
}

/** One row of the side-by-side view: the old line on the left, the new on the right. */
export interface SideRow {
  left: DiffLine | null;
  right: DiffLine | null;
}

/**
 * A hunk as side-by-side rows: unchanged lines on both sides, and each run of removed
 * lines paired with the run of added lines that follows it (the longer run leaves
 * blanks on the other side).
 */
export function sideBySide(hunk: DiffHunk): SideRow[] {
  const rows: SideRow[] = [];
  const lines = hunk.lines;
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (line.kind === 'same') {
      rows.push({ left: line, right: line });
      i++;
      continue;
    }
    const removed: DiffLine[] = [];
    const added: DiffLine[] = [];
    while (i < lines.length && lines[i].kind === 'removed') removed.push(lines[i++]);
    while (i < lines.length && lines[i].kind === 'added') added.push(lines[i++]);
    for (let k = 0; k < Math.max(removed.length, added.length); k++) {
      rows.push({ left: removed[k] ?? null, right: added[k] ?? null });
    }
  }
  return rows;
}

/** The new-side (else old-side) number of a line. */
export function shownNumber(line: DiffLine): number {
  return line.new ?? line.old ?? 0;
}

/**
 * Unchanged lines not shown before hunk `index` (since the previous hunk, or since the
 * top of the file for the first), counted on the old side.
 */
export function hiddenBefore(hunks: readonly DiffHunk[], index: number): number {
  const first = hunks[index]?.lines.find(l => l.old !== null)?.old;
  if (first === undefined || first === null) return 0;
  if (index === 0) return first - 1;
  const previous = hunks[index - 1].lines;
  let last: number | null = null;
  for (let k = previous.length - 1; k >= 0 && last === null; k--) last = previous[k].old;
  return last === null ? 0 : Math.max(0, first - last - 1);
}

/** Lines a file shows in all. */
export function lineCount(change: FileChange): number {
  return change.hunks.reduce((n, h) => n + h.lines.length, 0);
}

/**
 * The hunks shown while a long file is folded: whole hunks up to `limit` lines (at least
 * the first hunk, cut at `limit`). `hidden` is the number of lines folded away.
 */
export function foldedHunks(change: FileChange, limit: number = FOLD_AFTER_LINES): { hunks: DiffHunk[]; hidden: number } {
  const total = lineCount(change);
  if (total <= limit) return { hunks: change.hunks, hidden: 0 };
  const shown: DiffHunk[] = [];
  let used = 0;
  for (const hunk of change.hunks) {
    if (used + hunk.lines.length > limit) {
      if (shown.length === 0) shown.push({ lines: hunk.lines.slice(0, limit) });
      break;
    }
    shown.push(hunk);
    used += hunk.lines.length;
  }
  const kept = shown.reduce((n, h) => n + h.lines.length, 0);
  return { hunks: shown, hidden: total - kept };
}

/** Lines selected in one file of a diff or file view: `from` to `to` by shown number. */
export interface LineSelection {
  path: string;
  from: number;
  to: number;
}

/** A click on line `line` of `path`, with Shift extending `current` (same file). */
export function extendSelection(current: LineSelection | null, path: string, line: number, extend: boolean): LineSelection {
  if (extend && current && current.path === path) {
    return { path, from: Math.min(current.from, line), to: Math.max(current.to, line) };
  }
  return { path, from: line, to: line };
}

/** Whether `line` (by shown number) of `path` is selected. */
export function isSelected(selection: LineSelection | null, path: string, line: number): boolean {
  return selection !== null && selection.path === path && line >= selection.from && line <= selection.to;
}

/** The diff lines of `change` whose shown number is in the selection (every line when none). */
export function selectedLines(change: FileChange, selection: LineSelection | null): DiffLine[] {
  const all = change.hunks.flatMap(h => h.lines);
  if (!selection || selection.path !== change.path) return all;
  return all.filter(l => isSelected(selection, change.path, shownNumber(l)));
}

/** A file's text as one hunk of unchanged lines, numbered from `first`. */
export function textAsChange(path: string, text: string, first = 1): FileChange {
  const lines = text.replace(/\r\n/g, '\n').replace(/\n$/, '').split('\n');
  return {
    path,
    status: 'changed',
    binary: false,
    tooLarge: false,
    hunks: [{ lines: lines.map((t, i) => ({ kind: 'same' as const, text: t, old: first + i, new: first + i })) }],
  };
}

/** Old/new line diff ops (an instruction diff) numbered as a one-hunk change. */
export function opsAsChange(path: string, ops: readonly { op: DiffKind; text: string }[]): FileChange {
  let old = 1;
  let next = 1;
  const lines: DiffLine[] = ops.map(({ op, text }) => {
    if (op === 'added') return { kind: op, text, old: null, new: next++ };
    if (op === 'removed') return { kind: op, text, old: old++, new: null };
    return { kind: op, text, old: old++, new: next++ };
  });
  return { path, status: 'changed', binary: false, tooLarge: false, hunks: lines.length ? [{ lines }] : [] };
}

const LANGUAGES: Record<string, string> = {
  rs: 'rust',
  ts: 'typescript',
  mts: 'typescript',
  cts: 'typescript',
  tsx: 'tsx',
  js: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  jsx: 'jsx',
  py: 'python',
  go: 'go',
  java: 'java',
  kt: 'kotlin',
  swift: 'swift',
  c: 'c',
  h: 'c',
  cc: 'cpp',
  cpp: 'cpp',
  hpp: 'cpp',
  cs: 'csharp',
  rb: 'ruby',
  php: 'php',
  sh: 'bash',
  bash: 'bash',
  ps1: 'powershell',
  json: 'json',
  toml: 'toml',
  yaml: 'yaml',
  yml: 'yaml',
  md: 'markdown',
  html: 'markup',
  xml: 'markup',
  svg: 'markup',
  css: 'css',
  scss: 'scss',
  sql: 'sql',
};

/** The highlighter's language for `path` (`text` when unknown). */
export function languageOf(path: string): string {
  const name = path.split('/').pop() ?? path;
  if (name === 'Dockerfile') return 'docker';
  if (name === 'Makefile') return 'makefile';
  const dot = name.lastIndexOf('.');
  if (dot <= 0) return 'text';
  return LANGUAGES[name.slice(dot + 1).toLowerCase()] ?? 'text';
}
