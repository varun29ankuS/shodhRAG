import { useId, useState } from 'react';
import { Check, ChevronDown, Loader2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import type { ModelPickerHandle } from './modelApi';
import { ModelPickerPanel } from './ModelPicker';
import { BUTTON, FOCUS_RING } from './ConnectPanel';
import { PICK_HINTS, modelName, pickUnavailableText, sourceText } from './modelFormat';
import type { PickId, PickerError } from './modelTypes';
import { PROVIDER_LABELS, sameModel } from './modelTypes';

/**
 * Settings → Model → Model: the four picks (Best quality, Fast & cheap,
 * Free, Private), each resolved from the connected providers. Choosing one
 * makes it the default; when its model is rate-limited or unavailable the
 * next model of the same pick answers instead, without asking. "Show all
 * models" opens the searchable list of everything the connected providers
 * offer.
 */
export function ModelPicks({ picker }: { picker: ModelPickerHandle }) {
  const { view, choosePick } = picker;
  const [busy, setBusy] = useState<PickId | null>(null);
  const [refusal, setRefusal] = useState<(PickerError & { pick: PickId }) | null>(null);
  const [showAll, setShowAll] = useState(false);
  const groupId = useId();
  const allId = useId();

  if (!view) return null;

  const run = async (pick: PickId, sessionOverride: boolean) => {
    setBusy(pick);
    setRefusal(null);
    const error = await choosePick(pick, sessionOverride);
    setBusy(null);
    if (error) setRefusal({ ...error, pick });
    else notify.success(`${view.picks.find(p => p.pick === pick)?.label ?? 'The model'} will answer from now on`);
  };

  const activePick = view.active?.source === 'settings' ? view.prefs.pick : null;
  const source = sourceText(view.active);

  return (
    <div className="flex flex-col gap-3">
      <p className="m-0 text-[12.5px] text-shodh-text-muted max-w-[65ch]">
        {view.active
          ? `Answering with ${modelName(view.models, view.active.model)} (${PROVIDER_LABELS[view.active.model.provider]})${source ? `. ${source}` : ''}.`
          : 'Choose how answers are written. If a model is busy, the next one in its list answers instead.'}
      </p>
      <div role="radiogroup" aria-labelledby={groupId} className="grid grid-cols-1 sm:grid-cols-2 gap-2">
        <span id={groupId} className="sr-only">
          Model
        </span>
        {view.picks.map(option => {
          const available = option.model !== null;
          const selected = activePick === option.pick && sameModel(view.active?.model, option.model);
          return (
            <button
              key={option.pick}
              type="button"
              role="radio"
              aria-checked={selected}
              disabled={!available || busy !== null}
              onClick={() => void run(option.pick, false)}
              className={cn(
                'flex items-start gap-2.5 p-3 rounded-xl border text-left transition-colors duration-micro active:scale-[0.99]',
                selected ? 'border-shodh-accent bg-shodh-accent-soft' : 'border-shodh-border bg-shodh-ground hover:bg-shodh-raised',
                !available && 'opacity-60 cursor-not-allowed hover:bg-shodh-ground',
                FOCUS_RING,
              )}
            >
              <span
                className={cn(
                  'mt-0.5 w-4 h-4 shrink-0 rounded-full border inline-flex items-center justify-center',
                  selected ? 'border-shodh-accent bg-shodh-accent text-shodh-on-accent' : 'border-shodh-border-strong',
                )}
                aria-hidden="true"
              >
                {selected && <Check className="w-3 h-3" />}
              </span>
              <span className="min-w-0 flex flex-col gap-0.5">
                <span className="flex items-center gap-2 text-[13.5px] font-semibold text-shodh-text">
                  {option.label}
                  {busy === option.pick && <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
                </span>
                <span className="text-[12px] text-shodh-text-secondary truncate">
                  {available && option.model
                    ? `${option.name ?? option.model.model} · ${PROVIDER_LABELS[option.model.provider]}`
                    : pickUnavailableText(option.pick, view)}
                </span>
                <span className="text-[11.5px] text-shodh-text-muted">
                  {available && option.fallbacks.length > 0
                    ? `${PICK_HINTS[option.pick]} Then ${option.fallbacks.length === 1 ? '1 other model' : `${option.fallbacks.length} other models`} if busy.`
                    : PICK_HINTS[option.pick]}
                </span>
              </span>
            </button>
          );
        })}
      </div>
      {refusal && (
        <div role="alert" className="flex flex-wrap items-center gap-2 text-[12.5px] text-shodh-text-secondary">
          <span className="flex-1 min-w-[14rem]">
            {refusal.code === 'environment_set'
              ? 'The environment sets the model for this computer. Use this pick for this session only? The environment returns when Shodh restarts.'
              : refusal.message}
          </span>
          {refusal.code === 'environment_set' && (
            <button type="button" className={BUTTON} onClick={() => void run(refusal.pick, true)}>
              Use for this session
            </button>
          )}
        </div>
      )}
      <div>
        <button
          type="button"
          className={cn('inline-flex items-center gap-1 text-[12.5px] font-medium text-shodh-accent-text hover:underline rounded', FOCUS_RING)}
          aria-expanded={showAll}
          aria-controls={allId}
          onClick={() => setShowAll(s => !s)}
        >
          {showAll ? 'Hide the list' : 'Show all models'}
          <ChevronDown className={cn('w-3.5 h-3.5 transition-transform duration-micro', showAll && 'rotate-180')} aria-hidden="true" />
        </button>
      </div>
      {showAll && (
        <div id={allId} className="rounded-xl border border-shodh-border overflow-hidden">
          <ModelPickerPanel picker={picker} answerRunning={false} tall />
        </div>
      )}
    </div>
  );
}
