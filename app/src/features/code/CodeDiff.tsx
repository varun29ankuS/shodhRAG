import React, { useMemo, useState } from 'react';
import { ChevronDown, ChevronRight, Columns2, Rows3 } from 'lucide-react';
import { cn } from '../../lib/utils';
import { useTheme } from '../../contexts/ThemeContext';
import { CodeHighlight } from '../../components/CodeHighlight';
import { revealHidden } from '../agent/hiddenChars';
import {
  OPEN_FILES,
  changeCounts,
  foldedHunks,
  hiddenBefore,
  isSelected,
  languageOf,
  shownNumber,
  sideBySide,
  textAsChange,
} from './diffModel';
import type { DiffHunk, DiffLine, FileChange, LineSelection } from './diffModel';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

/** One code row's height: the gutter and the highlighted code line up on it. */
const ROW = '20px';

const CODE_STYLE: React.CSSProperties = {
  margin: 0,
  padding: 0,
  background: 'transparent',
  fontSize: '12px',
  lineHeight: ROW,
  overflow: 'visible',
};

const STATUS_WORDS: Record<FileChange['status'], string> = {
  added: 'Added',
  removed: 'Removed',
  changed: 'Changed',
};

function mark(kind: DiffLine['kind']): string {
  return kind === 'added' ? '+' : kind === 'removed' ? '−' : ' ';
}

function spoken(line: DiffLine): string {
  const word = line.kind === 'added' ? 'added' : line.kind === 'removed' ? 'removed' : 'unchanged';
  return `Line ${shownNumber(line)}, ${word}: ${line.text === '' ? 'empty' : line.text}`;
}

function rowClass(kind: DiffLine['kind'] | null, selected: boolean): string {
  return cn(
    kind === 'added' && 'bg-shodh-success/10',
    kind === 'removed' && 'bg-shodh-error/10',
    kind === null && 'bg-shodh-raised-2/60',
    selected && 'bg-shodh-accent-soft',
  );
}

interface SideProps {
  path: string;
  lines: (DiffLine | null)[];
  /** Which number the gutter shows: both (unified), or one side. */
  numbers: 'both' | 'old' | 'new';
  language: string;
  dark: boolean;
  selection: LineSelection | null;
  onLine?: (line: number, extend: boolean) => void;
}

/** A gutter of numbers and +/− marks beside the highlighted code, row for row. */
function CodeColumn({ path, lines, numbers, language, dark, selection, onLine }: SideProps) {
  const code = lines.map(l => (l ? revealHidden(l.text) : '')).join('\n');
  const lineProps = useMemo(
    () => (index: number) => {
      const line = lines[index - 1] ?? null;
      const selected = line !== null && isSelected(selection, path, shownNumber(line));
      return {
        style: { display: 'block', minHeight: ROW, paddingRight: '12px' } as React.CSSProperties,
        className: rowClass(line?.kind ?? null, selected),
      };
    },
    [lines, selection, path],
  );
  return (
    <div className="flex min-w-0 flex-1">
      <ol className="m-0 p-0 list-none shrink-0 select-none font-mono text-caption text-shodh-text-faint" style={{ lineHeight: ROW }}>
        {lines.map((line, i) => {
          if (!line) {
            return <li key={i} aria-hidden="true" className={cn('px-1.5', rowClass(null, false))} style={{ height: ROW }} />;
          }
          const selected = isSelected(selection, path, shownNumber(line));
          return (
            <li
              key={i}
              data-selected={selected ? 'true' : undefined}
              className={cn('flex gap-1 pl-1.5 pr-1 tabular-nums', rowClass(line.kind, selected), onLine && 'cursor-pointer hover:text-shodh-text')}
              style={{ height: ROW }}
              onClick={onLine ? e => onLine(shownNumber(line), e.shiftKey) : undefined}
            >
              <span className="sr-only">{spoken(line)}</span>
              {numbers !== 'new' && <span aria-hidden="true" className="w-9 text-right">{line.old ?? ''}</span>}
              {numbers !== 'old' && <span aria-hidden="true" className="w-9 text-right">{line.new ?? ''}</span>}
              <span aria-hidden="true" className={cn('w-3 text-center', line.kind !== 'same' && 'text-shodh-text-secondary font-semibold')}>
                {mark(line.kind)}
              </span>
            </li>
          );
        })}
      </ol>
      <div aria-hidden="true" className="min-w-0 flex-1 overflow-x-auto scrollbar-thin">
        <CodeHighlight code={code} language={language} dark={dark} preTag="div" customStyle={CODE_STYLE} lineProps={lineProps} />
      </div>
    </div>
  );
}

