import React, { useCallback, useEffect, useId, useState } from 'react';
import { ask } from '@tauri-apps/plugin-dialog';
import { Check, Pencil, RotateCcw, ShieldAlert, Square, X } from 'lucide-react';
import { cn } from '../lib/utils';
import { notify } from '../lib/notify';
import { removeWithUndo } from '../lib/undoToast';
import type { LearnMode, MemoryPrefs } from '../lib/appSettings';
import {
  acceptSuggestion,
  acceptSuggestions,
  consolidateNow,
  errorText,
  learnStatus,
  listSuggestions,
  onSuggestionsChanged,
  rejectSuggestion,
  stopLearning,
  undoSuggestion,
} from '../features/memory/api';
import {
  batchAcceptable,
  contentFromSuggestionEdit,
  describeDecision,
  editableSuggestionFields,
  sensitiveLabel,
  suggestionText,
} from '../features/memory/suggestions';
import type { LearnStatus, Suggestion } from '../features/memory/suggestions';
import { formatDate, propertyLabel } from '../features/memory/model';

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

const MODES: { id: LearnMode; label: string; description: string }[] = [
  { id: 'off', label: 'Off', description: 'Nothing is learned from conversations.' },
  { id: 'ask', label: 'Ask', description: 'Shodh suggests memories from what you say; nothing is saved until you accept.' },
  {
    id: 'auto',
    label: 'Automatic',
    description:
      'Confident, non-sensitive suggestions are saved and listed under “Learned”, where you can undo them. Anything sensitive or uncertain still asks.',
  },
];

interface LearningControlsProps {
  prefs: MemoryPrefs;
  saving: boolean;
  onChange: (next: MemoryPrefs) => void;
  status: LearnStatus | null;
  onStopped: () => void;
}

