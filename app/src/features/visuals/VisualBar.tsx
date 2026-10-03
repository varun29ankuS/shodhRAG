import React, { useCallback, useEffect, useId, useRef, useState } from 'react';
import { CornerDownRight, Download, Loader2, Pencil, Pin, PinOff, Sparkles, Square, Trash2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { useChatSession } from '../ask/ChatSessionContext';
import { useFocus } from '../focus/FocusContext';
import type { OpenFocus } from '../focus/FocusContext';
import { confirmDelete, goToMessage, runExport } from './actions';
import { onVisualsChanged, toVisualError, visualsApi } from './api';
import { EditVisualDialog } from './EditVisualDialog';
import { KIND_NOUN } from './extract';
import { exportFormats, FORMAT_LABEL } from './exportVisual';
import type { VisualDetail } from './model';
import { paramsFor } from './model';
import { MAX_REFINE_INSTRUCTION_CHARS } from './refine';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';
const BAR_BUTTON = cn(
  'h-7 px-2 inline-flex items-center gap-1.5 rounded-lg text-[12px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-40 disabled:cursor-not-allowed transition-colors duration-micro',
  FOCUS_RING,
);

/**
 * The gallery strip of the focus pop-out, for a visual opened from the
 * gallery: version switcher, Go to message, pin, rename and note, export,
 * delete, and Refine (ask for a change; the result is saved as the next
 * version and shown, the original answer is never changed).
 *
 * `stage` is the element the visual is drawn in (for export).
 */
export function VisualBar({
  level,
  stage,
  dark,
  onClose,
}: {
  level: OpenFocus;
  stage: React.RefObject<HTMLElement | null>;
  dark: boolean;
  onClose: () => void;
}) {
  const focus = useFocus();
  const { switchConversation } = useChatSession();
  const visual = level.visual;
  const [detail, setDetail] = useState<VisualDetail | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  const [editing, setEditing] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [instruction, setInstruction] = useState('');
  const [refining, setRefining] = useState(false);
  const [refineMessage, setRefineMessage] = useState<{ tone: 'error' | 'done'; text: string } | null>(null);
  const inputId = useId();
  const statusId = useId();
  const exportRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!visual) return;
    let cancelled = false;
    visualsApi.get(visual.id)
      .then(d => { if (!cancelled) { setDetail(d); setLoadError(null); } })
      .catch(error => { if (!cancelled) setLoadError(toVisualError(error).message); });
    return () => { cancelled = true; };
  }, [visual, reload]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    onVisualsChanged(() => setReload(r => r + 1))
      .then(fn => { if (disposed) fn(); else unlisten = fn; })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (!exportOpen) return;
    exportRef.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    const onPointer = (e: PointerEvent) => {
      if (!exportRef.current?.contains(e.target as Node)) setExportOpen(false);
    };
    window.addEventListener('pointerdown', onPointer, true);
    return () => window.removeEventListener('pointerdown', onPointer, true);
  }, [exportOpen]);

  const showVersion = useCallback((id: string) => {
    visualsApi.get(id)
      .then(d => focus?.showVisualVersion(d.visual))
      .catch(error => notify.error('That version could not be opened', { description: toVisualError(error).message }));
  }, [focus]);

  if (!visual) return null;
  const record = detail?.visual ?? null;
  const noun = record ? KIND_NOUN[record.kind].toLowerCase() : 'visual';
  const busyElsewhere = focus?.sideLive !== null && focus?.sideLive !== undefined && !refining;

  const pin = () => {
    if (!record) return;
    visualsApi.setPinned(record.id, !record.pinned)
      .then(setDetail)
      .catch(error => notify.error('The pin did not change', { description: toVisualError(error).message }));
  };

  const refine = async (e: React.FormEvent) => {
    e.preventDefault();
    const text = instruction.trim();
    if (!text || !focus || !record || refining) return;
    setRefining(true);
    setRefineMessage(null);
    const outcome = await focus.refine(level, text);
    if ('message' in outcome) {
      setRefining(false);
      setRefineMessage({ tone: 'error', text: outcome.message });
      return;
    }
    try {
      const values = level.target.kind === 'plot' || level.target.kind === 'simulation' ? level.target.values : [];
      const saved = await visualsApi.addVersion(record.id, outcome.source, paramsFor(values), text);
      setInstruction('');
      setRefineMessage({ tone: 'done', text: `Saved as version ${saved.visual.version}.${outcome.note ? ` ${outcome.note}` : ''}` });
      focus.showVisualVersion(saved.visual);
    } catch (error) {
      setRefineMessage({ tone: 'error', text: `The new version was not saved: ${toVisualError(error).message}` });
    } finally {
      setRefining(false);
    }
  };

  const onExportKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const items = Array.from(exportRef.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? []);
    const index = items.indexOf(document.activeElement as HTMLElement);
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      items[(index + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length]?.focus();
    } else if (e.key === 'Escape') {
      // Closes the menu only, not the pop-out.
      e.preventDefault();
      e.stopPropagation();
      setExportOpen(false);
    }
  };

  return (
    <div className="shrink-0 border-b border-shodh-border-subtle bg-shodh-surface-2 px-3 py-2 flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-1.5">
        {detail && detail.versions.length > 1 ? (
          <div role="radiogroup" aria-label="Version" className="flex items-center gap-0.5 mr-1">
            {detail.versions.map(v => (
              <button
                key={v.id}
                type="button"
                role="radio"
                aria-checked={v.id === visual.id}
                onClick={() => { if (v.id !== visual.id) showVersion(v.id); }}
                title={v.instruction ? `v${v.version}: ${v.instruction}` : v.version === 1 ? 'v1: from the answer' : `v${v.version}`}
                className={cn(
                  'h-7 min-w-[34px] px-2 rounded-lg text-[12px] tabular-nums transition-colors duration-micro',
                  v.id === visual.id ? 'bg-shodh-accent-soft text-shodh-accent-text font-semibold' : 'text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text',
                  FOCUS_RING,
                )}
              >
                {`v${v.version}`}
              </button>
            ))}
          </div>
        ) : (
          <span className="mr-1 text-[12px] text-shodh-text-muted">{detail ? 'v1 · from the answer' : loadError ? '' : 'Loading…'}</span>
        )}
        {loadError && <span role="alert" className="text-[12px] text-shodh-error">{loadError}</span>}
        <span className="flex-1" />
        <button
          type="button"
          className={BAR_BUTTON}
          disabled={!visual.messageId}
          title={visual.messageId ? 'Show the answer this came from' : 'This came from a side discussion without an answer'}
          onClick={() => {
            onClose();
            goToMessage(visual, switchConversation);
          }}
        >
          <CornerDownRight className="w-3.5 h-3.5" aria-hidden="true" />
          Go to message
        </button>
        <button type="button" className={BAR_BUTTON} disabled={!record} aria-pressed={record?.pinned ?? false} onClick={pin}>
          {record?.pinned ? <PinOff className="w-3.5 h-3.5" aria-hidden="true" /> : <Pin className="w-3.5 h-3.5" aria-hidden="true" />}
          {record?.pinned ? 'Unpin' : 'Pin'}
        </button>
        <button type="button" className={BAR_BUTTON} disabled={!record} onClick={() => setEditing(true)}>
          <Pencil className="w-3.5 h-3.5" aria-hidden="true" />
          Rename
        </button>
        <div className="relative">
          <button
            type="button"
            className={BAR_BUTTON}
            disabled={!record}
            aria-haspopup="menu"
            aria-expanded={exportOpen}
            onClick={() => setExportOpen(o => !o)}
          >
            <Download className="w-3.5 h-3.5" aria-hidden="true" />
            Export
          </button>
          {exportOpen && record && (
            <div
              ref={exportRef}
              role="menu"
              aria-label="Export as"
              data-esc-local=""
              onKeyDown={onExportKeyDown}
              className="absolute right-0 top-8 z-30 min-w-[190px] rounded-xl border border-shodh-border-strong bg-shodh-surface p-1 shadow-[0_12px_36px_rgba(0,0,0,0.32)] shell-pop"
            >
              {exportFormats(record.kind).map(format => (
                <button
                  key={format}
                  type="button"
                  role="menuitem"
                  tabIndex={-1}
                  onClick={() => {
                    setExportOpen(false);
                    void runExport(record, format, stage.current, dark);
                  }}
                  className="w-full h-8 px-2.5 rounded-lg inline-flex items-center gap-2 text-left text-[12.5px] text-shodh-text hover:bg-shodh-raised focus:bg-shodh-raised focus:outline-none"
                >
                  {FORMAT_LABEL[format]}
                </button>
              ))}
            </div>
          )}
        </div>
        <button
          type="button"
          className={cn(BAR_BUTTON, 'hover:text-shodh-error')}
          disabled={!record}
          onClick={() => {
            if (!record) return;
            void confirmDelete(record).then(deleted => { if (deleted) onClose(); });
          }}
        >
          <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
          Delete
        </button>
      </div>

      <form onSubmit={e => void refine(e)} className="flex items-center gap-2">
        <label htmlFor={inputId} className="sr-only">{`Describe a change to this ${noun}`}</label>
        <input
          id={inputId}
          value={instruction}
          maxLength={MAX_REFINE_INSTRUCTION_CHARS}
          onChange={e => setInstruction(e.target.value)}
          disabled={refining || !record}
          aria-describedby={refineMessage ? statusId : undefined}
          placeholder={`Refine this ${noun}: “label the forces”, “use a log scale”…`}
          className="flex-1 min-w-0 h-8 px-3 rounded-lg border border-shodh-border bg-shodh-surface text-[12.5px] text-shodh-text placeholder:text-shodh-text-faint focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-60"
        />
        {refining ? (
          <button
            type="button"
            onClick={() => focus?.stop()}
            className={cn('h-8 px-3 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border text-[12.5px] text-shodh-text hover:bg-shodh-raised', FOCUS_RING)}
          >
            <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />
            <Square className="w-3 h-3" aria-hidden="true" />
            Stop
          </button>
        ) : (
          <button
            type="submit"
            disabled={!instruction.trim() || !record || busyElsewhere}
            title={busyElsewhere ? 'Another answer is running' : 'Save a revised version (the original stays)'}
            className={cn('h-8 px-3 inline-flex items-center gap-1.5 rounded-lg bg-shodh-accent text-shodh-on-accent text-[12.5px] font-semibold hover:bg-shodh-accent-hover disabled:opacity-50 disabled:cursor-not-allowed', FOCUS_RING)}
          >
            <Sparkles className="w-3.5 h-3.5" aria-hidden="true" />
            Refine
          </button>
        )}
      </form>
      <p id={statusId} role="status" aria-live="polite" className={cn('text-[12px] min-h-0', refineMessage?.tone === 'error' ? 'text-shodh-error' : 'text-shodh-text-muted', !refineMessage && 'sr-only')}>
        {refining ? `Refining the ${noun}…` : refineMessage?.text ?? ''}
      </p>

      {record && (
        <EditVisualDialog
          record={record}
          open={editing}
          onOpenChange={setEditing}
          onSaved={setDetail}
        />
      )}
    </div>
  );
}
