import React, { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { ask, save } from '@tauri-apps/plugin-dialog';
import { Download, History, Pencil, Pin, PinOff, RefreshCw, Search, Trash2 } from 'lucide-react';
import { cn } from '../lib/utils';
import { notify } from '../lib/notify';
import { getAppSettings, onAppSettingsChanged, setMemoryPreferences } from '../lib/appSettings';
import { SwitchRow } from './PrivacySettings';
import {
  contentFromEdit,
  editableFields,
  exportFileName,
  formatDate,
  groupByClass,
  matchesFilter,
  relativeTime,
  scopeLabel,
  strengthDisplay,
} from '../features/memory/model';
import type { MemoryRecord, StrengthLevel } from '../features/memory/model';
import {
  errorText,
  exportMemories,
  forgetMemory,
  listMemories,
  memoryHistory,
  setMemoryPinned,
  updateMemory,
} from '../features/memory/api';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

const BUTTON = cn(
  'h-8 px-3 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border bg-shodh-surface text-[12.5px] text-shodh-text',
  'hover:bg-shodh-raised disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
  FOCUS_RING,
);

const ICON_BUTTON = cn(
  'h-7 px-2 inline-flex items-center gap-1 rounded-md text-[12px] text-shodh-text-secondary',
  'hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
  FOCUS_RING,
);

const BAR_COLOUR: Record<StrengthLevel, string> = {
  pinned: 'bg-shodh-accent',
  strong: 'bg-shodh-success',
  fading: 'bg-shodh-warning',
  faint: 'bg-shodh-text-faint',
};

interface MemorySettingsProps {
  /** Conversation titles by id, for the "From" link. */
  conversationTitle: (id: string) => string | undefined;
  /** Open a conversation in Ask. */
  onOpenConversation: (id: string) => void;
  /** Source names by id, for workspace-scoped memories. */
  sourceName: (id: string) => string | undefined;
}

function StrengthBar({ memory }: { memory: MemoryRecord }) {
  const display = strengthDisplay(memory.strength, memory.pinned);
  return (
    <div className="flex items-center gap-2 min-w-[150px]">
      <div
        role="meter"
        aria-label="Memory strength"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={display.percent}
        aria-valuetext={display.label}
        className="h-1.5 w-20 rounded-full bg-shodh-raised-2 overflow-hidden"
      >
        <div className={cn('h-full rounded-full', BAR_COLOUR[display.level])} style={{ width: `${display.percent}%` }} />
      </div>
      <span className="text-[11.5px] text-shodh-text-muted whitespace-nowrap">{display.label}</span>
    </div>
  );
}

interface MemoryItemProps extends MemorySettingsProps {
  memory: MemoryRecord;
  busy: boolean;
  onChanged: () => void;
  setBusy: (busy: boolean) => void;
}

function MemoryItem({ memory, busy, onChanged, setBusy, conversationTitle, onOpenConversation, sourceName }: MemoryItemProps) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [history, setHistory] = useState<MemoryRecord[] | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);
  const formId = useId();
  const historyId = useId();
  const now = new Date();
  const fields = editableFields(memory);
  const edit = contentFromEdit(memory, draft);

  const run = async (action: () => Promise<void>, failure: string) => {
    setBusy(true);
    try {
      await action();
      onChanged();
    } catch (err) {
      notify.error(failure, { description: errorText(err) });
    } finally {
      setBusy(false);
    }
  };

  const togglePin = () =>
    void run(async () => {
      await setMemoryPinned(memory.id, !memory.pinned);
    }, memory.pinned ? 'The memory was not unpinned' : 'The memory was not pinned');

  const forget = async () => {
    const confirmed = await ask(
      `Forget “${memory.text}”? Every earlier version of it is forgotten too. This is recorded in the audit log.`,
      { title: 'Forget memory', kind: 'warning', okLabel: 'Forget', cancelLabel: 'Keep' },
    );
    if (!confirmed) return;
    await run(async () => {
      await forgetMemory(memory.id);
      notify.success('Memory forgotten');
    }, 'The memory was not forgotten');
  };

  const saveEdit = () => {
    if (!edit) return;
    void run(async () => {
      await updateMemory(memory.id, edit);
      setEditing(false);
      setHistory(null);
      notify.success('Memory updated', { description: 'The previous version is kept in its history.' });
    }, 'The memory was not updated');
  };

  const toggleHistory = async () => {
    if (historyOpen) {
      setHistoryOpen(false);
      return;
    }
    setHistoryOpen(true);
    if (history) return;
    try {
      setHistory(await memoryHistory(memory.id));
    } catch (err) {
      setHistoryOpen(false);
      notify.error('History could not be loaded', { description: errorText(err) });
    }
  };

  const fromConversation = memory.conversationId;
  const conversationName = fromConversation ? conversationTitle(fromConversation) : undefined;

  return (
    <li className="py-3 flex flex-col gap-2">
      <div className="flex items-start justify-between gap-4">
        <p className="m-0 text-[13.5px] text-shodh-text break-words">{memory.text}</p>
        <StrengthBar memory={memory} />
      </div>

      <p className="m-0 text-[12px] text-shodh-text-muted flex flex-wrap gap-x-2 gap-y-0.5">
        <span>Since {formatDate(memory.validFrom)}</span>
        <span aria-hidden="true">·</span>
        <span>
          Last used {relativeTime(memory.lastUsedAt, now)}
          {memory.useCount > 0 ? ` (${memory.useCount} ${memory.useCount === 1 ? 'time' : 'times'})` : ''}
        </span>
        {memory.expiresAt && (
          <>
            <span aria-hidden="true">·</span>
            <span>Expires {formatDate(memory.expiresAt)}</span>
          </>
        )}
        <span aria-hidden="true">·</span>
        <span>{scopeLabel(memory.scope, sourceName)}</span>
        <span aria-hidden="true">·</span>
        {fromConversation ? (
          <button
            type="button"
            className={cn('underline underline-offset-2 text-shodh-accent-text hover:text-shodh-accent-hover rounded-sm', FOCUS_RING)}
            onClick={() => onOpenConversation(fromConversation)}
          >
            From “{conversationName ?? 'a deleted conversation'}”
          </button>
        ) : (
          <span>Added in Settings</span>
        )}
      </p>

      {editing ? (
        <form
          id={formId}
          className="flex flex-col gap-2 rounded-lg border border-shodh-border-subtle bg-shodh-ground p-3"
          onSubmit={e => {
            e.preventDefault();
            saveEdit();
          }}
        >
          {fields.length === 0 ? (
            <p className="m-0 text-[12.5px] text-shodh-text-muted">
              This memory has no text to edit. Forget it and ask Shodh to remember the corrected fact.
            </p>
          ) : (
            fields.map(field => {
              const inputId = `${formId}-${field.name}`;
              const value = draft[field.name] ?? field.value;
              const common = {
                id: inputId,
                value,
                onChange: (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) =>
                  setDraft(d => ({ ...d, [field.name]: e.target.value })),
                className: cn(
                  'w-full rounded-md border border-shodh-border bg-shodh-surface px-2.5 py-1.5 text-[13px] text-shodh-text',
                  FOCUS_RING,
                ),
              };
              return (
                <div key={field.name} className="flex flex-col gap-1">
                  <label htmlFor={inputId} className="text-[12px] font-semibold text-shodh-text-secondary">
                    {field.label}
                  </label>
                  {field.multiline ? <textarea rows={3} {...common} /> : <input type="text" {...common} />}
                </div>
              );
            })
          )}
          <div className="flex gap-2 justify-end">
            <button type="button" className={BUTTON} onClick={() => { setEditing(false); setDraft({}); }} disabled={busy}>
              Cancel
            </button>
            <button type="submit" className={BUTTON} disabled={busy || edit === null}>
              Save as new version
            </button>
          </div>
        </form>
      ) : (
        <div className="flex flex-wrap gap-1 -ml-2">
          {memory.current && (
            <button type="button" className={ICON_BUTTON} onClick={() => { setDraft({}); setEditing(true); }} disabled={busy}>
              <Pencil aria-hidden="true" className="w-3.5 h-3.5" /> Edit
            </button>
          )}
          <button type="button" className={ICON_BUTTON} onClick={togglePin} disabled={busy} aria-pressed={memory.pinned}>
            {memory.pinned ? <PinOff aria-hidden="true" className="w-3.5 h-3.5" /> : <Pin aria-hidden="true" className="w-3.5 h-3.5" />}
            {memory.pinned ? 'Unpin' : 'Pin'}
          </button>
          <button
            type="button"
            className={ICON_BUTTON}
            onClick={() => void toggleHistory()}
            aria-expanded={historyOpen}
            aria-controls={historyId}
          >
            <History aria-hidden="true" className="w-3.5 h-3.5" /> History
          </button>
          <button type="button" className={cn(ICON_BUTTON, 'hover:text-shodh-error')} onClick={() => void forget()} disabled={busy}>
            <Trash2 aria-hidden="true" className="w-3.5 h-3.5" /> Forget
          </button>
        </div>
      )}

      {historyOpen && (
        <div id={historyId} className="rounded-lg border border-shodh-border-subtle bg-shodh-ground p-3">
          {history === null ? (
            <p className="m-0 text-[12.5px] text-shodh-text-muted">Loading…</p>
          ) : history.length <= 1 ? (
            <p className="m-0 text-[12.5px] text-shodh-text-muted">No earlier versions: this fact has not changed.</p>
          ) : (
            <ol className="m-0 p-0 list-none flex flex-col gap-1.5" aria-label="Versions, oldest first">
              {history.map(version => (
                <li key={version.id} className="text-[12.5px]">
                  <span className={cn('text-shodh-text', !version.current && 'line-through decoration-shodh-text-faint')}>
                    {version.text}
                  </span>
                  <span className="text-shodh-text-muted">
                    {' '}
                    — {formatDate(version.validFrom)} to {version.validTo ? formatDate(version.validTo) : 'now'}
                    {version.current ? ' (current)' : ''}
                  </span>
                </li>
              ))}
            </ol>
          )}
        </div>
      )}
    </li>
  );
}

