import React, { useEffect, useRef, useState } from 'react';
import { AlertTriangle, ArrowDown, ArrowLeft, ArrowRight, ArrowUp, ArrowUpDown, Loader2, MoreHorizontal, Pencil, Trash2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import { NOT_FOUND_TEXT, splitError } from './format';
import { cellKey, cellMark, isStale, KIND_LABELS, nextCell, valueText } from './model';
import type { GridPos, SortDir } from './model';
import type { Cell, Column, RowView } from './types';
import { FOCUS_RING, MARK_CLASSES, MARK_LABELS } from './ui';

export interface SortState {
  /** Null: by the paper's title. */
  columnId: string | null;
  dir: SortDir;
}

const CELL_FOCUS = 'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring';

function ColumnMenu({
  column,
  index,
  count,
  onEdit,
  onMove,
  onDelete,
}: {
  column: Column;
  index: number;
  count: number;
  onEdit: (column: Column) => void;
  onMove: (column: Column, position: number) => void;
  onDelete: (column: Column) => void;
}) {
  const [open, setOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return undefined;
    const close = (e: PointerEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('pointerdown', close);
    return () => document.removeEventListener('pointerdown', close);
  }, [open]);

  const item = 'w-full flex items-center gap-2 px-3 py-1.5 text-[12.5px] text-left hover:bg-shodh-raised-2 focus-visible:bg-shodh-raised-2 focus-visible:outline-none disabled:opacity-50';
  const run = (action: () => void) => {
    setOpen(false);
    buttonRef.current?.focus();
    action();
  };

  return (
    <div ref={menuRef} className="relative shrink-0">
      <button
        ref={buttonRef}
        type="button"
        aria-label={`Options of the column “${column.question}”`}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen(o => !o)}
        className={cn('w-6 h-6 inline-flex items-center justify-center rounded text-shodh-text-muted hover:bg-shodh-raised-2 hover:text-shodh-text', FOCUS_RING)}
      >
        <MoreHorizontal className="w-3.5 h-3.5" aria-hidden="true" />
      </button>
      {open && (
        <div
          role="menu"
          aria-label={`Options of the column “${column.question}”`}
          className="shell-pop absolute right-0 top-7 z-40 min-w-[170px] py-1 rounded-lg border border-shodh-border-strong bg-shodh-raised shadow-lg font-normal"
          onKeyDown={e => {
            const items = Array.from(e.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)'));
            const at = items.indexOf(document.activeElement as HTMLButtonElement);
            if (e.key === 'Escape') {
              e.preventDefault();
              e.stopPropagation();
              setOpen(false);
              buttonRef.current?.focus();
            } else if (e.key === 'ArrowDown') {
              e.preventDefault();
              items[(at + 1) % items.length]?.focus();
            } else if (e.key === 'ArrowUp') {
              e.preventDefault();
              items[(at - 1 + items.length) % items.length]?.focus();
            } else if (e.key === 'Tab') {
              setOpen(false);
            }
          }}
        >
          <button type="button" role="menuitem" className={cn(item, 'text-shodh-text-secondary')} onClick={() => run(() => onEdit(column))} autoFocus>
            <Pencil className="w-3.5 h-3.5" aria-hidden="true" /> Edit column…
          </button>
          <button type="button" role="menuitem" className={cn(item, 'text-shodh-text-secondary')} disabled={index === 0} onClick={() => run(() => onMove(column, index - 1))}>
            <ArrowLeft className="w-3.5 h-3.5" aria-hidden="true" /> Move left
          </button>
          <button type="button" role="menuitem" className={cn(item, 'text-shodh-text-secondary')} disabled={index >= count - 1} onClick={() => run(() => onMove(column, index + 1))}>
            <ArrowRight className="w-3.5 h-3.5" aria-hidden="true" /> Move right
          </button>
          <div role="separator" className="my-1 border-t border-shodh-border" />
          <button type="button" role="menuitem" className={cn(item, 'text-shodh-error')} onClick={() => run(() => onDelete(column))}>
            <Trash2 className="w-3.5 h-3.5" aria-hidden="true" /> Delete column…
          </button>
        </div>
      )}
    </div>
  );
}

