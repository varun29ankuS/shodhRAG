import { useId, useState } from 'react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { SwitchRow } from '../../components/PrivacySettings';
import { useModelPicker } from './modelApi';
import { ModelPickerPanel } from './ModelPicker';
import { modelKey, modelName, sourceText } from './modelFormat';
import type { ModelRef } from './modelTypes';
import { PROVIDER_LABELS } from './modelTypes';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

const AUTOMATIC = '';

/**
 * Settings → Model: the model picker (the same list as the Ask chip) and
 * what happens when a model is rate-limited or unavailable: which model is
 * offered instead, and whether to use it without asking.
 */
export function ModelSettingsSection({ onOpenKeys }: { onOpenKeys: () => void }) {
  const picker = useModelPicker();
  const { view, setFallback } = picker;
  const [saving, setSaving] = useState(false);
  const fallbackId = useId();
  const fallbackHelpId = useId();

  const saveFallback = async (fallback: ModelRef | null, always: boolean) => {
    setSaving(true);
    const error = await setFallback(fallback, always);
    setSaving(false);
    if (error) notify.error('The fallback was not saved', { description: error.message });
  };

  const prefs = view?.prefs ?? null;
  const fallbackValue = prefs?.fallback ? modelKey(prefs.fallback) : AUTOMATIC;
  const options = (view?.models ?? []).filter(m => m.tools !== 'no');
  // Keep a saved fallback selectable even when it is not listed right now (offline, key removed).
  const savedMissing = prefs?.fallback && !options.some(m => m.provider === prefs.fallback?.provider && m.id === prefs.fallback?.model);
  const source = sourceText(view?.active ?? null);

  return (
    <div className="flex flex-col gap-5">
      <section className="rounded-[14px] bg-shodh-surface border border-shodh-border overflow-hidden" aria-labelledby={`${fallbackId}-model`}>
        <header className="px-5 pt-4 pb-1">
          <h3 id={`${fallbackId}-model`} className="m-0 text-[15px] font-semibold text-shodh-text">Model for answers</h3>
          <p className="m-0 mt-0.5 text-[12.5px] text-shodh-text-muted">
            {view?.active
              ? `${modelName(view.models, view.active.model)} (${PROVIDER_LABELS[view.active.model.provider]})${source ? ` · ${source}` : ''}. A new choice applies to the next answer; no restart.`
              : 'No model is chosen yet. Add a provider key below, or start Ollama for local models, then choose one.'}
          </p>
        </header>
        <ModelPickerPanel picker={picker} answerRunning={false} onOpenSettings={onOpenKeys} tall />
      </section>

      <section className="p-5 rounded-[14px] bg-shodh-surface border border-shodh-border flex flex-col gap-4" aria-labelledby={`${fallbackId}-heading`}>
        <div>
          <h3 id={`${fallbackId}-heading`} className="m-0 text-[15px] font-semibold text-shodh-text">When a model is rate-limited or unavailable</h3>
          <p className="m-0 mt-0.5 text-[12.5px] text-shodh-text-muted">
            The answer shows what happened with Retry, Use the fallback for that answer, and Change model. Nothing switches without your click unless you turn on automatic fallback.
          </p>
        </div>
        <div className="flex flex-col gap-1.5">
          <label htmlFor={fallbackId} className="text-[13px] font-semibold text-shodh-text">
            Fallback model
          </label>
          <select
            id={fallbackId}
            aria-describedby={fallbackHelpId}
            value={fallbackValue}
            disabled={!view || saving}
            onChange={e => {
              const key = e.target.value;
              const model = key === AUTOMATIC ? null : options.find(m => modelKey({ provider: m.provider, model: m.id }) === key);
              const ref = model ? { provider: model.provider, model: model.id } : key === AUTOMATIC ? null : prefs?.fallback ?? null;
              void saveFallback(ref, prefs?.alwaysFallBack ?? false);
            }}
            className={cn('h-9 max-w-[420px] px-2.5 rounded-lg border border-shodh-border bg-shodh-ground text-[13px] text-shodh-text', FOCUS_RING)}
          >
            <option value={AUTOMATIC}>Choose automatically (a fast model you have a key for)</option>
            {savedMissing && prefs?.fallback && (
              <option value={modelKey(prefs.fallback)}>{`${prefs.fallback.model} (not available now)`}</option>
            )}
            {options.map(m => (
              <option key={modelKey({ provider: m.provider, model: m.id })} value={modelKey({ provider: m.provider, model: m.id })}>
                {`${m.name} · ${PROVIDER_LABELS[m.provider]}`}
              </option>
            ))}
          </select>
          <p id={fallbackHelpId} className="m-0 text-[12px] text-shodh-text-muted">
            Stealth models are offered as a fallback only after you have accepted them in the list above. In Local-only mode only local models are offered.
          </p>
        </div>
        <SwitchRow
          label="Always fall back"
          description="Retry a rate-limited or unavailable answer with the fallback model at once, without asking. The answer still says which model wrote it, and the switch is recorded in Activity."
          checked={prefs?.alwaysFallBack ?? false}
          disabled={!view || saving}
          onChange={always => void saveFallback(prefs?.fallback ?? null, always)}
        />
      </section>
    </div>
  );
}