/**
 * Settings → Memory: what Shodh remembers about the user, grouped by kind, with how
 * strong each memory is now. Memories are created only by the user (asking Shodh to
 * remember, and approving it); here the user can edit, pin, forget and export them.
 */
export default function MemorySettings(props: MemorySettingsProps) {
  const [memories, setMemories] = useState<MemoryRecord[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [filter, setFilter] = useState('');
  const [busy, setBusy] = useState(false);
  const [inject, setInject] = useState<boolean | null>(null);
  const [savingInject, setSavingInject] = useState(false);
  const [exporting, setExporting] = useState(false);
  const searchId = useId();

  const refresh = useCallback(async () => {
    try {
      setMemories(await listMemories());
      setLoadError(null);
    } catch (err) {
      setLoadError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void refresh();
    let cancelled = false;
    getAppSettings()
      .then(settings => {
        if (!cancelled && settings) setInject(settings.memory.injectMemories);
      })
      .catch(err => notify.error('Memory settings could not be loaded', { description: errorText(err) }));
    const unsubscribe = onAppSettingsChanged(settings => setInject(settings.memory.injectMemories));
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, [refresh]);

  const changeInject = async (next: boolean) => {
    setSavingInject(true);
    try {
      const saved = await setMemoryPreferences({ injectMemories: next });
      if (saved) setInject(saved.memory.injectMemories);
    } catch (err) {
      notify.error('The memory setting was not saved', { description: errorText(err) });
    } finally {
      setSavingInject(false);
    }
  };

  const runExport = async () => {
    setExporting(true);
    try {
      const path = await save({
        defaultPath: exportFileName(new Date()),
        filters: [{ name: 'JSON', extensions: ['json'] }],
      });
      if (!path) return;
      const result = await exportMemories(path);
      notify.success(`Exported ${result.memories} ${result.memories === 1 ? 'memory' : 'memories'}`, {
        description: result.path,
      });
    } catch (err) {
      notify.error('Export failed', { description: errorText(err) });
    } finally {
      setExporting(false);
    }
  };

  const groups = useMemo(
    () => groupByClass((memories ?? []).filter(m => matchesFilter(m, filter))),
    [memories, filter],
  );

  return (
    <div className="flex flex-col gap-6">
      <SwitchRow
        label="Use memories in answers"
        description="Before each answer, Shodh recalls what it remembers that is relevant to your question and tells the model, marked as possibly outdated. Turn off to answer without them; your memories are kept."
        checked={inject ?? true}
        disabled={inject === null || savingInject}
        onChange={next => void changeInject(next)}
      />

      <div className="flex flex-wrap items-center gap-2">
        <div className="relative flex-1 min-w-[200px]">
          <label htmlFor={searchId} className="sr-only">Search memories</label>
          <Search aria-hidden="true" className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-shodh-text-muted" />
          <input
            id={searchId}
            type="search"
            value={filter}
            onChange={e => setFilter(e.target.value)}
            placeholder="Search memories"
            className={cn(
              'w-full h-8 pl-8 pr-2.5 rounded-lg border border-shodh-border bg-shodh-surface text-[13px] text-shodh-text placeholder:text-shodh-text-faint',
              FOCUS_RING,
            )}
          />
        </div>
        <button type="button" className={BUTTON} onClick={() => void runExport()} disabled={exporting || !memories?.length}>
          <Download aria-hidden="true" className="w-3.5 h-3.5" />
          Export JSON
        </button>
        <button type="button" className={BUTTON} onClick={() => void refresh()} aria-label="Refresh memories">
          <RefreshCw aria-hidden="true" className="w-3.5 h-3.5" />
        </button>
      </div>

      {loadError ? (
        <p role="alert" className="m-0 rounded-lg border border-shodh-error/40 bg-shodh-surface px-3 py-2 text-[13px] text-shodh-error">
          Memories could not be loaded: {loadError}
        </p>
      ) : memories === null ? (
        <p className="m-0 text-[13px] text-shodh-text-muted">Loading…</p>
      ) : memories.length === 0 ? (
        <div className="rounded-lg border border-dashed border-shodh-border px-4 py-5 flex flex-col gap-2">
          <p className="m-0 text-[13.5px] font-semibold text-shodh-text">Shodh doesn’t remember anything about you yet</p>
          <p className="m-0 text-[12.5px] text-shodh-text-muted">
            Ask in a conversation — for example “Remember that I prefer reports as PDF” or “Remember that Priya is
            my accountant”. Shodh shows you exactly what it will save and saves it only if you approve. It never
            saves anything from your documents or the web on its own.
          </p>
          <p className="m-0 text-[12.5px] text-shodh-text-muted">
            Memories strengthen each time they help an answer and fade when they go unused. When a fact changes,
            the new version replaces the old one, which stays in its history.
          </p>
        </div>
      ) : groups.length === 0 ? (
        <p className="m-0 text-[13px] text-shodh-text-muted">No memories match “{filter.trim()}”.</p>
      ) : (
        groups.map(group => (
          <section key={group.classId} aria-labelledby={`memory-group-${group.classId}`} className="flex flex-col">
            <h3 id={`memory-group-${group.classId}`} className="m-0 text-[13px] font-bold text-shodh-text-secondary">
              {group.heading} <span className="font-normal text-shodh-text-muted">({group.memories.length})</span>
            </h3>
            <ul className="m-0 p-0 list-none divide-y divide-shodh-border-subtle">
              {group.memories.map(memory => (
                <MemoryItem
                  key={memory.id}
                  memory={memory}
                  busy={busy}
                  setBusy={setBusy}
                  onChanged={() => void refresh()}
                  {...props}
                />
              ))}
            </ul>
          </section>
        ))
      )}
    </div>
  );
}
