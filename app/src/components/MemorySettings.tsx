import React, { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { ask, save } from '@tauri-apps/plugin-dialog';
import { Download, History, Pencil, Pin, PinOff, RefreshCw, Search, Trash2 } from 'lucide-react';
import { cn } from '../lib/utils';
import { notify } from '../lib/notify';
import { getAppSettings, onAppSettingsChanged, setMemoryPreferences } from '../lib/appSettings';
import type { MemoryPrefs } from '../lib/appSettings';
import { SwitchRow } from './PrivacySettings';
import SuggestedMemories from './SuggestedMemories';
import {
  contentFromEdit,
  editableFields,
  exportFileName,
  forgetConfirmation,
  formatDate,
  groupByClass,
  matchesFilter,
  relativeTime,
  scopeLabel,
  strengthDisplay,
} from '../features/memory/model';
import type { MemoryRecord, StrengthLevel } from '../features/memory/model';
import {
  coreMemories,
  errorText,
  exportMemories,
  forgetMemory,
  forgetNotice,
  listMemories,
  memoryHistory,
  setMemoryPinned,
  updateMemory,
} from '../features/memory/api';
import { OPEN_MEMORY_EDIT, takeRequestedMemoryEdit } from '../features/memory/navigation';
import { coreHeading, shortScope } from '../features/memory/usedMemories';

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
  /** Name of a workspace, by id (memories learned in a workspace are scoped to it). */
  workspaceName: (id: string) => string | undefined;
  /**
   * Only memories and suggestions of this scope (`workspace:<id>`): a workspace's Memory
   * tab. The memory settings themselves stay in Settings → Memory.
   */
  scope?: string | null;
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
  /** Its editor was asked for ("Outdated" under an answer): open it and bring it into view. */
  editRequested?: boolean;
  /** The requested editor was opened. */
  onEditOpened?: () => void;
}

