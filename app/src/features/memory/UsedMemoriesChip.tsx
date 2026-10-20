import React, { useCallback, useEffect, useId, useRef, useState } from 'react';
import { BookmarkCheck, ChevronDown } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { errorText, markNotRelevant, onSuggestionsChanged, usedMemories } from './api';
import { requestMemoryEdit } from './navigation';
import { FORGOTTEN_MEMORY, chipText, markedNotRelevant, rowOf } from './usedMemories';
import type { UsedMemory } from './usedMemories';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

const LINK = cn(
  'shrink-0 underline underline-offset-2 text-shodh-accent-text hover:text-shodh-accent-hover rounded-sm disabled:opacity-50 disabled:cursor-not-allowed',
  FOCUS_RING,
);

interface UsedMemoriesChipProps {
  /** The answer's run (feedback is recorded against it). */
  runId: string;
  /** The memories put in front of its question (`TranscriptState.memoryIds`). */
  ids: readonly string[];
  /** Name of a workspace, by id. */
  workspaceName: (id: string) => string | undefined;
  /** Shows Settings (after "Outdated" asked for a memory's editor there). */
  onOpenSettings: () => void;
}

type Loaded =
  | { state: 'loading' }
  | { state: 'failed'; error: string }
  | { state: 'ready'; used: UsedMemory[] };

/**
 * "Used 2 memories" next to an answer's sources: the memories put in front of its question.
 * Opens a list of them, read live (a forgotten one shows as "a forgotten memory", without
 * its words), each with "Outdated" (its editor in Settings → Memory; saving keeps the old
 * version as history) and "Not relevant" (it did not belong in this answer: it weighs a
 * little less in later answers and is not deleted). Every memory given to the answer is
 * listed: whether the answer used it is judged only to decide what to reinforce.
 */
export function UsedMemoriesChip({ runId, ids, workspaceName, onOpenSettings }: UsedMemoriesChipProps) {
  const [open, setOpen] = useState(false);
  const [loaded, setLoaded] = useState<Loaded>({ state: 'loading' });
  const [busy, setBusy] = useState<string | null>(null);
  const panelId = useId();
  const headingId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  // Only the newest read is applied.
  const seqRef = useRef(0);
  const key = ids.join('\n');

  const load = useCallback(async () => {
    const seq = ++seqRef.current;
    try {
      const used = await usedMemories(runId, key.split('\n').filter(id => id.length > 0));
      if (seq === seqRef.current) setLoaded({ state: 'ready', used });
    } catch (err) {
      if (seq === seqRef.current) setLoaded({ state: 'failed', error: errorText(err) });
    }
  }, [runId, key]);

  // Read when opened, and again on any forget or edit while open, so a forgotten
  // memory's words never stay on screen.
  useEffect(() => {
    if (!open) return undefined;
    setLoaded({ state: 'loading' });
    void load();
    return onSuggestionsChanged((_pending, turns) => {
      if (turns === null) void load();
    });
  }, [open, load]);

  // Close on a click outside.
  useEffect(() => {
    if (!open) return undefined;
    const onDown = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  const label = chipText(ids.length);
  if (label === null) return null;

  const close = () => {
    setOpen(false);
    buttonRef.current?.focus();
  };

  const notRelevant = async (id: string) => {
    setBusy(id);
    try {
      await markNotRelevant(runId, id);
      setLoaded(prev => (prev.state === 'ready' ? { state: 'ready', used: markedNotRelevant(prev.used, id) } : prev));
    } catch (err) {
      notify.error('It was not marked', { description: errorText(err) });
    } finally {
      setBusy(null);
    }
  };

  const outdated = (id: string) => {
    setOpen(false);
    requestMemoryEdit(id);
    onOpenSettings();
  };

  return (
    <div ref={rootRef} className="flex flex-col gap-2 min-w-0">
      <button
        ref={buttonRef}
        type="button"
        onClick={() => setOpen(o => !o)}
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        title="The memories Shodh gave the model with your question"
        className={cn(
          'self-start inline-flex items-center gap-1.5 h-7 max-w-full px-2.5 rounded-full border border-shodh-border bg-shodh-surface text-[12px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
          FOCUS_RING,
        )}
      >
        <BookmarkCheck className="w-3.5 h-3.5 shrink-0 text-shodh-accent-text" aria-hidden="true" />
        <span className="truncate">{label}</span>
        <ChevronDown
          className={cn('w-3 h-3 shrink-0 text-shodh-text-faint transition-transform duration-panel ease-standard', open && 'rotate-180')}
          strokeWidth={2.4}
          aria-hidden="true"
        />
      </button>

      {open && (
        <section
          id={panelId}
          aria-labelledby={headingId}
          onKeyDown={e => {
            if (e.key === 'Escape') {
              e.preventDefault();
              e.stopPropagation();
              close();
            }
          }}
          className="self-start w-[420px] max-w-full rounded-xl border border-shodh-border bg-shodh-surface shadow-lg"
        >
          <h2 id={headingId} className="m-0 px-3.5 pt-2.5 text-[12.5px] font-semibold text-shodh-text">
            Memories given with your question
          </h2>
          {loaded.state === 'loading' ? (
            <p className="m-0 px-3.5 py-3 text-[12.5px] text-shodh-text-muted">Loading…</p>
          ) : loaded.state === 'failed' ? (
            <p role="alert" className="m-0 px-3.5 py-3 text-[12.5px] text-shodh-error break-words">
              The memories could not be read: {loaded.error}
            </p>
          ) : (
            <ul aria-live="polite" className="m-0 p-0 list-none flex flex-col divide-y divide-shodh-border-subtle">
              {loaded.used.map(used => {
                const row = rowOf(used, workspaceName);
                const meta = [row.classLabel, row.since, row.scope].filter(part => part.length > 0);
                return (
                  <li key={row.id} className="px-3.5 py-2.5 flex flex-col gap-1 text-[12.5px]">
                    {row.text === null ? (
                      <p className="m-0 text-shodh-text-muted italic">{FORGOTTEN_MEMORY.charAt(0).toUpperCase() + FORGOTTEN_MEMORY.slice(1)}</p>
                    ) : (
                      <p className="m-0 text-shodh-text break-words">“{row.text}”</p>
                    )}
                    {(meta.length > 0 || row.replaced) && (
                      <p className="m-0 text-[11.5px] text-shodh-text-muted">
                        {meta.join(' · ')}
                        {row.replaced && ' · replaced by a newer version since'}
                      </p>
                    )}
                    {row.text !== null && (
                      <p className="m-0 inline-flex flex-wrap items-center gap-1.5 text-shodh-text-muted">
                        {row.canEdit && (
                          <button
                            type="button"
                            className={LINK}
                            onClick={() => outdated(row.id)}
                            aria-label={`Outdated: edit “${row.text}” in Settings, Memory`}
                          >
                            Outdated
                          </button>
                        )}
                        {row.canEdit && <span aria-hidden="true">·</span>}
                        {row.notRelevant ? (
                          <span>Marked not relevant</span>
                        ) : (
                          <button
                            type="button"
                            className={LINK}
                            disabled={busy !== null}
                            onClick={() => void notRelevant(row.id)}
                            aria-label={`Not relevant to this answer: “${row.text}”`}
                          >
                            Not relevant
                          </button>
                        )}
                      </p>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
          <p className="m-0 px-3.5 py-2 border-t border-shodh-border-subtle text-[11.5px] text-shodh-text-faint">
            Memories the answer drew on grow stronger; “Not relevant” makes one weigh less in later answers without deleting it.
          </p>
        </section>
      )}
    </div>
  );
}
