import React, { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { ask, save } from '@tauri-apps/plugin-dialog';
import { writeTextFile } from '@tauri-apps/plugin-fs';
import { ArrowLeft, Columns3, Download, FilePlus2, Loader2, MoreHorizontal, Pencil, Play, Plus, Square, Table2, Trash2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { onAppSettingsChanged } from '../../lib/appSettings';
import { relativeTime } from '../../utils/time';
import type { WorkspaceDetail } from '../workspaces/types';
import { onTableChanged, tableError, tablesApi } from './api';
import { AddPapersDialog } from './AddPapersDialog';
import { CellPanel } from './CellPanel';
import { ColumnDialog } from './ColumnDialog';
import { FillDialog, ModelProblem } from './FillDialog';
import { callsLine, cancelOutcome, cancelPending, fillStatusText, jobActive, reasonText } from './format';
import { cellIndex, cellKey, cellValue, filterRows, filterValue, MAX_COLUMNS, MAX_NAME_CHARS, sortRows } from './model';
import type { GridPos, RowFilter } from './model';
import { NewTableDialog } from './NewTableDialog';
import { TableGrid } from './TableGrid';
import type { SortState } from './TableGrid';
import type { CellStatus, Column, EvidenceTable, TableFailure, TableView } from './types';
import { useFillStatus } from './useFillStatus';
import { FOCUS_RING, INPUT, openSettings, OUTLINE_BUTTON, PRIMARY_BUTTON, QUIET_BUTTON, SECTION_TITLE, TONE_CLASSES } from './ui';

/** The "show papers with" choices of the status filter. */
const STATUS_FILTERS: readonly { id: string; label: string; statuses: readonly CellStatus[] }[] = [
  { id: 'all', label: 'All papers', statuses: [] },
  { id: 'unverified', label: 'With unverified answers', statuses: ['unverified'] },
  { id: 'not_found', label: 'With answers not found', statuses: ['not_found'] },
  { id: 'failed', label: 'With failed cells', statuses: ['failed'] },
  { id: 'edited', label: 'With your edits', statuses: ['edited'] },
  { id: 'empty', label: 'With empty cells', statuses: ['empty'] },
];

/** The fill's pause reasons the user fixes in Settings → Model. */
const MODEL_PAUSES: readonly string[] = ['auth', 'model_unavailable', 'failing', 'unreachable'];

function fileStem(name: string): string {
  const stem = name.replace(/[<>:"/\\|?*\u0000-\u001f]+/g, ' ').replace(/\s+/g, ' ').trim();
  return stem || 'table';
}

function localDate(at: Date): string {
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}`;
}

/** The fill's status line, the day's calls, and the ways out of a pause. */
function FillStatusLine({
  tableId,
  onProgress,
  onFill,
}: {
  tableId: string;
  onProgress: () => void;
  onFill: (resume: boolean) => void;
}) {
  const { status, error, refresh } = useFillStatus(tableId);
  const [cancelling, setCancelling] = useState(false);
  const job = status?.job ?? null;

  // New cells as the fill walks the papers.
  const progressKey = job ? `${job.id}:${job.state}:${job.doneCalls}:${status?.papersDone ?? 0}` : '';
  const firstKey = useRef(true);
  useEffect(() => {
    if (firstKey.current) {
      firstKey.current = false;
      return;
    }
    onProgress();
  }, [progressKey, onProgress]);

  const cancel = async () => {
    setCancelling(true);
    try {
      const result = await tablesApi.cancel(tableId);
      const outcome = cancelOutcome(result, Date.now());
      if (outcome.tone === 'ok') notify.success(outcome.text);
      else notify.info(outcome.text);
      await refresh();
    } catch (err) {
      const failure = tableError(err);
      notify.error('The fill was not cancelled', { description: failure.message || reasonText(failure.code) });
    } finally {
      setCancelling(false);
    }
  };

  if (!status) {
    return error ? <p role="alert" className="m-0 text-[12.5px] text-shodh-error">The fill status could not be read: {error}</p> : null;
  }
  const now = Date.now();
  const line = fillStatusText(status, now);
  const active = jobActive(job);
  const pending = cancelPending(job);

  return (
    <div className="flex flex-col gap-1.5 rounded-lg border border-shodh-border-subtle bg-shodh-raised/50 px-3 py-2">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        {line && (
          <p role="status" aria-live="polite" className={cn('m-0 flex-1 min-w-[240px] text-[12.5px]', TONE_CLASSES[line.tone])}>
            {job?.state === 'running' && !pending && <Loader2 className="inline w-3.5 h-3.5 mr-1.5 -mt-0.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
            {line.text}
          </p>
        )}
        {job?.state === 'paused' && job.pauseReason && MODEL_PAUSES.includes(job.pauseReason) && (
          <button type="button" className={OUTLINE_BUTTON} onClick={() => openSettings('models')}>Open Settings → Model</button>
        )}
        {job?.state === 'paused' && job.pauseReason === 'daily_cap' && (
          <button type="button" className={OUTLINE_BUTTON} onClick={() => openSettings('tables')}>Settings → Tables</button>
        )}
        {job?.state === 'paused' && (
          <button type="button" className={OUTLINE_BUTTON} onClick={() => onFill(true)}>
            <Play className="w-3.5 h-3.5" aria-hidden="true" /> Try now…
          </button>
        )}
        {active && (
          <button type="button" className={QUIET_BUTTON} disabled={cancelling || pending} onClick={() => void cancel()}>
            {cancelling || pending ? <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <Square className="w-3.5 h-3.5" aria-hidden="true" />}
            {pending ? 'Cancelling…' : 'Cancel fill'}
          </button>
        )}
      </div>
      <p className="m-0 text-[12px] text-shodh-text-muted">{callsLine(status.callsToday, status.cap, status.dayResetsAt, now)}</p>
    </div>
  );
}

/** One table: its toolbar, fill status, filters, grid and the selected cell's panel. */
function TableEditor({
  workspace,
  tableId,
  diagnostics,
  onBack,
}: {
  workspace: WorkspaceDetail;
  tableId: string;
  diagnostics: boolean;
  onBack: () => void;
}) {
  const ids = { name: useId(), status: useId() };
  const [view, setView] = useState<TableView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [nameDraft, setNameDraft] = useState('');
  const [sort, setSort] = useState<SortState | null>(null);
  const [statusFilter, setStatusFilter] = useState('all');
  const [valueFilters, setValueFilters] = useState<Record<string, string>>({});
  const [active, setActive] = useState<GridPos>({ row: 0, col: 0 });
  const [selected, setSelected] = useState<{ rowId: string; columnId: string | null } | null>(null);
  const [columnDialog, setColumnDialog] = useState<{ open: boolean; column: Column | null }>({ open: false, column: null });
  const [fillOpen, setFillOpen] = useState(false);
  const [papersOpen, setPapersOpen] = useState(false);
  const [modelProblem, setModelProblem] = useState<TableFailure | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const [exporting, setExporting] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuButtonRef = useRef<HTMLButtonElement>(null);

  const load = useCallback(async () => {
    try {
      setView(await tablesApi.get(tableId));
      setLoadError(null);
    } catch (err) {
      const failure = tableError(err);
      setLoadError(failure.message || reasonText(failure.code));
    }
  }, [tableId]);

  useEffect(() => {
    setView(null);
    void load();
  }, [load]);

  useEffect(
    () => onTableChanged(id => {
      if (id === null || id === tableId) void load();
    }),
    [load, tableId],
  );

  useEffect(() => {
    if (!menuOpen) return undefined;
    const close = (e: PointerEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setMenuOpen(false);
    };
    document.addEventListener('pointerdown', close);
    return () => document.removeEventListener('pointerdown', close);
  }, [menuOpen]);

  const cells = useMemo(() => cellIndex(view?.cells ?? []), [view]);
  const columns = view?.columns ?? [];
  const sortedColumns = useMemo(() => [...columns].sort((a, b) => a.position - b.position), [columns]);
  const filters = useMemo<RowFilter[]>(() => {
    const list: RowFilter[] = [];
    const chosen = STATUS_FILTERS.find(f => f.id === statusFilter);
    if (chosen && chosen.statuses.length > 0) list.push({ kind: 'status', statuses: chosen.statuses });
    for (const [columnId, value] of Object.entries(valueFilters)) {
      if (value) list.push({ kind: 'value', columnId, values: [value] });
    }
    return list;
  }, [statusFilter, valueFilters]);
  const rows = useMemo(() => {
    if (!view) return [];
    const ordered = [...view.rows].sort((a, b) => a.position - b.position);
    const kept = filterRows(ordered, cells, sortedColumns, filters);
    if (!sort) return kept;
    const column = sort.columnId === null ? null : sortedColumns.find(c => c.id === sort.columnId) ?? null;
    if (sort.columnId !== null && column === null) return kept;
    return sortRows(kept, cells, column, sort.dir);
  }, [view, cells, sortedColumns, filters, sort]);

  const filterColumns = sortedColumns.filter(c => c.kind === 'yes_no' || c.kind === 'choice');

  if (loadError && !view) {
    return (
      <div className="flex flex-col gap-3">
        <button type="button" className={cn(QUIET_BUTTON, 'self-start -ml-2')} onClick={onBack}>
          <ArrowLeft className="w-3.5 h-3.5" aria-hidden="true" /> All tables
        </button>
        <p role="alert" className="m-0 text-[13px] text-shodh-error">This table could not be opened: {loadError}</p>
      </div>
    );
  }
  if (!view) return <p role="status" className="m-0 text-[13px] text-shodh-text-muted">Opening the table…</p>;

  const table = view.table;
  const selectedRow = selected ? view.rows.find(r => r.id === selected.rowId) ?? null : null;
  const selectedColumn = selected && selected.columnId ? sortedColumns.find(c => c.id === selected.columnId) ?? null : null;
  const selectionValid = selectedRow !== null && (selected?.columnId === null || selectedColumn !== null);
  const selectedPos: GridPos | null = selectionValid && selected
    ? { row: rows.findIndex(r => r.id === selected.rowId), col: selected.columnId === null ? 0 : sortedColumns.findIndex(c => c.id === selected.columnId) + 1 }
    : null;

  const select = (pos: GridPos) => {
    const row = rows[pos.row];
    if (!row) return;
    setActive(pos);
    setSelected({ rowId: row.id, columnId: pos.col === 0 ? null : sortedColumns[pos.col - 1]?.id ?? null });
  };

  const toggleSort = (columnId: string | null) =>
    setSort(prev => (prev && prev.columnId === columnId ? (prev.dir === 'asc' ? { columnId, dir: 'desc' } : null) : { columnId, dir: 'asc' }));

  const rename = async () => {
    const name = nameDraft.trim();
    if (!name || name === table.name) {
      setRenaming(false);
      return;
    }
    try {
      await tablesApi.rename(table.id, name);
      setRenaming(false);
      void load();
    } catch (err) {
      notify.error('The table was not renamed', { description: tableError(err).message });
    }
  };

  const removeTable = async () => {
    setMenuOpen(false);
    const confirmed = await ask(`Delete “${table.name}”? Its columns, answers and quotes are deleted; the papers stay in your library.`, {
      title: 'Delete table',
      kind: 'warning',
      okLabel: 'Delete',
      cancelLabel: 'Keep',
    });
    if (!confirmed) return;
    try {
      await tablesApi.remove(table.id);
      notify.success(`Deleted “${table.name}”`);
      onBack();
    } catch (err) {
      notify.error('The table was not deleted', { description: tableError(err).message });
    }
  };

  const exportCaptures = async () => {
    setMenuOpen(false);
    setExporting(true);
    try {
      const captures = await tablesApi.exportCaptures(table.id);
      if (captures.length === 0) {
        notify.info('No quality checks kept yet', { description: 'Run “Run quality check on this paper” from a paper of this table first.' });
        return;
      }
      const path = await save({
        defaultPath: `${fileStem(table.name)} quality checks ${localDate(new Date())}.json`,
        filters: [{ name: 'JSON', extensions: ['json'] }],
      });
      if (!path) return;
      await writeTextFile(path, `${JSON.stringify(captures, null, 2)}\n`);
      notify.success(`Exported ${captures.length} ${captures.length === 1 ? 'quality check' : 'quality checks'}`, { description: path });
    } catch (err) {
      notify.error('Export failed', { description: tableError(err).message });
    } finally {
      setExporting(false);
    }
  };

  const moveColumn = async (column: Column, position: number) => {
    try {
      await tablesApi.moveColumn(table.id, column.id, position);
      void load();
    } catch (err) {
      notify.error('The column was not moved', { description: tableError(err).message });
    }
  };

  const deleteColumn = async (column: Column) => {
    const confirmed = await ask(`Delete the column “${column.question}”? Its answers and quotes in every paper are deleted.`, {
      title: 'Delete column',
      kind: 'warning',
      okLabel: 'Delete',
      cancelLabel: 'Keep',
    });
    if (!confirmed) return;
    try {
      await tablesApi.deleteColumn(table.id, column.id);
      void load();
    } catch (err) {
      notify.error('The column was not deleted', { description: tableError(err).message });
    }
  };

  const menuItem = 'w-full flex items-center gap-2 px-3 py-1.5 text-[12.5px] text-left hover:bg-shodh-raised-2 focus-visible:bg-shodh-raised-2 focus-visible:outline-none';
  const canFill = view.rows.length > 0 && sortedColumns.length > 0;

  return (
    <div className="flex flex-col gap-4">
      <button type="button" className={cn(QUIET_BUTTON, 'self-start -ml-2')} onClick={onBack}>
        <ArrowLeft className="w-3.5 h-3.5" aria-hidden="true" /> All tables
      </button>

      <header className="flex flex-wrap items-center gap-2">
        {renaming ? (
          <form
            className="flex items-center gap-2 flex-1 min-w-[240px]"
            onSubmit={e => {
              e.preventDefault();
              void rename();
            }}
          >
            <label htmlFor={ids.name} className="sr-only">Table name</label>
            <input
              id={ids.name}
              className={cn(INPUT, 'max-w-[420px]')}
              value={nameDraft}
              maxLength={MAX_NAME_CHARS}
              onChange={e => setNameDraft(e.target.value)}
              onKeyDown={e => { if (e.key === 'Escape') setRenaming(false); }}
              autoFocus
            />
            <button type="submit" className={OUTLINE_BUTTON}>Save</button>
            <button type="button" className={QUIET_BUTTON} onClick={() => setRenaming(false)}>Cancel</button>
          </form>
        ) : (
          <div className="flex items-center gap-1.5 flex-1 min-w-[240px]">
            <h3 className="m-0 text-[16px] font-semibold text-shodh-text truncate">{table.name}</h3>
            <button
              type="button"
              aria-label="Rename table"
              className={cn(QUIET_BUTTON, 'w-7 h-7 justify-center px-0')}
              onClick={() => {
                setNameDraft(table.name);
                setRenaming(true);
              }}
            >
              <Pencil className="w-3.5 h-3.5" aria-hidden="true" />
            </button>
          </div>
        )}
        <button type="button" className={OUTLINE_BUTTON} onClick={() => setPapersOpen(true)}>
          <FilePlus2 className="w-3.5 h-3.5" aria-hidden="true" /> Add papers
        </button>
        <button
          type="button"
          className={OUTLINE_BUTTON}
          disabled={sortedColumns.length >= MAX_COLUMNS}
          title={sortedColumns.length >= MAX_COLUMNS ? `A table has at most ${MAX_COLUMNS} columns.` : undefined}
          onClick={() => setColumnDialog({ open: true, column: null })}
        >
          <Columns3 className="w-3.5 h-3.5" aria-hidden="true" /> Add column
        </button>
        <button
          type="button"
          className={cn(PRIMARY_BUTTON, 'h-8')}
          disabled={!canFill}
          title={canFill ? undefined : 'Add papers and at least one column first.'}
          onClick={() => setFillOpen(true)}
        >
          <Play className="w-3.5 h-3.5" aria-hidden="true" /> Fill…
        </button>
        <div ref={menuRef} className="relative">
          <button
            ref={menuButtonRef}
            type="button"
            className={cn(QUIET_BUTTON, 'w-8 h-8 justify-center px-0')}
            aria-label="Table options"
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            onClick={() => setMenuOpen(o => !o)}
          >
            {exporting ? <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <MoreHorizontal className="w-4 h-4" aria-hidden="true" />}
          </button>
          {menuOpen && (
            <div
              role="menu"
              aria-label="Table options"
              className="shell-pop absolute right-0 top-9 z-50 min-w-[200px] py-1 rounded-lg border border-shodh-border-strong bg-shodh-raised shadow-lg"
              onKeyDown={e => {
                const items = Array.from(e.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'));
                const index = items.indexOf(document.activeElement as HTMLButtonElement);
                if (e.key === 'Escape') {
                  e.preventDefault();
                  setMenuOpen(false);
                  menuButtonRef.current?.focus();
                } else if (e.key === 'ArrowDown') {
                  e.preventDefault();
                  items[(index + 1) % items.length]?.focus();
                } else if (e.key === 'ArrowUp') {
                  e.preventDefault();
                  items[(index - 1 + items.length) % items.length]?.focus();
                } else if (e.key === 'Tab') {
                  setMenuOpen(false);
                }
              }}
            >
              {diagnostics && (
                <button type="button" role="menuitem" className={cn(menuItem, 'text-shodh-text-secondary')} onClick={() => void exportCaptures()} autoFocus>
                  <Download className="w-3.5 h-3.5" aria-hidden="true" /> Export captures…
                </button>
              )}
              <button type="button" role="menuitem" className={cn(menuItem, 'text-shodh-error')} onClick={() => void removeTable()} autoFocus={!diagnostics}>
                <Trash2 className="w-3.5 h-3.5" aria-hidden="true" /> Delete table…
              </button>
            </div>
          )}
        </div>
      </header>

      {modelProblem && <ModelProblem failure={modelProblem} />}

      <FillStatusLine tableId={table.id} onProgress={load} onFill={() => setFillOpen(true)} />

      {view.rows.length > 0 && sortedColumns.length > 0 && (
        <div className="flex flex-wrap items-center gap-2" role="group" aria-label="Filter papers">
          <label htmlFor={ids.status} className="text-[12px] text-shodh-text-muted">Show</label>
          <select
            id={ids.status}
            value={statusFilter}
            onChange={e => setStatusFilter(e.target.value)}
            className={cn('h-8 rounded-lg border border-shodh-border bg-shodh-surface px-2 text-[12.5px] text-shodh-text', FOCUS_RING)}
          >
            {STATUS_FILTERS.map(f => <option key={f.id} value={f.id}>{f.label}</option>)}
          </select>
          {filterColumns.map(column => {
            const values = column.kind === 'yes_no'
              ? ['yes', 'no']
              : column.options;
            const present = new Set(view.rows.map(r => filterValue(cellValue(cells.get(cellKey(r.id, column.id)), column))).filter((v): v is string => v !== null));
            return (
              <select
                key={column.id}
                aria-label={`Filter by “${column.question}”`}
                value={valueFilters[column.id] ?? ''}
                onChange={e => setValueFilters(prev => ({ ...prev, [column.id]: e.target.value }))}
                className={cn('h-8 max-w-[220px] rounded-lg border border-shodh-border bg-shodh-surface px-2 text-[12.5px] text-shodh-text', FOCUS_RING)}
              >
                <option value="">{column.question}: any</option>
                {values.map(v => (
                  <option key={v} value={v} disabled={!present.has(v)}>{`${column.question}: ${v}`}</option>
                ))}
              </select>
            );
          })}
          <span className="text-[12px] text-shodh-text-muted" aria-live="polite">
            {rows.length === view.rows.length ? `${rows.length} ${rows.length === 1 ? 'paper' : 'papers'}` : `Showing ${rows.length} of ${view.rows.length} papers`}
          </span>
        </div>
      )}

      {sortedColumns.length === 0 || view.rows.length === 0 ? (
        <div className="flex flex-col items-start gap-2 rounded-xl border border-dashed border-shodh-border px-4 py-5">
          <p className="m-0 text-[13px] text-shodh-text">
            {view.rows.length === 0 && sortedColumns.length === 0
              ? 'Add the papers to compare and the questions to ask of each.'
              : view.rows.length === 0
                ? 'Add the papers to compare: each becomes a row.'
                : 'Add a column: a question asked of every paper.'}
          </p>
          <div className="flex gap-2">
            {view.rows.length === 0 && (
              <button type="button" className={OUTLINE_BUTTON} onClick={() => setPapersOpen(true)}>
                <FilePlus2 className="w-3.5 h-3.5" aria-hidden="true" /> Add papers
              </button>
            )}
            {sortedColumns.length === 0 && (
              <button type="button" className={OUTLINE_BUTTON} onClick={() => setColumnDialog({ open: true, column: null })}>
                <Columns3 className="w-3.5 h-3.5" aria-hidden="true" /> Add column
              </button>
            )}
          </div>
        </div>
      ) : (
        <div className="flex flex-col xl:flex-row gap-4 items-start">
          <div className="flex-1 min-w-0 w-full">
            {rows.length === 0 ? (
              <p className="m-0 text-[12.5px] text-shodh-text-muted">No paper matches the filters.</p>
            ) : (
              <TableGrid
                caption={`${table.name}: ${rows.length} papers by ${sortedColumns.length} questions`}
                columns={sortedColumns}
                rows={rows}
                cells={cells}
                active={active}
                selected={selectedPos}
                sort={sort}
                onActivate={setActive}
                onSelect={select}
                onSort={toggleSort}
                onEditColumn={column => setColumnDialog({ open: true, column })}
                onMoveColumn={(column, position) => void moveColumn(column, position)}
                onDeleteColumn={column => void deleteColumn(column)}
              />
            )}
          </div>
          <aside aria-label="Selected cell" className="w-full xl:w-[340px] shrink-0 rounded-xl border border-shodh-border bg-shodh-surface p-4 xl:sticky xl:top-2">
            {selectionValid && selectedRow ? (
              <CellPanel
                key={`${selectedRow.id}:${selectedColumn?.id ?? 'paper'}`}
                tableId={table.id}
                row={selectedRow}
                column={selectedColumn}
                cell={selectedColumn ? cells.get(cellKey(selectedRow.id, selectedColumn.id)) : undefined}
                diagnostics={diagnostics}
                onChanged={() => void load()}
              />
            ) : (
              <p className="m-0 text-[12.5px] text-shodh-text-muted">
                Choose a cell to see its quote, its page in the paper and why it is marked. Arrow keys move between
                cells; Enter opens one.
              </p>
            )}
          </aside>
        </div>
      )}

      <ColumnDialog
        open={columnDialog.open}
        onOpenChange={open => setColumnDialog(d => ({ ...d, open }))}
        tableId={table.id}
        column={columnDialog.column}
        onSaved={() => void load()}
      />
      <FillDialog open={fillOpen} onOpenChange={setFillOpen} tableId={table.id} onStarted={() => void load()} onModelProblem={setModelProblem} />
      <AddPapersDialog open={papersOpen} onOpenChange={setPapersOpen} tableId={table.id} workspace={workspace} rows={view.rows} onAdded={() => void load()} />
    </div>
  );
}

/**
 * A workspace's evidence tables: the list with "New table", and one table open at a time.
 * The diagnostics setting (Settings → Tables) is followed live.
 */
export function TablesTab({ workspace }: { workspace: WorkspaceDetail }) {
  const [tables, setTables] = useState<EvidenceTable[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [openId, setOpenId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [diagnostics, setDiagnostics] = useState(false);

  const load = useCallback(async () => {
    try {
      const list = await tablesApi.list(workspace.id);
      setTables([...list].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt)));
      setError(null);
    } catch (err) {
      const failure = tableError(err);
      setError(failure.message || reasonText(failure.code));
    }
  }, [workspace.id]);

  useEffect(() => {
    setTables(null);
    setOpenId(null);
    void load();
  }, [load]);

  // A table created, renamed or deleted anywhere (its own id is sent).
  useEffect(() => onTableChanged(() => void load()), [load]);

  useEffect(() => {
    let cancelled = false;
    const read = () => {
      tablesApi
        .settings()
        .then(s => { if (!cancelled) setDiagnostics(s.diagnostics); })
        .catch(() => { if (!cancelled) setDiagnostics(false); });
    };
    read();
    const unsubscribe = onAppSettingsChanged(read);
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  if (openId) {
    return <TableEditor key={openId} workspace={workspace} tableId={openId} diagnostics={diagnostics} onBack={() => { setOpenId(null); void load(); }} />;
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center gap-2">
        <h3 className={SECTION_TITLE}>
          Evidence tables {tables && <span className="font-normal text-shodh-text-muted">({tables.length})</span>}
        </h3>
        <button type="button" className={cn(PRIMARY_BUTTON, 'ml-auto h-8')} onClick={() => setCreating(true)}>
          <Plus className="w-4 h-4" aria-hidden="true" /> New table
        </button>
      </div>
      {error && <p role="alert" className="m-0 text-[12.5px] text-shodh-error">The tables could not be listed: {error}</p>}
      {tables === null && !error && <p role="status" className="m-0 text-[12.5px] text-shodh-text-muted">Loading tables…</p>}
      {tables !== null && tables.length === 0 && (
        <p className="m-0 text-[12.5px] text-shodh-text-muted">
          No tables yet. A table compares this workspace's papers question by question: the model answers each cell
          with a quote that Shodh checks against the paper, so every value can be traced to its page.
        </p>
      )}
      {tables !== null && tables.length > 0 && (
        <ul className="m-0 p-0 list-none divide-y divide-shodh-border-subtle">
          {tables.map(t => (
            <li key={t.id}>
              <button
                type="button"
                onClick={() => setOpenId(t.id)}
                className={cn('w-full flex items-center gap-3 py-2 px-1 text-left rounded-md hover:bg-shodh-raised/60', FOCUS_RING)}
              >
                <Table2 className="w-4 h-4 text-shodh-text-faint shrink-0" aria-hidden="true" />
                <span className="flex-1 min-w-0 text-[13px] text-shodh-text truncate">{t.name}</span>
                <span className="text-[11.5px] text-shodh-text-muted shrink-0">{relativeTime(t.updatedAt)}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      <NewTableDialog open={creating} onOpenChange={setCreating} workspaceId={workspace.id} onCreated={t => { void load(); setOpenId(t.id); }} />
    </div>
  );
}