function CellContent({ cell, column }: { cell: Cell | undefined; column: Column }) {
  const status = cell?.status ?? 'empty';
  if (status === 'queued') {
    return (
      <span className="inline-flex items-center gap-1.5 text-shodh-text-faint">
        <Loader2 className="w-3 h-3 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        <span className="text-[12px]">Waiting</span>
      </span>
    );
  }
  if (status === 'empty' || !cell) {
    return <span className="text-shodh-text-faint" aria-label="Not filled yet">—</span>;
  }
  const mark = cellMark(cell);
  const stale = isStale(cell, column);
  const refreshFailed = cell.error ? splitError(cell.error).refillFailed : false;
  const text = status === 'not_found' ? NOT_FOUND_TEXT : status === 'failed' ? '' : valueText(cell.value);
  return (
    <span className={cn('flex flex-col gap-1 min-w-0', stale && 'opacity-55')}>
      {text && (
        <span className={cn('line-clamp-3 break-words', status === 'not_found' ? 'text-[12px] italic text-shodh-text-muted' : 'text-shodh-text')}>{text}</span>
      )}
      <span className="flex flex-wrap items-center gap-1">
        {mark && mark !== 'not_found' && (
          <span className={cn('px-1 py-px rounded text-[10.5px] font-medium', MARK_CLASSES[mark])}>{MARK_LABELS[mark]}</span>
        )}
        {stale && <span className="px-1 py-px rounded text-[10.5px] font-medium bg-shodh-raised-2 text-shodh-text-muted">Out of date</span>}
        {refreshFailed && (
          <span className="inline-flex items-center gap-0.5 px-1 py-px rounded text-[10.5px] font-medium bg-shodh-warning/15 text-shodh-warning">
            <AlertTriangle className="w-2.5 h-2.5" aria-hidden="true" /> Couldn’t refresh
          </span>
        )}
      </span>
    </span>
  );
}

/**
 * The table as a real `<table>`: a row per paper (its title is the row header), a column
 * per question. One cell is in the tab order at a time; arrow keys, Home/End (with Ctrl:
 * the grid's corners) and PageUp/PageDown move it, Enter or Space opens it in the panel.
 * Out-of-date cells are dimmed; marks say unverified, edited, failed and couldn't refresh.
 */
