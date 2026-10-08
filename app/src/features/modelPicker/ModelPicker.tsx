import { useEffect, useId, useMemo, useRef, useState } from 'react';
import type React from 'react';
import { AlertTriangle, Check, Cpu, Loader2, RefreshCw, Search, ShieldAlert, Star, Wrench } from 'lucide-react';
import { cn } from '../../lib/utils';
import type { ModelPickerHandle } from './modelApi';
import type { CatalogModel, ModelRef, PickerError } from './modelTypes';
import { PROVIDER_LABELS, sameModel } from './modelTypes';
import {
  catalogNotice,
  formatContext,
  formatPrice,
  modelKey,
  modelName,
  pickerSections,
  privacyText,
  sourceText,
} from './modelFormat';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

const SMALL_BUTTON = cn(
  'inline-flex items-center gap-1.5 h-7 px-2.5 rounded-lg border border-shodh-border bg-shodh-raised text-[12px] font-medium text-shodh-text-secondary hover:bg-shodh-raised-2 hover:text-shodh-text transition-colors duration-micro disabled:opacity-50 disabled:pointer-events-none',
  FOCUS_RING,
);

/** A selection waiting for the person to confirm a warning. */
type Pending =
  | { kind: 'stealth'; model: CatalogModel }
  | { kind: 'environment'; model: CatalogModel; confirmStealth: boolean };

interface ModelPickerPanelProps {
  picker: ModelPickerHandle;
  /** An answer is running: a selection applies to the next one. */
  answerRunning: boolean;
  /** After a model was selected; `changed` is false when it was already the active one. */
  onSelected?: (model: CatalogModel, changed: boolean) => void;
  /** Open Settings → Model (keys live there). */
  onOpenSettings?: () => void;
  /** Focus the search field on mount (the popover). */
  autoFocus?: boolean;
  /** Tall list for Settings; the popover is shorter. */
  tall?: boolean;
}

/**
 * The list of models the agent can answer with, grouped Favourites, Recent,
 * Free, Paid and Local, with search. Each row shows the context window, the
 * price per million tokens (or "Price unknown" offline), whether tool
 * calling is confirmed and what happens to prompts. Stealth models and
 * overriding the environment's model need an explicit confirmation here.
 */
