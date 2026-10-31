import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import type { ModelPickerHandle } from './modelApi';
import { modelApi } from './modelApi';
import { FOCUS_RING, INPUT } from './ConnectPanel';
import type { ChainCandidate, ChainPlan, FallbackChain, HealthTone } from './fallbackChain';
import { LatestOnly, healthWords, modelOptionValue, originLabel, privacyLabel, readModelOption } from './fallbackChain';
import type { ModelRef } from './modelTypes';
import { PROVIDER_LABELS, sameModel, toPickerError } from './modelTypes';

/** How often the chain's health is read again while the section is open. */
const REFRESH_MS = 30_000;

const DOT: Record<HealthTone, string> = {
  ok: 'bg-shodh-success',
  warn: 'bg-shodh-warning',
  down: 'bg-shodh-error',
  unknown: 'bg-shodh-text-faint',
};

/**
 * Settings → Model → Backup models: the models the app tries, in order, when the
 * chosen one cannot answer, each with its health, live uptime and what happens to
 * prompts; the backup the user picks; whether Ask switches without asking; whether a
 * free model may fall back to a paid one; and that connectors and background work
 * always switch by themselves.
 */
export function BackupModels({ picker }: { picker: ModelPickerHandle }) {
  const { view, setFallback, reload } = picker;
  const headingId = useId();
  const selectId = useId();
  const askId = useId();
  const paidId = useId();
  const [chain, setChain] = useState<FallbackChain | null>(null);
  const [saving, setSaving] = useState(false);
  const latest = useRef(new LatestOnly());

  const load = useCallback(() => {
    const ticket = latest.current.begin();
    modelApi
      .fallbackChain()
      .then(next => {
        // An older read that answers late never replaces a newer one.
        if (latest.current.isLatest(ticket)) setChain(next);
      })
      .catch(error => console.error('Backup models not read:', error));
  }, []);

  // Read again when the model or its backups change, and every half minute for health.
  const prefsKey = view
    ? JSON.stringify([view.active?.model ?? null, view.prefs.fallback, view.prefs.pick, view.prefs.alwaysFallBack, view.prefs.allowPaidBackups, view.connected])
    : '';
  useEffect(() => {
    if (!view) return;
    load();
    const timer = window.setInterval(load, REFRESH_MS);
    return () => window.clearInterval(timer);
    // `prefsKey` stands for the parts of `view` that change the chain.
  }, [load, prefsKey]);

  if (!view) return null;
  const { prefs } = view;
  // Any model of the list but the one in use.
  const choices = view.models
    .map(m => ({ ref: { provider: m.provider, model: m.id }, name: m.name }))
    .filter(m => !sameModel(m.ref, view.active?.model));

  const save = async (fallback: ModelRef | null, alwaysFallBack: boolean) => {
    setSaving(true);
    const refused = await setFallback(fallback, alwaysFallBack);
    setSaving(false);
    if (refused) notify.error('The backup was not saved', { description: refused.message });
    else load();
  };

  const saveAllowPaid = async (allow: boolean) => {
    setSaving(true);
    try {
      await modelApi.setAllowPaid(allow);
      await reload(false);
      load();
    } catch (error) {
      notify.error('The setting was not saved', { description: toPickerError(error).message });
    } finally {
      setSaving(false);
    }
  };

  const onPick = (value: string) => {
    if (!value) {
      void save(null, prefs.alwaysFallBack);
      return;
    }
    const model = readModelOption(value);
    if (model) void save(model, prefs.alwaysFallBack);
  };

  return (
    <section className="p-5 rounded-[14px] bg-shodh-surface border border-shodh-border flex flex-col gap-4" aria-labelledby={headingId}>
      <div className="flex flex-col gap-1">
        <h3 id={headingId} className="m-0 text-body-lg font-semibold text-shodh-text">
          Backup models
        </h3>
        <p className="m-0 text-small text-shodh-text-muted max-w-[65ch]">
          When a model can&apos;t answer (it is down, overloaded or out of free requests), Shodh tries the next one in this list. A model that just failed is skipped for a few minutes.
        </p>
      </div>

      <ChainList chain={chain?.plan ?? null} nowMs={chain?.nowMs ?? Date.now()} localOnly={chain?.localOnly ?? view.localOnly} />

      {chain?.direct && chain.plan && !samePlan(chain.direct, chain.plan) && (
        <div className="flex flex-col gap-2">
          <p className="m-0 text-small font-medium text-shodh-text-secondary">For Slack, Telegram and background work</p>
          <p className="m-0 -mt-1 text-caption text-shodh-text-muted max-w-[65ch]">
            These answer without the assistant, so a subscription (Claude Pro, ChatGPT Plus…) can&apos;t answer them. They also never switch to a model less private than yours, not even your own backup.
          </p>
          <ChainList chain={chain.direct} nowMs={chain.nowMs} localOnly={chain.localOnly} />
        </div>
      )}

      <div className="flex flex-col gap-1.5 max-w-[30rem]">
        <label htmlFor={selectId} className="text-small font-medium text-shodh-text">
          Your backup
        </label>
        <select
          id={selectId}
          className={cn(INPUT, 'w-full')}
          disabled={saving}
          value={prefs.fallback ? modelOptionValue(prefs.fallback) : ''}
          onChange={e => onPick(e.target.value)}
        >
          <option value="">Let Shodh choose</option>
          {prefs.fallback && !choices.some(m => sameModel(m.ref, prefs.fallback)) && (
            <option value={modelOptionValue(prefs.fallback)}>{prefs.fallback.model}</option>
          )}
          {choices.map(m => (
            <option key={modelOptionValue(m.ref)} value={modelOptionValue(m.ref)}>
              {`${m.name} (${PROVIDER_LABELS[m.ref.provider]})`}
            </option>
          ))}
        </select>
        <p className="m-0 text-caption text-shodh-text-muted">Tried first when your model can&apos;t answer.</p>
      </div>

      <div className="flex items-start gap-2.5">
        <input
          id={askId}
          type="checkbox"
          className={cn('mt-0.5 w-4 h-4 rounded', FOCUS_RING)}
          checked={prefs.alwaysFallBack}
          disabled={saving}
          onChange={e => void save(prefs.fallback, e.target.checked)}
        />
        <label htmlFor={askId} className="flex flex-col gap-0.5">
          <span className="text-small text-shodh-text">In Ask, switch to a backup without asking</span>
          <span className="text-caption text-shodh-text-muted">
            Off: Ask shows what failed and offers the backup as a button. Either way the answer says which model wrote it.
          </span>
        </label>
      </div>

      <div className="flex items-start gap-2.5">
        <input
          id={paidId}
          type="checkbox"
          className={cn('mt-0.5 w-4 h-4 rounded', FOCUS_RING)}
          checked={prefs.allowPaidBackups}
          disabled={saving}
          onChange={e => void saveAllowPaid(e.target.checked)}
        />
        <label htmlFor={paidId} className="flex flex-col gap-0.5">
          <span className="text-small text-shodh-text">Allow paid backups</span>
          <span className="text-caption text-shodh-text-muted">
            Off: a free model only falls back to other free or local models, and when today&apos;s free quota is used up Shodh says so and when it resets. On: it may use a paid model or an API key, which bills that provider per request, typically a fraction of a cent to a few cents an answer.
          </span>
        </label>
      </div>

      <p className="m-0 text-small text-shodh-text-secondary max-w-[65ch]" role="note">
        Slack, Telegram, memory learning and other background tasks always switch to a backup on their own, since nobody is there to ask. Their conversations in Shodh say which model answered.
      </p>
    </section>
  );
}