function HunkView({
  path,
  hunk,
  sideways,
  language,
  dark,
  selection,
  onLine,
}: {
  path: string;
  hunk: DiffHunk;
  sideways: boolean;
  language: string;
  dark: boolean;
  selection: LineSelection | null;
  onLine?: (line: number, extend: boolean) => void;
}) {
  const rows = useMemo(() => (sideways ? sideBySide(hunk) : null), [hunk, sideways]);
  if (!rows) {
    return <CodeColumn path={path} lines={hunk.lines} numbers="both" language={language} dark={dark} selection={selection} onLine={onLine} />;
  }
  return (
    <div className="flex divide-x divide-shodh-border-subtle">
      <CodeColumn path={path} lines={rows.map(r => r.left)} numbers="old" language={language} dark={dark} selection={selection} />
      <CodeColumn path={path} lines={rows.map(r => r.right)} numbers="new" language={language} dark={dark} selection={selection} onLine={onLine} />
    </div>
  );
}

interface FileViewProps {
  change: FileChange;
  sideways: boolean;
  dark: boolean;
  startOpen: boolean;
  selection: LineSelection | null;
  onSelect?: (selection: LineSelection) => void;
  language?: string;
}

function FileDiff({ change, sideways, dark, startOpen, selection, onSelect, language }: FileViewProps) {
  const [open, setOpen] = useState(startOpen);
  const [showAll, setShowAll] = useState(false);
  const counts = changeCounts(change);
  const lang = language ?? languageOf(change.path);
  const { hunks, hidden } = showAll ? { hunks: change.hunks, hidden: 0 } : foldedHunks(change);
  const onLine = onSelect
    ? (line: number, extend: boolean) => {
        const current = selection && selection.path === change.path ? selection : null;
        onSelect(
          extend && current
            ? { path: change.path, from: Math.min(current.from, line), to: Math.max(current.to, line) }
            : { path: change.path, from: line, to: line },
        );
      }
    : undefined;
  const label = `${STATUS_WORDS[change.status]} ${change.path}: ${counts.added} ${counts.added === 1 ? 'line' : 'lines'} added, ${counts.removed} removed`;
  return (
    <section aria-label={label} className="border-b border-shodh-border-subtle last:border-b-0">
      <h4 className="m-0">
        <button
          type="button"
          onClick={() => setOpen(o => !o)}
          aria-expanded={open}
          className={cn('w-full flex items-center gap-2 px-3 py-1.5 text-left text-caption text-shodh-text-secondary hover:bg-shodh-raised-2', FOCUS_RING)}
        >
          {open ? <ChevronDown className="w-3.5 h-3.5 shrink-0" aria-hidden="true" /> : <ChevronRight className="w-3.5 h-3.5 shrink-0" aria-hidden="true" />}
          <span className="font-semibold">{STATUS_WORDS[change.status]}</span>
          <span className="font-mono truncate min-w-0 text-shodh-text">{change.path}</span>
          <span className="ml-auto shrink-0 font-mono" aria-hidden="true">
            +{counts.added} −{counts.removed}
          </span>
        </button>
      </h4>
      {open && (
        change.binary ? (
          <p className="m-0 px-3 py-2 text-small text-shodh-text-muted">Not text: only that it changed is shown.</p>
        ) : change.tooLarge ? (
          <p className="m-0 px-3 py-2 text-small text-shodh-text-muted">Too large to show line by line here.</p>
        ) : (
          <div>
            {hunks.map((hunk, index) => {
              const gap = hiddenBefore(change.hunks, index);
              return (
                <React.Fragment key={index}>
                  {gap > 0 && (
                    <p className="m-0 px-3 py-0.5 text-caption text-shodh-text-muted bg-shodh-raised border-y border-shodh-border-subtle">
                      {`${gap} unchanged ${gap === 1 ? 'line' : 'lines'}`}
                    </p>
                  )}
                  <HunkView path={change.path} hunk={hunk} sideways={sideways} language={lang} dark={dark} selection={selection} onLine={onLine} />
                  {onSelect && hunk.lines.length > 0 && (
                    <button
                      type="button"
                      onClick={() => {
                        const numbers = hunk.lines.map(shownNumber);
                        onSelect({ path: change.path, from: Math.min(...numbers), to: Math.max(...numbers) });
                      }}
                      className={cn('sr-only focus:not-sr-only focus:block focus:px-3 focus:py-1 text-caption text-shodh-accent-text', FOCUS_RING)}
                    >
                      {`Select lines ${Math.min(...hunk.lines.map(shownNumber))} to ${Math.max(...hunk.lines.map(shownNumber))}`}
                    </button>
                  )}
                </React.Fragment>
              );
            })}
            {hidden > 0 && (
              <button
                type="button"
                onClick={() => setShowAll(true)}
                className={cn('w-full px-3 py-1.5 text-left text-caption text-shodh-accent-text hover:bg-shodh-raised-2', FOCUS_RING)}
              >
                {`Show ${hidden} more ${hidden === 1 ? 'line' : 'lines'}`}
              </button>
            )}
          </div>
        )
      )}
    </section>
  );
}