export function ModelPickerPanel({ picker, answerRunning, onSelected, onOpenSettings, autoFocus = false, tall = false }: ModelPickerPanelProps) {
  const { view, loading, loadError, reload, select, setFavourite } = picker;
  const [query, setQuery] = useState('');
  const [activeIndex, setActiveIndex] = useState(0);
  const [pending, setPending] = useState<Pending | null>(null);
  const [refusal, setRefusal] = useState<PickerError | null>(null);
  const [busy, setBusy] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);
  const listId = useId();
  const noticeId = useId();

  useEffect(() => {
    if (autoFocus) searchRef.current?.focus();
  }, [autoFocus]);

  const sections = useMemo(
    () => (view ? pickerSections(view.models, view.prefs, query) : []),
    [view, query],
  );
  const flat = useMemo(() => sections.flatMap(s => s.models.map(m => ({ section: s.id, model: m }))), [sections]);
  const clampedIndex = Math.min(activeIndex, Math.max(0, flat.length - 1));
  const optionId = (index: number) => `${listId}-opt-${index}`;

  useEffect(() => {
    setActiveIndex(0);
  }, [query]);

  // Keep the highlighted option in view while moving with the arrow keys.
  useEffect(() => {
    document.getElementById(`${listId}-opt-${clampedIndex}`)?.scrollIntoView({ block: 'nearest' });
  }, [listId, clampedIndex]);

  const run = async (model: CatalogModel, confirmStealth: boolean, sessionOverride: boolean) => {
    const ref: ModelRef = { provider: model.provider, model: model.id };
    setBusy(true);
    setRefusal(null);
    const error = await select(ref, { confirmStealth, sessionOverride });
    setBusy(false);
    if (!error) {
      setPending(null);
      onSelected?.(model, true);
      return;
    }
    if (error.code === 'stealth_confirmation') setPending({ kind: 'stealth', model });
    else if (error.code === 'environment_set') setPending({ kind: 'environment', model, confirmStealth });
    else setRefusal(error);
  };

  const choose = (model: CatalogModel) => {
    if (busy) return;
    if (view && sameModel(view.active?.model, { provider: model.provider, model: model.id })) {
      onSelected?.(model, false);
      return;
    }
    void run(model, false, false);
  };

  const onSearchKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (flat.length === 0) return;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setActiveIndex(i => Math.min(flat.length - 1, i + 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setActiveIndex(i => Math.max(0, i - 1));
    } else if (e.key === 'Home' && e.ctrlKey) {
      e.preventDefault();
      setActiveIndex(0);
    } else if (e.key === 'End' && e.ctrlKey) {
      e.preventDefault();
      setActiveIndex(flat.length - 1);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const item = flat[clampedIndex];
      if (item) choose(item.model);
    }
  };

  if (!view) {
    return (
      <div className="flex items-center gap-2 p-4 text-[12.5px] text-shodh-text-muted" role="status">
        {loadError ? (
          <>
            <AlertTriangle className="w-4 h-4 shrink-0 text-shodh-warning" aria-hidden="true" />
            <span className="flex-1">The model list could not be loaded: {loadError}</span>
            <button type="button" className={SMALL_BUTTON} onClick={() => void reload(false)}>
              Try again
            </button>
          </>
        ) : (
          <>
            <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
            Loading models…
          </>
        )}
      </div>
    );
  }

  const activeRef = view.active?.model ?? null;
  const source = sourceText(view.active);
  const notice = catalogNotice(view, Date.now());
  const favourite = (m: CatalogModel) => view.prefs.favourites.some(f => f.provider === m.provider && f.model === m.id);
  let index = -1;

  return (
    <div className="flex flex-col min-h-0">
      {view.environment && (
        <p className="mx-3 mt-3 px-3 py-2 rounded-lg bg-shodh-raised text-[12px] leading-relaxed text-shodh-text-secondary">
          <span className="font-semibold text-shodh-text">Set by environment:</span>{' '}
          {modelName(view.models, view.environment)} ({PROVIDER_LABELS[view.environment.provider]}).{' '}
          {view.active?.source === 'session'
            ? `This session uses ${modelName(view.models, activeRef)} instead; the environment's model returns when Shodh restarts.`
            : 'Choosing another model overrides it for this session only.'}
        </p>
      )}
      {view.localOnly && (
        <p className="mx-3 mt-3 px-3 py-2 rounded-lg bg-shodh-raised text-[12px] text-shodh-text-secondary">
          Local-only mode is on: only models on this computer can answer.
        </p>
      )}

      <div className="flex items-center gap-2 px-3 pt-3 pb-2">
        <label className="relative flex-1 min-w-0">
          <span className="sr-only">Search models</span>
          <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-shodh-text-faint pointer-events-none" aria-hidden="true" />
          <input
            ref={searchRef}
            type="search"
            value={query}
            onChange={e => setQuery(e.target.value)}
            onKeyDown={onSearchKeyDown}
            placeholder="Search models"
            role="combobox"
            aria-expanded="true"
            aria-controls={listId}
            aria-activedescendant={flat.length > 0 ? optionId(clampedIndex) : undefined}
            aria-describedby={notice ? noticeId : undefined}
            className={cn(
              'w-full h-8 pl-8 pr-2 rounded-lg border border-shodh-border bg-shodh-ground text-[12.5px] text-shodh-text placeholder:text-shodh-text-faint',
              FOCUS_RING,
            )}
          />
        </label>
        <button
          type="button"
          onClick={() => void reload(true)}
          disabled={loading || view.localOnly}
          aria-label="Refresh the model list and prices"
          title="Refresh the model list and prices"
          className={cn('w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-50', FOCUS_RING)}
        >
          <RefreshCw className={cn('w-3.5 h-3.5', loading && 'animate-spin motion-reduce:animate-none')} aria-hidden="true" />
        </button>
      </div>

      {notice && (
        <p id={noticeId} className="px-4 pb-2 text-[11.5px] text-shodh-text-muted">
          {notice}
        </p>
      )}
      {answerRunning && (
        <p className="px-4 pb-2 text-[11.5px] text-shodh-text-muted">The answer in progress finishes with its model; a new choice applies to the next answer.</p>
      )}

      {pending && (
        <div role="alertdialog" aria-labelledby={`${listId}-confirm`} className="mx-3 mb-2 p-3 rounded-lg border border-shodh-warning/50 bg-shodh-raised flex flex-col gap-2">
          <p id={`${listId}-confirm`} className="flex items-start gap-2 text-[12.5px] leading-relaxed text-shodh-text">
            <ShieldAlert className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
            {pending.kind === 'stealth'
              ? `${pending.model.name} is a stealth model: its provider may log your prompts and the passages sent with them, and use them for training. Do not use it with confidential files.`
              : `The environment sets the model for this computer. Use ${pending.model.name} for this session only? The environment's model returns when Shodh restarts.`}
          </p>
          <div className="flex gap-2">
            <button
              type="button"
              autoFocus
              disabled={busy}
              className={SMALL_BUTTON}
              onClick={() =>
                pending.kind === 'stealth'
                  ? void run(pending.model, true, false)
                  : void run(pending.model, pending.confirmStealth, true)
              }
            >
              {pending.kind === 'stealth' ? 'I accept, use it' : 'Use for this session'}
            </button>
            <button type="button" className={SMALL_BUTTON} onClick={() => { setPending(null); searchRef.current?.focus(); }}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {refusal && (
        <div role="alert" className="mx-3 mb-2 px-3 py-2 rounded-lg bg-shodh-raised flex flex-wrap items-center gap-2 text-[12px] text-shodh-text-secondary">
          <AlertTriangle className="w-3.5 h-3.5 shrink-0 text-shodh-warning" aria-hidden="true" />
          <span className="flex-1 min-w-[12rem]">{refusal.message}</span>
          {refusal.code === 'missing_key' && onOpenSettings && (
            <button type="button" className={SMALL_BUTTON} onClick={onOpenSettings}>
              Add a key
            </button>
          )}
        </div>
      )}

      <div
        id={listId}
        role="listbox"
        aria-label="Models"
        className={cn('overflow-y-auto scrollbar-thin px-1.5 pb-2', tall ? 'max-h-[460px]' : 'max-h-[360px]')}
      >
        {sections.length === 0 && (
          <p className="px-3 py-6 text-center text-[12.5px] text-shodh-text-muted">
            {query.trim()
              ? 'No model matches.'
              : view.connected.length === 0
                ? 'Nothing is connected. Sign in, paste an API key or start LM Studio in Settings → Model.'
                : 'No model with tool calling is available.'}
          </p>
        )}
        {sections.map(section => (
          <div key={section.id} role="group" aria-labelledby={`${listId}-${section.id}`}>
            <div id={`${listId}-${section.id}`} className="px-2.5 pt-2.5 pb-1 text-[10.5px] font-semibold uppercase tracking-[0.08em] text-shodh-text-faint">
              {section.title}
            </div>
            {section.models.map(model => {
              index += 1;
              const i = index;
              const ref = { provider: model.provider, model: model.id };
              const isActive = sameModel(activeRef, ref);
              const starred = favourite(model);
              const context = formatContext(model.contextLength);
              return (
                <div
                  key={`${section.id}-${modelKey(ref)}`}
                  id={optionId(i)}
                  role="option"
                  aria-selected={isActive}
                  data-highlighted={i === clampedIndex || undefined}
                  onClick={() => choose(model)}
                  onMouseMove={() => setActiveIndex(i)}
                  className={cn(
                    'group relative flex items-start gap-2 px-2.5 py-2 rounded-lg cursor-pointer',
                    i === clampedIndex ? 'bg-shodh-raised' : 'hover:bg-shodh-raised',
                  )}
                >
                  <Check className={cn('w-3.5 h-3.5 mt-[3px] shrink-0 text-shodh-accent-text', !isActive && 'invisible')} aria-hidden="true" />
                  <div className="flex-1 min-w-0">
                    <div className="flex items-baseline gap-2 min-w-0">
                      <span className="truncate text-[13px] font-medium text-shodh-text">{model.name}</span>
                      <span className="shrink-0 text-[11px] text-shodh-text-faint">{PROVIDER_LABELS[model.provider]}</span>
                    </div>
                    <div className="mt-0.5 flex flex-wrap items-center gap-x-2.5 gap-y-0.5 text-[11.5px] text-shodh-text-muted">
                      <span className="tabular-nums">{formatPrice(model)}</span>
                      {context && <span className="tabular-nums">{context} context</span>}
                      {model.tools === 'unknown' && (
                        <span className="inline-flex items-center gap-1" title="The provider does not say whether this model supports tool calling; the agent needs it.">
                          <Wrench className="w-3 h-3" aria-hidden="true" />
                          Tool calling unconfirmed
                        </span>
                      )}
                    </div>
                    <div className={cn('mt-0.5 text-[11px]', model.stealth || model.privacy === 'may_log_prompts' ? 'text-shodh-warning' : 'text-shodh-text-faint')}>
                      {privacyText(model.privacy, model.stealth)}
                    </div>
                  </div>
                  <button
                    type="button"
                    tabIndex={-1}
                    onClick={e => {
                      e.stopPropagation();
                      void setFavourite(ref, !starred);
                    }}
                    aria-label={starred ? `Remove ${model.name} from favourites` : `Add ${model.name} to favourites`}
                    aria-pressed={starred}
                    title={starred ? 'Remove from favourites' : 'Add to favourites'}
                    className="w-7 h-7 shrink-0 inline-flex items-center justify-center rounded-md text-shodh-text-faint hover:text-shodh-text hover:bg-shodh-raised-2"
                  >
                    <Star className={cn('w-3.5 h-3.5', starred && 'fill-current text-shodh-warning')} aria-hidden="true" />
                  </button>
                </div>
              );
            })}
          </div>
        ))}
        {view.llamaCppFile && !query.trim() && (
          <div className="mx-1 mt-2 px-2.5 py-2 rounded-lg border border-dashed border-shodh-border flex items-start gap-2 text-[11.5px] text-shodh-text-muted">
            <Cpu className="w-3.5 h-3.5 mt-0.5 shrink-0" aria-hidden="true" />
            <span>
              <span className="font-medium text-shodh-text-secondary">{view.llamaCppFile}</span> (llama.cpp) is not listed: the
              agent needs tool calling, which in-process models do not provide. Serve a local model with LM Studio to answer on this computer.
            </span>
          </div>
        )}
      </div>

      <p className="px-4 py-2 border-t border-shodh-border-subtle text-[11px] text-shodh-text-faint">
        {source ? `${source}. ` : ''}Models without tool calling are hidden. Keys stay in the system keychain.
      </p>
    </div>
  );
}