function MemoryItem({
  memory,
  busy,
  onChanged,
  setBusy,
  conversationTitle,
  onOpenConversation,
  workspaceName,
  editRequested = false,
  onEditOpened,
}: MemoryItemProps) {
  const [editing, setEditing] = useState(false);
  const itemRef = useRef<HTMLLIElement>(null);
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
    // Unknown (the audit log could not be read): say what may be left.
    const olderWords = await forgetNotice(memory.id).then(
      n => n.olderEntriesHaveWords,
      () => true,
    );
    const confirmed = await ask(forgetConfirmation(memory.text, olderWords), {
      title: 'Forget memory',
      kind: 'warning',
      okLabel: 'Forget',
      cancelLabel: 'Keep',
    });
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

  // "Outdated" under an answer: the editor opens here, in view, with its first field focused.
  useEffect(() => {
    if (!editRequested || !memory.current) return;
    setDraft({});
    setEditing(true);
    onEditOpened?.();
    requestAnimationFrame(() => {
      const item = itemRef.current;
      if (!item) return;
      item.scrollIntoView({ block: 'center' });
      item.querySelector<HTMLInputElement | HTMLTextAreaElement>('input, textarea')?.focus();
    });
  }, [editRequested, memory.current, onEditOpened]);

  const fromConversation = memory.conversationId;
  const conversationName = fromConversation ? conversationTitle(fromConversation) : undefined;

  return (
    <li ref={itemRef} className="py-3 flex flex-col gap-2">
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
        <span>{scopeLabel(memory.scope, workspaceName)}</span>
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
 * strong each memory is now. Memories are created by the user (asking Shodh to remember
 * and approving it), or learned from what the user says in conversations under the
 * learning mode chosen here (suggestions to accept, or automatic with undo); here the
 * user can also edit, pin, forget and export them.
 */
export default function MemorySettings(props: MemorySettingsProps) {
  const [memories, setMemories] = useState<MemoryRecord[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [core, setCore] = useState<MemoryRecord[] | null>(null);
  const [coreError, setCoreError] = useState<string | null>(null);
  const [filter, setFilter] = useState('');
  const [busy, setBusy] = useState(false);
  const [prefs, setPrefs] = useState<MemoryPrefs | null>(null);
  const [savingPrefs, setSavingPrefs] = useState(false);
  const [exporting, setExporting] = useState(false);
  // The memory whose editor "Outdated" under an answer asked for (Settings → Memory only).
  const [editId, setEditId] = useState<string | null>(() => (props.scope ? null : takeRequestedMemoryEdit()));
  const searchId = useId();

  // Settings, Memory shows the core of chats outside workspaces (global memories).
  const showCore = !props.scope;
  const refresh = useCallback(async () => {
    try {
      setMemories(await listMemories());
      setLoadError(null);
    } catch (err) {
      setLoadError(errorText(err));
    }
    if (!showCore) return;
    try {
      setCore(await coreMemories(null));
      setCoreError(null);
    } catch (err) {
      setCoreError(errorText(err));
    }
  }, [showCore]);

  useEffect(() => {
    void refresh();
    let cancelled = false;
    getAppSettings()
      .then(settings => {
        if (!cancelled && settings) setPrefs(settings.memory);
      })
      .catch(err => notify.error('Memory settings could not be loaded', { description: errorText(err) }));
    const unsubscribe = onAppSettingsChanged(settings => setPrefs(settings.memory));
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, [refresh]);

  // The whole memory section is saved at once: every field is sent, so changing one
  // never resets another.
  const changePrefs = async (next: MemoryPrefs) => {
    setSavingPrefs(true);
    try {
      const saved = await setMemoryPreferences(next);
      if (saved) setPrefs(saved.memory);
    } catch (err) {
      notify.error('The memory setting was not saved', { description: errorText(err) });
    } finally {
      setSavingPrefs(false);
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

  // Asked for while Settings → Memory is open: read the memories again (the one asked
  // for may be newer than the list) and clear the search so it is shown.
  useEffect(() => {
    if (props.scope) return undefined;
    const open = () => {
      const id = takeRequestedMemoryEdit();
      if (!id) return;
      setFilter('');
      setMemories(null);
      setEditId(id);
      void refresh();
    };
    window.addEventListener(OPEN_MEMORY_EDIT, open);
    return () => window.removeEventListener(OPEN_MEMORY_EDIT, open);
  }, [props.scope, refresh]);

  const editOpened = useCallback(() => setEditId(null), []);

  // "Outdated" on a core memory: its editor in the list below.
  const editCore = useCallback((id: string) => {
    setFilter('');
    setEditId(id);
  }, []);

  // The memory asked for is no longer current (edited or forgotten since the answer).
  useEffect(() => {
    if (editId === null || memories === null) return;
    if (!memories.some(m => m.id === editId && m.current)) {
      setEditId(null);
      notify.info('This memory changed since that answer', {
        description: 'It was edited or forgotten. The memories Shodh keeps now are listed here.',
      });
    }
  }, [editId, memories]);

  const scope = props.scope ?? null;
  const scoped = useMemo(
    () => (memories === null ? null : scope ? memories.filter(m => m.scope === scope) : memories),
    [memories, scope],
  );
  const groups = useMemo(
    () => groupByClass((scoped ?? []).filter(m => matchesFilter(m, filter))),
    [scoped, filter],
  );

  return (
    <div className="flex flex-col gap-6">
      {scope === null && (
      <SwitchRow
        label="Use memories in answers"
        description="Before each answer, Shodh recalls what it remembers that is relevant to your question and tells the model, marked as possibly outdated. Turn off to answer without them; your memories are kept."
        checked={prefs?.injectMemories ?? true}
        disabled={prefs === null || savingPrefs}
        onChange={next => prefs && void changePrefs({ ...prefs, injectMemories: next })}
      />
      )}

      {showCore && (
        <CoreMemories
          core={core}
          error={coreError}
          injecting={prefs?.injectMemories ?? true}
          workspaceName={props.workspaceName}
          onOutdated={editCore}
        />
      )}

      {prefs && (
        <SuggestedMemories
          prefs={prefs}
          savingPrefs={savingPrefs}
          onChangePrefs={next => void changePrefs(next)}
          onMemoriesChanged={() => void refresh()}
          conversationTitle={props.conversationTitle}
          onOpenConversation={props.onOpenConversation}
          scope={scope}
        />
      )}

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
        {scope === null && (
          <button type="button" className={BUTTON} onClick={() => void runExport()} disabled={exporting || !memories?.length}>
            <Download aria-hidden="true" className="w-3.5 h-3.5" />
            Export JSON
          </button>
        )}
        <button type="button" className={BUTTON} onClick={() => void refresh()} aria-label="Refresh memories">
          <RefreshCw aria-hidden="true" className="w-3.5 h-3.5" />
        </button>
      </div>

      {loadError ? (
        <p role="alert" className="m-0 rounded-lg border border-shodh-error/40 bg-shodh-surface px-3 py-2 text-[13px] text-shodh-error">
          Memories could not be loaded: {loadError}
        </p>
      ) : scoped === null ? (
        <p className="m-0 text-[13px] text-shodh-text-muted">Loading…</p>
      ) : scoped.length === 0 && scope !== null ? (
        <div className="rounded-lg border border-dashed border-shodh-border px-4 py-5 flex flex-col gap-2">
          <p className="m-0 text-[13.5px] font-semibold text-shodh-text">Nothing remembered in this workspace yet</p>
          <p className="m-0 text-[12.5px] text-shodh-text-muted">
            What Shodh learns from this workspace’s chats, and what you ask it to remember in them, is kept here and
            used only in this workspace’s chats (memories about you in general apply everywhere). Settings → Memory
            controls learning and shows every memory.
          </p>
        </div>
      ) : scoped.length === 0 ? (
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
                  editRequested={memory.id === editId}
                  onEditOpened={editOpened}
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

interface CoreMemoriesProps {
  /** The core memories, as they would be sent now (null while loading). */
  core: MemoryRecord[] | null;
  /** Why they could not be read, if they could not. */
  error: string | null;
  /** Memories are put in front of questions (the switch above). */
  injecting: boolean;
  /** Name of a workspace, by id. */
  workspaceName: (id: string) => string | undefined;
  /** Opens memory `id`'s editor in the list below. */
  onOutdated: (id: string) => void;
}

/**
 * "Always included in answers (N)": the core memories that go in front of every
 * question here, whatever it asks (who the user is, their projects, preferences and the
 * notes they asked Shodh to keep), so the user can see what is always sent. Read-only:
 * which memories are core follows from their kind and strength. "Outdated" opens one's
 * editor; "Not relevant" is given under an answer, where it applies to that answer.
 */
function CoreMemories({ core, error, injecting, workspaceName, onOutdated }: CoreMemoriesProps) {
  const headingId = useId();
  return (
    <section aria-labelledby={headingId} className="flex flex-col gap-1.5">
      <h3 id={headingId} className="m-0 text-[13px] font-bold text-shodh-text-secondary">
        {coreHeading(injecting ? (core?.length ?? 0) : 0)}
      </h3>
      {!injecting ? (
        <p className="m-0 text-[12.5px] text-shodh-text-muted">
          Nothing is sent while “Use memories in answers” is off.
        </p>
      ) : error ? (
        <p role="alert" className="m-0 text-[12.5px] text-shodh-error break-words">
          They could not be read: {error}
        </p>
      ) : core === null ? (
        <p className="m-0 text-[12.5px] text-shodh-text-muted">Loading…</p>
      ) : core.length === 0 ? (
        <p className="m-0 text-[12.5px] text-shodh-text-muted">
          None yet. Who you are, your projects, your preferences and the notes you ask Shodh to keep go with every
          question.
        </p>
      ) : (
        <>
          <p className="m-0 text-[12.5px] text-shodh-text-muted">
            Sent with every question, marked as possibly outdated, pinned and strongest first. “Not relevant” under an
            answer makes one weigh less.
          </p>
          <ul className="m-0 p-0 list-none flex flex-col divide-y divide-shodh-border-subtle">
            {core.map(memory => (
              <li key={memory.id} className="py-1.5 flex items-start gap-2 text-[12.5px]">
                <p className="m-0 flex-1 min-w-0 text-shodh-text break-words">
                  {memory.text}
                  <span className="text-shodh-text-muted">
                    {' · '}
                    {[memory.classLabel, shortScope(memory.scope, workspaceName), memory.pinned ? 'pinned' : '']
                      .filter(part => part.length > 0)
                      .join(' · ')}
                  </span>
                </p>
                <button
                  type="button"
                  className={cn(ICON_BUTTON, 'shrink-0')}
                  onClick={() => onOutdated(memory.id)}
                  aria-label={`Outdated: edit “${memory.text}”`}
                >
                  <Pencil aria-hidden="true" className="w-3.5 h-3.5" /> Outdated
                </button>
              </li>
            ))}
          </ul>
        </>
      )}
    </section>
  );
}