export interface CodeDiffProps {
  changes: FileChange[];
  /** What the diff is, for screen readers ("Proposed change to src/lib.rs"). */
  label: string;
  /** Lines selected (by shown number), highlighted. */
  selection?: LineSelection | null;
  /** Makes lines selectable: a click selects a line, Shift+click extends it. */
  onSelect?: (selection: LineSelection) => void;
  /** Highlight every file as this language (the path decides otherwise). */
  language?: string;
  /** Extra controls in the toolbar (Review this). */
  actions?: React.ReactNode;
  className?: string;
  /** Height the diff scrolls within. */
  maxHeight?: number;
}

/**
 * The one diff of the app: unified or side by side, highlighted, with line numbers,
 * the unchanged lines between hunks counted, long files folded and many files closed.
 * Each line is marked +, − or blank and named for screen readers ("Line 12, added:
 * …"); colour is never the only sign.
 */
export function CodeDiff({ changes, label, selection = null, onSelect, language, actions, className, maxHeight = 420 }: CodeDiffProps) {
  const { theme } = useTheme();
  const dark = theme === 'dark';
  const [sideways, setSideways] = useState(false);
  const box = React.useRef<HTMLDivElement>(null);
  // A citation's lines are brought into view (only when they are out of it).
  React.useEffect(() => {
    box.current?.querySelector('[data-selected="true"]')?.scrollIntoView({ block: 'nearest' });
  }, [selection]);
  const totals = changes.reduce(
    (sum, change) => {
      const c = changeCounts(change);
      return { added: sum.added + c.added, removed: sum.removed + c.removed };
    },
    { added: 0, removed: 0 },
  );
  if (changes.length === 0) {
    return <p className={cn('text-small text-shodh-text-muted', className)}>No changes.</p>;
  }
  return (
    <div role="group" aria-label={label} className={cn('rounded-lg border border-shodh-border-subtle bg-shodh-surface overflow-hidden', className)}>
      <div className="flex items-center gap-2 px-2 py-1 border-b border-shodh-border-subtle bg-shodh-raised">
        <p className="m-0 text-caption text-shodh-text-muted">
          {`${changes.length} ${changes.length === 1 ? 'file' : 'files'}, ${totals.added} added, ${totals.removed} removed`}
        </p>
        <div className="ml-auto flex items-center gap-1">
          {actions}
          <button
            type="button"
            onClick={() => setSideways(s => !s)}
            aria-pressed={sideways}
            className={cn('inline-flex items-center gap-1.5 h-7 px-2 rounded-md text-caption text-shodh-text-secondary hover:bg-shodh-raised-2 hover:text-shodh-text', FOCUS_RING)}
          >
            {sideways ? <Rows3 className="w-3.5 h-3.5" aria-hidden="true" /> : <Columns2 className="w-3.5 h-3.5" aria-hidden="true" />}
            {sideways ? 'One column' : 'Side by side'}
          </button>
        </div>
      </div>
      <div ref={box} className="overflow-y-auto scrollbar-thin" style={{ maxHeight }}>
        {changes.map((change, index) => (
          <FileDiff
            key={change.path}
            change={change}
            sideways={sideways}
            dark={dark}
            startOpen={index < OPEN_FILES}
            selection={selection}
            onSelect={onSelect}
            language={language}
          />
        ))}
      </div>
    </div>
  );
}

