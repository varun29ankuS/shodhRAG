import React, { useId, useState } from 'react';
import { ExternalLink, Eraser, FlaskConical, Loader2, Pencil, RefreshCw, Trash2 } from 'lucide-react';
import { ask } from '@tauri-apps/plugin-dialog';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { relativeTime } from '../../utils/time';
import { showSourceBox } from '../research/snippetBus';
import { tableError, tablesApi } from './api';
import { cellExplanation, handFillOutcome, INLINE_REFUSALS, knownReason, reasonText } from './format';
import type { Outcome } from './format';
import { cellMark, cellSource, isStale, KIND_LABELS, MAX_EDIT_CHARS, quoteSegments, valueText } from './model';
import type { Cell, Column, RowView } from './types';
import { FOCUS_RING, INPUT, MARK_CLASSES, OUTLINE_BUTTON, PRIMARY_BUTTON, QUIET_BUTTON } from './ui';

type Notice = Outcome | null;

export const NOTICE_CLASSES: Record<Outcome['tone'], string> = {
  ok: 'text-shodh-success',
  info: 'text-shodh-text-secondary',
  warning: 'text-shodh-warning',
  error: 'text-shodh-error',
};

/** A refusal that is a wait (the fill runs, the paper is being filled), said inline; anything else is an error. */
export function failureNotice(err: unknown, what: string): Notice {
  const failure = tableError(err);
  if (INLINE_REFUSALS.includes(failure.code)) return { tone: 'info', text: reasonText(failure.code) };
  notify.error(what, { description: failure.message || reasonText(failure.code) });
  return null;
}

/**
 * A fill by hand (Refill, a quality check), kept by the table rather than by the panel: it
 * can take minutes, and its outcome must survive the selection moving on (see `TableEditor`).
 */
export interface HandFillState {
  busy: boolean;
  outcome: Outcome | null;
}

/** Asks the model again for one cell; the outcome is read from the cells it stored. */
export async function refillOutcome(tableId: string, rowId: string, columnId: string, columnName: (id: string) => string): Promise<Notice> {
  try {
    return handFillOutcome(await tablesApi.refillCell(tableId, rowId, columnId), columnName);
  } catch (err) {
    return failureNotice(err, 'The cell was not asked again');
  }
}

/** Fills one paper with its model calls kept (diagnostics); the outcome is read from its cells. */
export async function checkOutcome(tableId: string, rowId: string, columnName: (id: string) => string): Promise<Notice> {
  try {
    const capture = await tablesApi.fillPaper(tableId, rowId);
    const calls = capture.calls.length;
    const kept = capture.captureId ? 'kept for export' : 'returned but could not be kept';
    const callsKept = `${calls} model ${calls === 1 ? 'call was' : 'calls were'} ${kept}.`;
    if (capture.error) {
      return { tone: 'error', text: `The check stopped: ${knownReason(capture.error) ? reasonText(capture.error) : capture.error} ${callsKept}` };
    }
    if (capture.result) {
      const outcome = handFillOutcome(capture.result, columnName);
      return { tone: outcome.tone, text: `${outcome.text} ${callsKept}` };
    }
    return { tone: 'info', text: callsKept };
  } catch (err) {
    return failureNotice(err, 'The quality check did not run');
  }
}

