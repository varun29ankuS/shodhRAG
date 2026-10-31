import React, { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
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
const MENU_WIDTH = 190;

/** An open column menu: the column, its place, and the button it hangs from. */
interface OpenMenu {
  column: Column;
  index: number;
  anchor: DOMRect;
}

/**
 * A column's options (edit, move, delete), drawn in a portal at fixed coordinates so the
 * grid's scroll box never clips it. It closes on Escape, a click outside, Tab, scrolling or
 * resizing, and gives the focus back to the column's header.
 */
function ColumnMenu({
  menu,
  count,
  onClose,
  onEdit,
  onMove,
  onDelete,
}: {
  menu: OpenMenu;
  count: number;
  onClose: () => void;
  onEdit: (column: Column) => void;
  onMove: (column: Column, position: number) => void;
  onDelete: (column: Column) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const { column, index, anchor } = menu;

  useEffect(() => {
    const outside = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const away = () => onClose();
    document.addEventListener('pointerdown', outside);
    window.addEventListener('resize', away);
    window.addEventListener('scroll', away, true);
    return () => {
      document.removeEventListener('pointerdown', outside);
      window.removeEventListener('resize', away);
      window.removeEventListener('scroll', away, true);
    };
  }, [onClose]);

  useLayoutEffect(() => {
    ref.current?.querySelector<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')?.focus({ preventScroll: true });
  }, []);

  const item = 'w-full flex items-center gap-2 px-3 py-1.5 text-small text-left hover:bg-shodh-raised-2 focus-visible:bg-shodh-raised-2 focus-visible:outline-none disabled:opacity-50';
  const run = (action: () => void) => {
    onClose();
    action();
  };
  const left = Math.max(8, Math.min(anchor.right - MENU_WIDTH, window.innerWidth - MENU_WIDTH - 8));
  const top = Math.min(anchor.bottom + 4, window.innerHeight - 180);

  return createPortal(
    <div
      ref={ref}
      role="menu"
      aria-label={`Options of the column “${column.question}”`}
      style={{ position: 'fixed', top, left, width: MENU_WIDTH }}
      className="shell-pop z-[60] py-1 rounded-lg border border-shodh-border-strong bg-shodh-raised text-shodh-text shadow-lg"
      onKeyDown={e => {
        const items = Array.from(e.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)'));
        const at = items.indexOf(document.activeElement as HTMLButtonElement);
        if (e.key === 'Escape' || e.key === 'Tab') {
          e.preventDefault();
          e.stopPropagation();
          onClose();
        } else if (e.key === 'ArrowDown') {
          e.preventDefault();
          items[(at + 1) % items.length]?.focus();
        } else if (e.key === 'ArrowUp') {
          e.preventDefault();
          items[(at - 1 + items.length) % items.length]?.focus();
        }
      }}
    >
      <button type="button" role="menuitem" className={cn(item, 'text-shodh-text-secondary')} onClick={() => run(() => onEdit(column))}>
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
    </div>,
    document.body,
  );
}

function CellContent({ cell, column }: { cell: Cell | undefined; column: Column }) {
  const status = cell?.status ?? 'empty';
  if (status === 'queued') {
    return (
      <span className="inline-flex items-center gap-1.5 text-shodh-text-faint">
        <Loader2 className="w-3 h-3 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        <span className="text-caption">Waiting</span>
      </span>
    );
  }
  if (status === 'empty' || !cell) {
    return (
      <>
        <span className="text-shodh-text-faint" aria-hidden="true">—</span>
        <span className="sr-only">Not filled yet</span>
      </>
    );
  }
  const mark = cellMark(cell);
  const stale = isStale(cell, column);
  const refreshFailed = cell.error ? splitError(cell.error).refillFailed : false;
  const text = status === 'not_found' ? NOT_FOUND_TEXT : status === 'failed' ? '' : valueText(cell.value);
  return (
    <span className={cn('flex flex-col gap-1 min-w-0', stale && 'opacity-55')}>
      {text && (
        <span className={cn('line-clamp-3 break-words', status === 'not_found' ? 'text-caption italic text-shodh-text-muted' : 'text-shodh-text')}>{text}</span>
      )}
      <span className="flex flex-wrap items-center gap-1">
        {mark && mark !== 'not_found' && (
          <span className={cn('px-1 py-px rounded text-caption font-medium', MARK_CLASSES[mark])}>{MARK_LABELS[mark]}</span>
        )}
        {stale && <span className="px-1 py-px rounded text-caption font-medium bg-shodh-raised-2 text-shodh-text-muted">Out of date</span>}
        {refreshFailed && (
          <span className="inline-flex items-center gap-0.5 px-1 py-px rounded text-caption font-medium bg-shodh-warning/15 text-shodh-warning">
            <AlertTriangle className="w-2.5 h-2.5" aria-hidden="true" /> Couldn’t refresh
          </span>
        )}
      </span>
    </span>
  );
}

/**
 * The table as a real `<table role="grid">`: a header row of questions, then a row per
 * paper (its title is the row header). The grid is one tab stop: arrow keys, Home/End
 * (with Ctrl: the corners) and PageUp/PageDown move through the header row and the cells.
 * On a header, Enter or Space sorts and Shift+F10 or the menu key opens the column's
 * options; on a cell, Enter, Space or F2 opens it in the panel. `active` is in grid
 * coordinates (row 0 is the header row); `selected` and `onSelect` are in body coordinates
 * (row 0 is the first paper).
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
  const helpId = useId();
  const tableRef = useRef<HTMLTableElement>(null);
  const focusPending = useRef(false);
  const [menu, setMenu] = useState<OpenMenu | null>(null);
  const dims = { rows: rows.length + 1, cols: columns.length + 1 };
  const pos: GridPos = {
    row: Math.min(Math.max(active.row, 0), dims.rows - 1),
    col: Math.min(Math.max(active.col, 0), dims.cols - 1),
  };

  const cellAt = (at: GridPos) => tableRef.current?.querySelector<HTMLElement>(`[data-cell="${at.row}:${at.col}"]`) ?? null;

  // Move the focus to the active cell after a key moved it, and only while the focus is
  // still in the grid: a filter or a panel never has its focus taken.
  useEffect(() => {
    if (!focusPending.current) return;
    focusPending.current = false;
    if (!tableRef.current?.contains(document.activeElement)) return;
    cellAt(pos)?.focus();
  }, [pos.row, pos.col]);

  const openMenu = (column: Column, index: number, from: HTMLElement | null) => {
    const anchor = (from?.querySelector<HTMLElement>('[data-menu-button]') ?? from)?.getBoundingClientRect();
    if (anchor) setMenu({ column, index, anchor });
  };

  const closeMenu = () => {
    const open = menu;
    setMenu(null);
    if (open) cellAt({ row: 0, col: open.index + 1 })?.focus();
  };

  const move = (e: React.KeyboardEvent, at: GridPos) => {
    const next = nextCell(at, e.key, dims, e.ctrlKey || e.metaKey);
    if (!next) return false;
    e.preventDefault();
    if (next.row === at.row && next.col === at.col) return true;
    focusPending.current = true;
    onActivate(next);
    return true;
  };

  const onBodyKey = (e: React.KeyboardEvent, at: GridPos) => {
    if (e.key === 'Enter' || e.key === ' ' || e.key === 'F2') {
      e.preventDefault();
      onSelect({ row: at.row - 1, col: at.col });
      return;
    }
    move(e, at);
  };

  const onHeaderKey = (e: React.KeyboardEvent, at: GridPos, column: Column | null, index: number) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onSort(column ? column.id : null);
      return;
    }
    if (column && (e.key === 'ContextMenu' || (e.key === 'F10' && e.shiftKey))) {
      e.preventDefault();
      openMenu(column, index, e.currentTarget as HTMLElement);
      return;
    }
    move(e, at);
  };

  const sortIcon = (columnId: string | null) => {
    if (!sort || sort.columnId !== columnId) return <ArrowUpDown className="w-3 h-3 opacity-40" aria-hidden="true" />;
    return sort.dir === 'asc' ? <ArrowUp className="w-3 h-3" aria-hidden="true" /> : <ArrowDown className="w-3 h-3" aria-hidden="true" />;
  };
  const ariaSort = (columnId: string | null): 'ascending' | 'descending' | 'none' =>
    sort && sort.columnId === columnId ? (sort.dir === 'asc' ? 'ascending' : 'descending') : 'none';
  const tabIndexAt = (at: GridPos) => (pos.row === at.row && pos.col === at.col ? 0 : -1);

  const cellClass = (body: GridPos) =>
    cn(
      'align-top px-3 py-2 border-b border-r border-shodh-border-subtle text-small cursor-pointer',
      selected && selected.row === body.row && selected.col === body.col ? 'bg-shodh-accent-soft' : 'hover:bg-shodh-raised/60',
      CELL_FOCUS,
    );
  const headClass = 'align-top px-3 py-2 border-b border-r border-shodh-border text-caption font-semibold text-shodh-text-secondary';

  return (
    <div className="overflow-x-auto scrollbar-thin rounded-xl border border-shodh-border">
      <p id={helpId} className="sr-only">
        Arrow keys move between the column headers and the cells. On a column header, Enter sorts by it and Shift+F10
        or the menu key opens its options. On a cell, Enter opens it beside the table.
      </p>
      <table ref={tableRef} role="grid" aria-describedby={helpId} className="w-max min-w-full border-collapse text-left">
        <caption className="sr-only">{caption}</caption>
        <thead className="bg-shodh-raised">
          <tr>
            <th
              scope="col"
              aria-sort={ariaSort(null)}
              data-cell="0:0"
              tabIndex={tabIndexAt({ row: 0, col: 0 })}
              onFocus={() => onActivate({ row: 0, col: 0 })}
              onKeyDown={e => onHeaderKey(e, { row: 0, col: 0 }, null, -1)}
              className={cn(headClass, 'sticky left-0 z-10 bg-shodh-raised min-w-[200px] max-w-[260px]', CELL_FOCUS)}
            >
              <button type="button" tabIndex={-1} onClick={() => onSort(null)} className={cn('inline-flex items-center gap-1 rounded', FOCUS_RING)}>
                Paper {sortIcon(null)}
              </button>
            </th>
            {columns.map((column, index) => {
              const at: GridPos = { row: 0, col: index + 1 };
              return (
                <th
                  key={column.id}
                  scope="col"
                  aria-sort={ariaSort(column.id)}
                  data-cell={`0:${index + 1}`}
                  tabIndex={tabIndexAt(at)}
                  onFocus={() => onActivate(at)}
                  onKeyDown={e => onHeaderKey(e, at, column, index)}
                  className={cn(headClass, 'min-w-[180px] max-w-[280px]', CELL_FOCUS)}
                >
                  <div className="flex items-start gap-1">
                    <button
                      type="button"
                      tabIndex={-1}
                      onClick={() => onSort(column.id)}
                      title={`${column.question} (${KIND_LABELS[column.kind]}). Sort by this column.`}
                      className={cn('flex-1 min-w-0 inline-flex items-start gap-1 text-left rounded', FOCUS_RING)}
                    >
                      <span className="line-clamp-2 break-words">{column.question}</span>
                      <span className="mt-0.5 shrink-0">{sortIcon(column.id)}</span>
                    </button>
                    <button
                      type="button"
                      tabIndex={-1}
                      data-menu-button
                      aria-label={`Options of the column “${column.question}”`}
                      aria-haspopup="menu"
                      aria-expanded={menu?.column.id === column.id}
                      onClick={e => openMenu(column, index, e.currentTarget)}
                      className={cn('shrink-0 w-6 h-6 inline-flex items-center justify-center rounded text-shodh-text-muted hover:bg-shodh-raised-2 hover:text-shodh-text', FOCUS_RING)}
                    >
                      <MoreHorizontal className="w-3.5 h-3.5" aria-hidden="true" />
                    </button>
                  </div>
                  <span className="block mt-0.5 text-caption font-normal text-shodh-text-faint">{KIND_LABELS[column.kind]}</span>
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, r) => {
            const head: GridPos = { row: r + 1, col: 0 };
            const headBody: GridPos = { row: r, col: 0 };
            return (
              <tr key={row.id}>
                <th
                  scope="row"
                  data-cell={`${r + 1}:0`}
                  tabIndex={tabIndexAt(head)}
                  onFocus={() => onActivate(head)}
                  onClick={() => onSelect(headBody)}
                  onKeyDown={e => onBodyKey(e, head)}
                  className={cn('sticky left-0 z-[1] bg-shodh-surface font-medium text-shodh-text min-w-[200px] max-w-[260px]', cellClass(headBody))}
                >
                  <span className="line-clamp-3 break-words" title={row.documentId}>{row.title}</span>
                </th>
                {columns.map((column, c) => {
                  const at: GridPos = { row: r + 1, col: c + 1 };
                  const body: GridPos = { row: r, col: c + 1 };
                  const cell = cells.get(cellKey(row.id, column.id));
                  return (
                    <td
                      key={column.id}
                      data-cell={`${r + 1}:${c + 1}`}
                      tabIndex={tabIndexAt(at)}
                      onFocus={() => onActivate(at)}
                      onClick={() => onSelect(body)}
                      onKeyDown={e => onBodyKey(e, at)}
                      className={cn(cellClass(body), 'min-w-[180px] max-w-[280px]')}
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
      {menu && (
        <ColumnMenu
          menu={menu}
          count={columns.length}
          onClose={closeMenu}
          onEdit={onEditColumn}
          onMove={onMoveColumn}
          onDelete={onDeleteColumn}
        />
      )}
    </div>
  );
}