/** Lines a file view renders at once, around the selection. */
export const FILE_WINDOW_LINES = 1_500;

export interface CodeFileViewProps {
  path: string;
  text: string;
  language?: string;
  /** Number of the text's first line (a snippet of a file). */
  firstLine?: number;
  label: string;
  selection?: LineSelection | null;
  onSelect?: (selection: LineSelection) => void;
  maxHeight?: number;
  actions?: React.ReactNode;
}

/**
 * A file (or a part of one) with line numbers, the selection highlighted and scrolled
 * to. Long files show a window of lines around the selection.
 */
export function CodeFileView({ path, text, language, firstLine = 1, label, selection = null, onSelect, maxHeight = 420, actions }: CodeFileViewProps) {
  const { theme } = useTheme();
  const dark = theme === 'dark';
  const scroller = React.useRef<HTMLDivElement>(null);
  const all = useMemo(() => textAsChange(path, text, firstLine).hunks[0]?.lines ?? [], [path, text, firstLine]);
  const focus = selection && selection.path === path ? selection.from : firstLine;
  const start = Math.max(0, Math.min(all.length - FILE_WINDOW_LINES, focus - firstLine - Math.floor(FILE_WINDOW_LINES / 3)));
  const lines = useMemo(() => all.slice(start, start + FILE_WINDOW_LINES), [all, start]);
  const lastShown = firstLine + start + lines.length - 1;
  React.useEffect(() => {
    if (!selection || selection.path !== path || !scroller.current) return;
    const row = selection.from - (firstLine + start);
    scroller.current.scrollTop = Math.max(0, row * 20 - 60);
  }, [selection, path, firstLine, start]);
  const onLine = onSelect
    ? (line: number, extend: boolean) => {
        const current = selection && selection.path === path ? selection : null;
        onSelect(
          extend && current
            ? { path, from: Math.min(current.from, line), to: Math.max(current.to, line) }
            : { path, from: line, to: line },
        );
      }
    : undefined;
  return (
    <div role="group" aria-label={label} className="rounded-lg border border-shodh-border-subtle bg-shodh-surface overflow-hidden">
      <div className="flex items-center gap-2 px-2 py-1 border-b border-shodh-border-subtle bg-shodh-raised">
        <p className="m-0 min-w-0 truncate font-mono text-caption text-shodh-text-secondary">{path}</p>
        {all.length > lines.length && (
          <p className="m-0 shrink-0 text-caption text-shodh-text-muted">{`Lines ${firstLine + start}–${lastShown} of ${all.length}`}</p>
        )}
        <div className="ml-auto flex items-center gap-1">{actions}</div>
      </div>
      <div ref={scroller} className="overflow-y-auto scrollbar-thin" style={{ maxHeight }}>
        <CodeColumn path={path} lines={lines} numbers="new" language={language ?? languageOf(path)} dark={dark} selection={selection} onLine={onLine} />
      </div>
    </div>
  );
}

export default CodeDiff;