function samePlan(a: ChainPlan, b: ChainPlan): boolean {
  const ids = (p: ChainPlan) => [p.requested, ...p.fallbacks].map(c => modelOptionValue(c.model)).join('|');
  return ids(a) === ids(b);
}

function ChainList({ chain, nowMs, localOnly }: { chain: ChainPlan | null; nowMs: number; localOnly: boolean }) {
  if (!chain) {
    return <p className="m-0 text-small text-shodh-text-muted">Choose a model above to see its backups.</p>;
  }
  const rows: ChainCandidate[] = [chain.requested, ...chain.fallbacks];
  return (
    <div className="flex flex-col gap-2">
      <ol className="m-0 p-0 list-none flex flex-col" aria-label="Models tried, in order">
        {rows.map((c, i) => {
          const health = healthWords(c, nowMs);
          const last = i === rows.length - 1;
          return (
            <li key={modelOptionValue(c.model)} className="relative flex gap-3 pb-3 last:pb-0">
              {/* The route the answer takes: a dot per model, a line to the next one. */}
              <span className="relative flex flex-col items-center w-3 shrink-0" aria-hidden="true">
                <span className={cn('mt-[5px] w-2.5 h-2.5 rounded-full ring-2 ring-shodh-surface', DOT[health.tone])} />
                {!last && <span className="flex-1 w-px mt-1 bg-shodh-border" />}
              </span>
              <span className="min-w-0 flex flex-col gap-0.5">
                <span className="flex flex-wrap items-baseline gap-x-2 text-small">
                  <span className={cn('font-medium text-shodh-text', i > 0 && !c.reachable && 'line-through text-shodh-text-muted')}>{c.name}</span>
                  <span className="text-caption text-shodh-text-muted">
                    {`${originLabel(c.origin)}, ${PROVIDER_LABELS[c.model.provider]}, ${privacyLabel(c.privacy)}${c.paid ? ', paid' : ''}`}
                  </span>
                </span>
                <span className="text-caption text-shodh-text-muted">
                  {c.reachable ? health.text : 'Answers only in Ask'}
                  <span className="sr-only">{` (${health.tone === 'ok' ? 'healthy' : health.tone === 'unknown' ? 'not checked' : 'having problems'})`}</span>
                </span>
              </span>
            </li>
          );
        })}
      </ol>
      {chain.fallbacks.length === 0 && (
        <p className="m-0 text-small text-shodh-text-muted">
          {localOnly
            ? 'Local-only mode is on, and no other model runs on this computer. Load another model in LM Studio to have a backup.'
            : 'No backup is available yet. Connect another provider above, or choose a backup below.'}
        </p>
      )}
    </div>
  );
}
