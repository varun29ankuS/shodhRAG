import { useEffect, useRef, useState } from 'react';
import type React from 'react';
import { AlertTriangle, Check, Loader2, Settings2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import type { ModelPickerHandle } from './modelApi';
import type { ChipEntry } from './modelFormat';
import { chipEntries } from './modelFormat';
import type { PickerError } from './modelTypes';
import { PROVIDER_LABELS, sameModel } from './modelTypes';

const ITEM =
  'w-full flex items-start gap-2 px-2.5 py-2 rounded-lg text-left hover:bg-shodh-raised focus-visible:outline-none focus-visible:bg-shodh-raised focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50';

interface ChipMenuProps {
  picker: ModelPickerHandle;
  answerRunning: boolean;
  /** Open Settings → Model. */
  onManage: () => void;
  /** After a choice; `changed` is false when it was already the active model. */
  onSelected: (name: string, changed: boolean) => void;
}

/**
 * The composer chip's menu: only connected models (the picks that resolve,
 * then recently used ones) and "Manage…".
 */
export function ChipMenu({ picker, answerRunning, onManage, onSelected }: ChipMenuProps) {
  const { view, loadError, select, choosePick } = picker;
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<ChipEntry | null>(null);
  const [refusal, setRefusal] = useState<PickerError | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const loaded = view !== null;

  // Focus the first item once the menu has its models.
  useEffect(() => {
    if (loaded) listRef.current?.querySelector<HTMLButtonElement>('[role="menuitemradio"],[role="menuitem"]')?.focus();
  }, [loaded]);

  if (!view) {
    return (
      <p role="status" className="flex items-center gap-2 p-4 text-[12.5px] text-shodh-text-muted">
        {loadError ? (
          <>
            <AlertTriangle className="w-4 h-4 shrink-0 text-shodh-warning" aria-hidden="true" />
            The models could not be loaded: {loadError}
          </>
        ) : (
          <>
            <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
            Loading models…
          </>
        )}
      </p>
    );
  }

  const entries = chipEntries(view);
  const activePick = view.active?.source === 'settings' ? view.prefs.pick : null;

  const run = async (entry: ChipEntry, sessionOverride: boolean) => {
    setBusy(true);
    setRefusal(null);
    const error =
      entry.kind === 'pick' && entry.pick
        ? await choosePick(entry.pick, sessionOverride)
        : await select(entry.model, { sessionOverride });
    setBusy(false);
    if (!error) {
      setPending(null);
      onSelected(entry.name, true);
    } else if (error.code === 'environment_set') {
      setPending(entry);
    } else {
      setRefusal(error);
    }
  };

  const choose = (entry: ChipEntry) => {
    if (busy) return;
    if (sameModel(view.active?.model, entry.model) && (entry.kind === 'recent' || activePick === entry.pick)) {
      onSelected(entry.name, false);
      return;
    }
    void run(entry, false);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp' && e.key !== 'Home' && e.key !== 'End') return;
    const items = Array.from(listRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"],[role="menuitem"]') ?? []);
    if (items.length === 0) return;
    e.preventDefault();
    const at = items.indexOf(document.activeElement as HTMLButtonElement);
    const next =
      e.key === 'Home' ? 0 : e.key === 'End' ? items.length - 1 : e.key === 'ArrowDown' ? (at + 1) % items.length : (at - 1 + items.length) % items.length;
    items[next]?.focus();
  };

  return (
    <div ref={listRef} role="menu" aria-label="Models" onKeyDown={onKeyDown} className="flex flex-col p-1.5 max-h-[420px] overflow-y-auto scrollbar-thin">
      {answerRunning && (
        <p className="px-2.5 pt-1 pb-2 m-0 text-[11.5px] text-shodh-text-muted">The answer in progress finishes with its model.</p>
      )}
      {entries.length === 0 && (
        <p className="px-2.5 py-3 m-0 text-[12.5px] text-shodh-text-muted">Nothing is connected yet. Connect a subscription, an API key or a local model.</p>
      )}
      {entries.map(entry => {
        const active =
          sameModel(view.active?.model, entry.model) && (entry.kind === 'recent' ? activePick === null || view.active?.source !== 'settings' : activePick === entry.pick);
        return (
          <button
            key={`${entry.kind}-${entry.pick ?? ''}-${entry.model.provider}:${entry.model.model}`}
            type="button"
            role="menuitemradio"
            aria-checked={active}
            disabled={busy}
            onClick={() => choose(entry)}
            className={ITEM}
          >
            <Check className={cn('w-3.5 h-3.5 mt-[3px] shrink-0 text-shodh-accent-text', !active && 'invisible')} aria-hidden="true" />
            <span className="min-w-0 flex flex-col">
              <span className="text-[13px] font-medium text-shodh-text">{entry.kind === 'pick' ? entry.label : entry.name}</span>
              <span className="text-[11.5px] text-shodh-text-muted truncate">
                {entry.kind === 'pick' ? `${entry.name} · ${PROVIDER_LABELS[entry.model.provider]}` : `Recent · ${PROVIDER_LABELS[entry.model.provider]}`}
              </span>
            </span>
          </button>
        );
      })}
      {pending && (
        <div role="alertdialog" aria-label="Override the environment's model" className="mx-1 my-1.5 p-2.5 rounded-lg bg-shodh-raised flex flex-col gap-2 text-[12px] text-shodh-text-secondary">
          <p className="m-0">The environment sets the model for this computer. Use {pending.name} for this session only?</p>
          <div className="flex gap-2">
            <button type="button" autoFocus className={cn(ITEM, 'w-auto px-2.5 py-1 border border-shodh-border')} onClick={() => void run(pending, true)}>
              Use for this session
            </button>
            <button type="button" className={cn(ITEM, 'w-auto px-2.5 py-1 border border-shodh-border')} onClick={() => setPending(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}
      {refusal && (
        <p role="alert" className="mx-1 my-1.5 px-2.5 py-2 rounded-lg bg-shodh-raised m-0 text-[12px] text-shodh-text-secondary">
          {refusal.message}
        </p>
      )}
      <div className="my-1 border-t border-shodh-border-subtle" />
      <button type="button" role="menuitem" onClick={onManage} className={ITEM}>
        <Settings2 className="w-3.5 h-3.5 mt-[3px] shrink-0 text-shodh-text-muted" aria-hidden="true" />
        <span className="text-[13px] text-shodh-text">Manage…</span>
      </button>
    </div>
  );
}