/** The paper of a row: where it is, its quality check (with diagnostics on) and removing it. */
function PaperPanel({
  tableId,
  row,
  diagnostics,
  check,
  onCheck,
  onChanged,
}: {
  tableId: string;
  row: RowView;
  diagnostics: boolean;
  check: HandFillState | undefined;
  onCheck: () => void;
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState<'remove' | null>(null);
  const checking = check?.busy ?? false;
  const notice = check?.outcome ?? null;

  const remove = async () => {
    const confirmed = await ask(`Remove “${row.title}” from this table? Its cells go with it; the paper stays in your library.`, {
      title: 'Remove paper',
      kind: 'warning',
      okLabel: 'Remove',
      cancelLabel: 'Keep',
    });
    if (!confirmed) return;
    setBusy('remove');
    try {
      await tablesApi.removeRow(tableId, row.id);
      notify.success(`Removed “${row.title}” from the table`);
      onChanged();
    } catch (err) {
      notify.error('The paper was not removed', { description: tableError(err).message });
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="flex flex-col gap-3">
      <div>
        <p className="m-0 text-caption uppercase tracking-wide text-shodh-text-muted">Paper</p>
        <h3 className="m-0 mt-0.5 text-body font-semibold text-shodh-text break-words">{row.title}</h3>
        <p className="m-0 mt-1 text-caption text-shodh-text-muted break-all">{row.documentId}</p>
      </div>
      <div className="flex flex-wrap gap-2">
        {diagnostics && (
          <button type="button" className={OUTLINE_BUTTON} disabled={busy !== null || checking} onClick={onCheck}>
            {checking ? <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <FlaskConical className="w-3.5 h-3.5" aria-hidden="true" />}
            Run quality check on this paper
          </button>
        )}
        <button type="button" className={cn(QUIET_BUTTON, 'text-shodh-error')} disabled={busy !== null || checking} onClick={() => void remove()}>
          <Trash2 className="w-3.5 h-3.5" aria-hidden="true" /> Remove from table
        </button>
      </div>
      {diagnostics && (
        <p className="m-0 text-caption text-shodh-text-muted">
          The quality check fills this paper and keeps every model call it made (cells that are current or cached cost
          no call: clear them first for a fresh check). It counts against today's calls.
        </p>
      )}
      <p role="status" aria-live="polite" className={cn('m-0 text-small', notice ? NOTICE_CLASSES[notice.tone] : 'sr-only')}>
        {notice?.text ?? ''}
      </p>
    </div>
  );
}

/**
 * One cell: its value, the quote with the answer's words marked, the page with Open in
 * PDF (the passage's boxes outlined), the model and date, the reason for its mark, and
 * Edit, Refill and Clear. A row's paper column shows the paper instead.
 */
export function CellPanel({
  tableId,
  row,
  column,
  cell,
  diagnostics,
  refill,
  check,
  onRefill,
  onCheck,
  onChanged,
}: {
  tableId: string;
  row: RowView;
  /** Null: the paper itself (the row's first column). */
  column: Column | null;
  cell: Cell | undefined;
  diagnostics: boolean;
  /** This cell's Refill, as the table keeps it. */
  refill: HandFillState | undefined;
  /** This paper's quality check, as the table keeps it. */
  check: HandFillState | undefined;
  onRefill: () => void;
  onCheck: () => void;
  onChanged: () => void;
}) {
  const editId = useId();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState<'edit' | 'clear' | null>(null);
  const [localNotice, setNotice] = useState<Notice>(null);
  const [editError, setEditError] = useState<string | null>(null);

  if (!column) return <PaperPanel tableId={tableId} row={row} diagnostics={diagnostics} check={check} onCheck={onCheck} onChanged={onChanged} />;

  const refilling = refill?.busy ?? false;
  // This panel's own note (an edit, a clear), else the cell's last Refill outcome.
  const notice = refilling ? null : localNotice ?? refill?.outcome ?? null;

  const stale = isStale(cell, column);
  const explanation = cellExplanation(cell, stale);
  const mark = cellMark(cell);
  const value = cell?.value ?? null;
  const source = cell ? cellSource(cell, row) : null;
  const segments = cell?.quote ? quoteSegments(cell.quote, value) : [];

  const startEdit = () => {
    setDraft(valueText(value));
    setEditError(null);
    setEditing(true);
  };

  const saveEdit = async () => {
    setBusy('edit');
    setEditError(null);
    try {
      await tablesApi.editCell(tableId, row.id, column.id, draft);
      setEditing(false);
      setNotice({ tone: 'ok', text: 'Saved. A fill never overwrites your value.' });
      onChanged();
    } catch (err) {
      const failure = tableError(err);
      setEditError(failure.message || reasonText(failure.code));
    } finally {
      setBusy(null);
    }
  };

  const startRefill = () => {
    setNotice(null);
    onRefill();
  };

  const clear = async () => {
    setBusy('clear');
    setNotice(null);
    try {
      await tablesApi.clearCell(tableId, row.id, column.id);
      setNotice({ tone: 'ok', text: 'Cleared. The next fill answers it.' });
      onChanged();
    } catch (err) {
      setNotice(failureNotice(err, 'The cell was not cleared'));
    } finally {
      setBusy(null);
    }
  };

  const edited = cell?.status === 'edited';
  const badgeClass = mark
    ? MARK_CLASSES[mark]
    : cell?.status === 'verified'
      ? 'bg-shodh-success/10 text-shodh-success'
      : 'bg-shodh-raised-2 text-shodh-text-muted';
  const hasContent = cell !== undefined && cell.status !== 'empty' && cell.status !== 'queued';

  return (
    <div className="flex flex-col gap-3">
      <div>
        <p className="m-0 text-caption uppercase tracking-wide text-shodh-text-muted">{KIND_LABELS[column.kind]} · {row.title}</p>
        <h3 className="m-0 mt-0.5 text-body font-semibold text-shodh-text break-words">{column.question}</h3>
      </div>

      <div className={cn('flex flex-col gap-1', stale && 'opacity-70')}>
        {value ? (
          <p className="m-0 text-body-lg text-shodh-text break-words">{valueText(value)}</p>
        ) : null}
        <p className="m-0 flex flex-wrap items-center gap-1.5 text-small">
          <span className={cn('px-1.5 py-0.5 rounded text-caption font-medium', badgeClass)}>
            {explanation.label}
          </span>
          {stale && <span className="px-1.5 py-0.5 rounded text-caption font-medium bg-shodh-raised-2 text-shodh-text-muted">Out of date</span>}
        </p>
        {explanation.detail && <p className="m-0 text-small text-shodh-text-secondary">{explanation.detail}</p>}
        {explanation.refreshFailed && <p className="m-0 text-small text-shodh-warning">{explanation.refreshFailed}</p>}
      </div>

      {segments.length > 0 && (
        <figure className="m-0 flex flex-col gap-1">
          <blockquote className="m-0 pl-3 border-l-2 border-shodh-border-strong text-small leading-[1.55] text-shodh-text-secondary">
            “{segments.map((s, i) => (s.marked ? <mark key={i} className="bg-shodh-accent-soft text-shodh-text rounded-sm px-0.5">{s.text}</mark> : <React.Fragment key={i}>{s.text}</React.Fragment>))}”
          </blockquote>
          <figcaption className="text-caption text-shodh-text-muted flex flex-wrap items-center gap-x-3 gap-y-1">
            {source && <span>Page {source.page}</span>}
            {source && (
              <button
                type="button"
                className={cn('inline-flex items-center gap-1 text-shodh-accent-text hover:underline rounded', FOCUS_RING)}
                onClick={() => showSourceBox({ filePath: source.filePath, fileName: source.fileName, page: source.page, regions: source.regions, label: column.question })}
              >
                <ExternalLink className="w-3 h-3" aria-hidden="true" /> Open in PDF
              </button>
            )}
          </figcaption>
        </figure>
      )}

      {cell && (cell.model || cell.filledAt || cell.editedAt) && (
        <p className="m-0 text-caption text-shodh-text-muted">
          {edited && cell.editedAt
            ? `Edited ${relativeTime(cell.editedAt)}`
            : [cell.model ? `Answered by ${cell.model}` : null, cell.filledAt ? relativeTime(cell.filledAt) : null].filter(Boolean).join(' · ')}
        </p>
      )}

      {editing ? (
        <form
          className="flex flex-col gap-2"
          onSubmit={e => {
            e.preventDefault();
            void saveEdit();
          }}
        >
          <label htmlFor={editId} className="text-caption font-medium text-shodh-text-secondary">
            Your value ({KIND_LABELS[column.kind].toLowerCase()}{column.kind === 'choice' ? `: ${column.options.join(', ')}` : ''})
          </label>
          <input
            id={editId}
            className={INPUT}
            value={draft}
            maxLength={MAX_EDIT_CHARS}
            onChange={e => setDraft(e.target.value)}
            onKeyDown={e => {
              if (e.key === 'Escape') {
                e.stopPropagation();
                setEditing(false);
              }
            }}
            autoFocus
          />
          {editError && <p role="alert" className="m-0 text-small text-shodh-error">{editError}</p>}
          <div className="flex gap-2">
            <button type="submit" className={PRIMARY_BUTTON} disabled={busy !== null || draft.trim().length === 0}>
              {busy === 'edit' && <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
              Save value
            </button>
            <button type="button" className={QUIET_BUTTON} onClick={() => setEditing(false)}>Cancel</button>
          </div>
        </form>
      ) : (
        <div className="flex flex-wrap gap-2">
          <button type="button" className={OUTLINE_BUTTON} disabled={busy !== null || refilling} onClick={startEdit}>
            <Pencil className="w-3.5 h-3.5" aria-hidden="true" /> Edit
          </button>
          <button
            type="button"
            className={OUTLINE_BUTTON}
            disabled={busy !== null || edited || refilling}
            title={edited ? 'Your value is kept: clear it first to ask the model again.' : 'Ask the model again for this cell, over more of the paper. One call.'}
            onClick={startRefill}
          >
            {refilling ? <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <RefreshCw className="w-3.5 h-3.5" aria-hidden="true" />}
            Refill
          </button>
          {hasContent && (
            <button type="button" className={QUIET_BUTTON} disabled={busy !== null || refilling} onClick={() => void clear()}>
              {busy === 'clear' ? <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <Eraser className="w-3.5 h-3.5" aria-hidden="true" />}
              Clear
            </button>
          )}
        </div>
      )}
      {edited && !editing && <p className="m-0 text-caption text-shodh-text-muted">Refill asks the model only for a cell without your value: clear it first.</p>}
      <p role="status" aria-live="polite" className={cn('m-0 text-small', notice ? NOTICE_CLASSES[notice.tone] : 'sr-only')}>
        {notice?.text ?? ''}
      </p>
    </div>
  );
}