function LearningControls({ prefs, saving, onChange, status, onStopped }: LearningControlsProps) {
  const groupId = useId();
  const modelId = useId();
  const confidenceId = useId();
  const [model, setModel] = useState(prefs.learnModel ?? '');
  const [confidence, setConfidence] = useState(String(Math.round(prefs.autoMinConfidence * 100)));
  const [stopping, setStopping] = useState(false);
  const [consolidating, setConsolidating] = useState(false);

  useEffect(() => setModel(prefs.learnModel ?? ''), [prefs.learnModel]);
  useEffect(() => setConfidence(String(Math.round(prefs.autoMinConfidence * 100))), [prefs.autoMinConfidence]);

  const stop = async () => {
    const confirmed = await ask(
      'Stop learning from conversations now? Learning is turned off, turns waiting to be learned from are dropped and every waiting suggestion is rejected. Memories already saved are kept.',
      { title: 'Stop learning', kind: 'warning', okLabel: 'Stop learning', cancelLabel: 'Keep learning' },
    );
    if (!confirmed) return;
    setStopping(true);
    try {
      const discarded = await stopLearning();
      notify.success('Learning stopped', {
        description: discarded > 0 ? `${discarded} waiting ${discarded === 1 ? 'suggestion was' : 'suggestions were'} rejected.` : undefined,
      });
      onStopped();
    } catch (err) {
      notify.error('Learning was not stopped', { description: errorText(err) });
    } finally {
      setStopping(false);
    }
  };

  const consolidate = async () => {
    setConsolidating(true);
    try {
      await consolidateNow();
      notify.success('Memories reviewed', { description: 'Any suggestions are listed below.' });
      onStopped();
    } catch (err) {
      notify.error('Reviewing memories failed', { description: errorText(err) });
    } finally {
      setConsolidating(false);
    }
  };

  const saveConfidence = () => {
    const value = Number(confidence) / 100;
    if (!Number.isFinite(value) || value < 0.5 || value > 1) {
      notify.error('Confidence must be between 50 and 100');
      setConfidence(String(Math.round(prefs.autoMinConfidence * 100)));
      return;
    }
    if (value !== prefs.autoMinConfidence) onChange({ ...prefs, autoMinConfidence: value });
  };

  return (
    <div className="flex flex-col gap-3">
      <div>
        <p id={groupId} className="m-0 text-[13.5px] font-semibold text-shodh-text">Learn from conversations</p>
        <p className="m-0 mt-0.5 text-[12.5px] text-shodh-text-muted">
          After each answer, a model reads only what you wrote — never your documents, web pages or tool results — and
          suggests memories in Shodh’s types. Health, financial identifiers, passwords and facts about other people always
          ask first.
        </p>
      </div>
      <div role="radiogroup" aria-labelledby={groupId} className="flex flex-wrap gap-2">
        {MODES.map(mode => {
          const checked = prefs.learnMode === mode.id;
          return (
            <button
              key={mode.id}
              type="button"
              role="radio"
              aria-checked={checked}
              disabled={saving || status?.killSwitch === true}
              title={mode.description}
              onClick={() => onChange({ ...prefs, learnMode: mode.id })}
              className={cn(
                'h-8 px-3 rounded-lg border text-[12.5px] transition-colors duration-micro disabled:opacity-50',
                checked
                  ? 'border-shodh-accent bg-shodh-raised-2 text-shodh-text font-semibold'
                  : 'border-shodh-border bg-shodh-surface text-shodh-text-secondary hover:bg-shodh-raised',
                FOCUS_RING,
              )}
            >
              {mode.label}
            </button>
          );
        })}
      </div>
      <p className="m-0 text-[12px] text-shodh-text-muted">{MODES.find(m => m.id === prefs.learnMode)?.description}</p>
      {status?.killSwitch && (
        <p role="status" className="m-0 text-[12.5px] text-shodh-warning">
          Learning is turned off on this computer by its administrator (SHODH_MEMORY_LEARNING=off).
        </p>
      )}
      {prefs.learnMode !== 'off' && status && !status.available && (
        <p role="status" className="m-0 text-[12.5px] text-shodh-warning">
          Not learning right now: {status.unavailableReason ?? 'no model is available'}.
        </p>
      )}
      {prefs.learnMode !== 'off' && (
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div className="flex flex-col gap-1">
            <label htmlFor={modelId} className="text-[12px] font-semibold text-shodh-text-secondary">
              Learning model (same provider)
            </label>
            <input
              id={modelId}
              type="text"
              value={model}
              placeholder={status?.model ?? 'The model you chose in Models'}
              onChange={e => setModel(e.target.value)}
              onBlur={() => {
                const next = model.trim() || null;
                if (next !== prefs.learnModel) onChange({ ...prefs, learnModel: next });
              }}
              className={cn('h-8 rounded-md border border-shodh-border bg-shodh-surface px-2.5 text-[13px] text-shodh-text', FOCUS_RING)}
            />
            <span className="text-[11.5px] text-shodh-text-muted">A smaller model keeps learning cheap. Empty uses your answer model.</span>
          </div>
          {prefs.learnMode === 'auto' && (
            <div className="flex flex-col gap-1">
              <label htmlFor={confidenceId} className="text-[12px] font-semibold text-shodh-text-secondary">
                Save automatically from confidence (%)
              </label>
              <input
                id={confidenceId}
                type="number"
                min={50}
                max={100}
                step={1}
                value={confidence}
                onChange={e => setConfidence(e.target.value)}
                onBlur={saveConfidence}
                className={cn('h-8 w-28 rounded-md border border-shodh-border bg-shodh-surface px-2.5 text-[13px] text-shodh-text', FOCUS_RING)}
              />
            </div>
          )}
        </div>
      )}
      {status && (
        <p className="m-0 text-[12px] text-shodh-text-muted">
          Today: {status.usage.llmCalls} of {status.caps.maxCallsPerDay} model calls, {status.usage.proposals} of{' '}
          {status.caps.maxProposalsPerDay} suggestions
          {status.usage.refused > 0 ? `, ${status.usage.refused} refused (not in your words)` : ''}
          {status.usage.invalid > 0 ? `, ${status.usage.invalid} invalid dropped` : ''}.
          {status.lastConsolidation ? ` Last review of memories ${formatDate(status.lastConsolidation)}.` : ''}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        {prefs.learnMode !== 'off' && (
          <button type="button" className={BUTTON} onClick={() => void consolidate()} disabled={consolidating || !status?.available}>
            Review memories now
          </button>
        )}
        <button
          type="button"
          className={cn(BUTTON, 'hover:text-shodh-error')}
          onClick={() => void stop()}
          disabled={stopping || (prefs.learnMode === 'off' && (status?.pending ?? 0) === 0)}
        >
          <Square aria-hidden="true" className="w-3.5 h-3.5" /> Stop learning
        </button>
      </div>
    </div>
  );
}

interface SuggestionItemProps {
  suggestion: Suggestion;
  busy: boolean;
  run: (action: () => Promise<void>, failure: string) => Promise<void>;
  conversationTitle: (id: string) => string | undefined;
  onOpenConversation: (id: string) => void;
}

function SuggestionItem({ suggestion, busy, run, conversationTitle, onOpenConversation }: SuggestionItemProps) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<Record<string, string>>({});
  const formId = useId();
  const fields = editableSuggestionFields(suggestion);
  const edited = contentFromSuggestionEdit(suggestion, draft);
  const pending = suggestion.status === 'pending' || suggestion.status === 'failed';
  const sensitive = sensitiveLabel(suggestion.sensitive);
  const conversation = suggestion.conversationId;
  const [hidden, setHidden] = useState(false);

  // Hidden at once and rejected when the undo window ends; Undo leaves it pending.
  const reject = () =>
    removeWithUndo({
      message: 'Suggestion rejected',
      description: suggestionText(suggestion),
      hide: () => setHidden(true),
      restore: () => setHidden(false),
      commit: () => rejectSuggestion(suggestion.id),
      onError: err => notify.error('The suggestion was not rejected', { description: errorText(err) }),
    });

  const accept = (edit = false) =>
    run(async () => {
      await acceptSuggestion(suggestion.id, edit ? edited : null);
      setEditing(false);
      notify.success('Memory saved', { description: suggestionText(suggestion) });
    }, 'The suggestion was not accepted');

  if (hidden) return null;

  return (
    <li className="py-3 flex flex-col gap-1.5">
      <p className="m-0 text-[13.5px] text-shodh-text break-words">{suggestionText(suggestion)}</p>
      <p className="m-0 text-[12px] text-shodh-text-secondary">{describeDecision(suggestion)}</p>
      {suggestion.action.kind === 'remember' && (
        <p className="m-0 text-[12px] text-shodh-text-muted">
          You said: “{suggestion.action.evidence}” · confidence {Math.round(suggestion.confidence * 100)}%
        </p>
      )}
      <p className="m-0 text-[12px] text-shodh-text-muted flex flex-wrap gap-x-2">
        <span>{formatDate(suggestion.createdAt)}</span>
        {conversation && (
          <>
            <span aria-hidden="true">·</span>
            <button
              type="button"
              className={cn('underline underline-offset-2 text-shodh-accent-text hover:text-shodh-accent-hover rounded-sm', FOCUS_RING)}
              onClick={() => onOpenConversation(conversation)}
            >
              From “{conversationTitle(conversation) ?? 'a deleted conversation'}”
            </button>
          </>
        )}
        {suggestion.origin === 'consolidate' && (
          <>
            <span aria-hidden="true">·</span>
            <span>From reviewing your memories</span>
          </>
        )}
      </p>
      {sensitive && (
        <p className="m-0 text-[12px] text-shodh-warning inline-flex items-center gap-1">
          <ShieldAlert aria-hidden="true" className="w-3.5 h-3.5" /> {sensitive} — never saved without you
        </p>
      )}
      {suggestion.error && <p className="m-0 text-[12px] text-shodh-error">{suggestion.error}</p>}

      {editing ? (
        <form
          id={formId}
          className="flex flex-col gap-2 rounded-lg border border-shodh-border-subtle bg-shodh-ground p-3"
          onSubmit={e => {
            e.preventDefault();
            if (edited) void accept(true);
          }}
        >
          {fields.map(field => {
            const inputId = `${formId}-${field.name}`;
            return (
              <div key={field.name} className="flex flex-col gap-1">
                <label htmlFor={inputId} className="text-[12px] font-semibold text-shodh-text-secondary">
                  {propertyLabel(field.name)}
                </label>
                <input
                  id={inputId}
                  type="text"
                  value={draft[field.name] ?? field.value}
                  onChange={e => setDraft(d => ({ ...d, [field.name]: e.target.value }))}
                  className={cn('w-full rounded-md border border-shodh-border bg-shodh-surface px-2.5 py-1.5 text-[13px] text-shodh-text', FOCUS_RING)}
                />
              </div>
            );
          })}
          <div className="flex gap-2 justify-end">
            <button type="button" className={BUTTON} onClick={() => { setEditing(false); setDraft({}); }} disabled={busy}>
              Cancel
            </button>
            <button type="submit" className={BUTTON} disabled={busy || edited === null}>
              Save my version
            </button>
          </div>
        </form>
      ) : pending ? (
        <div className="flex flex-wrap gap-1 -ml-2">
          <button type="button" className={ICON_BUTTON} onClick={() => void accept()} disabled={busy}>
            <Check aria-hidden="true" className="w-3.5 h-3.5" /> Accept
          </button>
          {fields.length > 0 && (
            <button type="button" className={ICON_BUTTON} onClick={() => { setDraft({}); setEditing(true); }} disabled={busy}>
              <Pencil aria-hidden="true" className="w-3.5 h-3.5" /> Edit
            </button>
          )}
          <button
            type="button"
            className={cn(ICON_BUTTON, 'hover:text-shodh-error')}
            onClick={reject}
            disabled={busy}
          >
            <X aria-hidden="true" className="w-3.5 h-3.5" /> Reject
          </button>
        </div>
      ) : suggestion.undoable ? (
        <div className="flex flex-wrap gap-1 -ml-2">
          <button
            type="button"
            className={ICON_BUTTON}
            onClick={() =>
              void run(async () => {
                await undoSuggestion(suggestion.id);
                notify.success('Undone');
              }, 'It was not undone')
            }
            disabled={busy}
          >
            <RotateCcw aria-hidden="true" className="w-3.5 h-3.5" /> Undo
          </button>
        </div>
      ) : null}
    </li>
  );
}

interface SuggestedMemoriesProps {
  prefs: MemoryPrefs;
  savingPrefs: boolean;
  onChangePrefs: (next: MemoryPrefs) => void;
  /** Memories changed (refresh the list). */
  onMemoriesChanged: () => void;
  conversationTitle: (id: string) => string | undefined;
  onOpenConversation: (id: string) => void;
  /**
   * Only suggestions for this scope (`workspace:<id>`, a workspace's Memory tab); the
   * learning controls then stay in Settings. Null shows every suggestion.
   */
  scope?: string | null;
}

/**
 * Settings → Memory: learning from conversations — the mode, the "Suggested memories"
 * inbox (accept, edit, reject, accept all) and what was learned automatically (undo).
 */
export default function SuggestedMemories({
  prefs,
  savingPrefs,
  onChangePrefs,
  onMemoriesChanged,
  conversationTitle,
  onOpenConversation,
  scope = null,
}: SuggestedMemoriesProps) {
  const [status, setStatus] = useState<LearnStatus | null>(null);
  const [pending, setPending] = useState<Suggestion[]>([]);
  const [decided, setDecided] = useState<Suggestion[]>([]);
  const [busy, setBusy] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [s, waiting, recent] = await Promise.all([
        learnStatus(),
        listSuggestions(['pending', 'failed']),
        listSuggestions(['learned', 'accepted'], scope ? 200 : 30),
      ]);
      const inScope = (list: Suggestion[]) => (scope ? list.filter(x => x.scope === scope) : list);
      setStatus(s);
      setPending(inScope(waiting));
      setDecided(inScope(recent).slice(0, 30));
      setLoadError(null);
    } catch (err) {
      setLoadError(errorText(err));
    }
  }, [scope]);

  useEffect(() => {
    void refresh();
    return onSuggestionsChanged(() => void refresh());
  }, [refresh]);

  useEffect(() => {
    void refresh();
  }, [prefs.learnMode, refresh]);

  const run = async (action: () => Promise<void>, failure: string) => {
    setBusy(true);
    try {
      await action();
    } catch (err) {
      notify.error(failure, { description: errorText(err) });
    } finally {
      setBusy(false);
      await refresh();
      onMemoriesChanged();
    }
  };

  const batch = batchAcceptable(pending);

  return (
    <div className="flex flex-col gap-5">
      {scope === null && (
        <LearningControls
          prefs={prefs}
          saving={savingPrefs}
          onChange={onChangePrefs}
          status={status}
          onStopped={() => void refresh()}
        />
      )}

      {loadError && (
        <p role="alert" className="m-0 text-[12.5px] text-shodh-error">
          Suggestions could not be loaded: {loadError}
        </p>
      )}

      {pending.length > 0 && (
        <section aria-labelledby="suggested-memories-heading" className="flex flex-col">
          <div className="flex items-center justify-between gap-2">
            <h3 id="suggested-memories-heading" className="m-0 text-[13px] font-bold text-shodh-text-secondary">
              Suggested memories <span className="font-normal text-shodh-text-muted">({pending.length})</span>
            </h3>
            {batch.length > 1 && (
              <button
                type="button"
                className={BUTTON}
                disabled={busy}
                onClick={() =>
                  void run(async () => {
                    const results = await acceptSuggestions(batch.map(s => s.id));
                    const failed = results.filter(r => !r.ok).length;
                    if (failed > 0) notify.error(`${failed} of ${results.length} were not saved`);
                    else notify.success(`${results.length} memories saved`);
                  }, 'The suggestions were not accepted')
                }
              >
                <Check aria-hidden="true" className="w-3.5 h-3.5" /> Accept {batch.length} non-sensitive
              </button>
            )}
          </div>
          <ul className="m-0 p-0 list-none divide-y divide-shodh-border-subtle">
            {pending.map(s => (
              <SuggestionItem
                key={s.id}
                suggestion={s}
                busy={busy}
                run={run}
                conversationTitle={conversationTitle}
                onOpenConversation={onOpenConversation}
              />
            ))}
          </ul>
        </section>
      )}

      {decided.length > 0 && (
        <section aria-labelledby="learned-memories-heading" className="flex flex-col">
          <h3 id="learned-memories-heading" className="m-0 text-[13px] font-bold text-shodh-text-secondary">
            Recently learned <span className="font-normal text-shodh-text-muted">({decided.length})</span>
          </h3>
          <ul className="m-0 p-0 list-none divide-y divide-shodh-border-subtle">
            {decided.map(s => (
              <SuggestionItem
                key={s.id}
                suggestion={s}
                busy={busy}
                run={run}
                conversationTitle={conversationTitle}
                onOpenConversation={onOpenConversation}
              />
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