export function TableGrid({
  caption,
  columns,
  rows,
  cells,
  active,
  selected,
  sort,
  onActivate,
  onSelect,
  onSort,
  onEditColumn,
  onMoveColumn,
  onDeleteColumn,
}: {
  caption: string;
  columns: readonly Column[];
  rows: readonly RowView[];
  cells: ReadonlyMap<string, Cell>;
  active: GridPos;
  selected: GridPos | null;
  sort: SortState | null;
  onActivate: (pos: GridPos) => void;
  onSelect: (pos: GridPos) => void;
  onSort: (columnId: string | null) => void;
  onEditColumn: (column: Column) => void;
  onMoveColumn: (column: Column, position: number) => void;
  onDeleteColumn: (column: Column) => void;
}) {
  const tableRef = useRef<HTMLTableElement>(null);
  const focusPending = useRef(false);
  const dims = { rows: rows.length, cols: columns.length + 1 };
  const pos: GridPos = {
    row: Math.min(Math.max(active.row, 0), Math.max(rows.length - 1, 0)),
    col: Math.min(Math.max(active.col, 0), columns.length),
  };

  // Move the focus to the active cell after a key moved it (not on every render).
  useEffect(() => {
    if (!focusPending.current) return;
    focusPending.current = false;
    tableRef.current?.querySelector<HTMLElement>(`[data-cell="${pos.row}:${pos.col}"]`)?.focus();
  }, [pos.row, pos.col]);

  const onKeyDown = (e: React.KeyboardEvent, at: GridPos) => {
    if (e.key === 'Enter' || e.key === ' ' || e.key === 'F2') {
      e.preventDefault();
      onSelect(at);
      return;
    }
    const next = nextCell(at, e.key, dims, e.ctrlKey || e.metaKey);
    if (!next) return;
    e.preventDefault();
    focusPending.current = true;
    onActivate(next);
    if (next.row === at.row && next.col === at.col) tableRef.current?.querySelector<HTMLElement>(`[data-cell="${at.row}:${at.col}"]`)?.focus();
  };

  const sortIcon = (columnId: string | null) => {
    if (!sort || sort.columnId !== columnId) return <ArrowUpDown className="w-3 h-3 opacity-40" aria-hidden="true" />;
    return sort.dir === 'asc' ? <ArrowUp className="w-3 h-3" aria-hidden="true" /> : <ArrowDown className="w-3 h-3" aria-hidden="true" />;
  };
  const ariaSort = (columnId: string | null): 'ascending' | 'descending' | 'none' =>
    sort && sort.columnId === columnId ? (sort.dir === 'asc' ? 'ascending' : 'descending') : 'none';

  const cellClass = (at: GridPos) =>
    cn(
      'align-top px-3 py-2 border-b border-r border-shodh-border-subtle text-[12.5px] cursor-pointer',
      selected && selected.row === at.row && selected.col === at.col ? 'bg-shodh-accent-soft' : 'hover:bg-shodh-raised/60',
      CELL_FOCUS,
    );

  return (
    <div className="overflow-x-auto scrollbar-thin rounded-xl border border-shodh-border">
      <table ref={tableRef} className="w-max min-w-full border-collapse text-left">
        <caption className="sr-only">{caption}</caption>
        <thead className="bg-shodh-raised">
          <tr>
            <th scope="col" aria-sort={ariaSort(null)} className="sticky left-0 z-10 bg-shodh-raised px-3 py-2 border-b border-r border-shodh-border text-[12px] font-semibold text-shodh-text-secondary min-w-[200px] max-w-[260px]">
              <button type="button" onClick={() => onSort(null)} className={cn('inline-flex items-center gap-1 rounded', FOCUS_RING)}>
                Paper {sortIcon(null)}
              </button>
            </th>
            {columns.map((column, index) => (
              <th key={column.id} scope="col" aria-sort={ariaSort(column.id)} className="align-top px-3 py-2 border-b border-r border-shodh-border text-[12px] font-semibold text-shodh-text-secondary min-w-[180px] max-w-[280px]">
                <div className="flex items-start gap-1">
                  <button
                    type="button"
                    onClick={() => onSort(column.id)}
                    title={`${column.question} (${KIND_LABELS[column.kind]}). Sort by this column.`}
                    className={cn('flex-1 min-w-0 inline-flex items-start gap-1 text-left rounded', FOCUS_RING)}
                  >
                    <span className="line-clamp-2 break-words">{column.question}</span>
                    <span className="mt-0.5 shrink-0">{sortIcon(column.id)}</span>
                  </button>
                  <ColumnMenu column={column} index={index} count={columns.length} onEdit={onEditColumn} onMove={onMoveColumn} onDelete={onDeleteColumn} />
                </div>
                <span className="block mt-0.5 text-[10.5px] font-normal text-shodh-text-faint">{KIND_LABELS[column.kind]}</span>
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, r) => {
            const head: GridPos = { row: r, col: 0 };
            return (
              <tr key={row.id}>
                <th
                  scope="row"
                  data-cell={`${r}:0`}
                  tabIndex={pos.row === r && pos.col === 0 ? 0 : -1}
                  onFocus={() => onActivate(head)}
                  onClick={() => onSelect(head)}
                  onKeyDown={e => onKeyDown(e, head)}
                  className={cn('sticky left-0 z-[1] bg-shodh-surface font-medium text-shodh-text min-w-[200px] max-w-[260px]', cellClass(head))}
                >
                  <span className="line-clamp-3 break-words" title={row.documentId}>{row.title}</span>
                </th>
                {columns.map((column, c) => {
                  const at: GridPos = { row: r, col: c + 1 };
                  const cell = cells.get(cellKey(row.id, column.id));
                  return (
                    <td
                      key={column.id}
                      data-cell={`${r}:${c + 1}`}
                      tabIndex={pos.row === r && pos.col === c + 1 ? 0 : -1}
                      onFocus={() => onActivate(at)}
                      onClick={() => onSelect(at)}
                      onKeyDown={e => onKeyDown(e, at)}
                      className={cn(cellClass(at), 'min-w-[180px] max-w-[280px]')}
                    >
                      <CellContent cell={cell} column={column} />
                    </td>
                  );
                })}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
