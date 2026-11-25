/**
 * Automations: instructions Shodh runs on a schedule or when the user starts one.
 *
 * - The list on the left: one fixed-height row per automation, with when it runs, its
 *   state chip (one width for every state), its last run, and Run now and Pause/Resume
 *   in slots that never move.
 * - The pane on the right (fixed width): the selected automation and its runs, a run's
 *   steps and the changes it waits on (a checkbox each), or the form for a new one.
 * - The header: the model automations use, tagged with whose limit it counts against.
 * - The footer: today's model calls.
 */
import React, { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Loader2, Pause, Play, Plus, RotateCcw } from 'lucide-react';
import { cn } from '../../lib/utils';
import { APP_SETTINGS_CHANGED } from '../../lib/appSettings';
import { useNavigationTarget } from '../agent/useNavigationTarget';
import { CallsToday } from '../modelPicker/CallsToday';
import { modelApi, MODEL_CHANGED_EVENT } from '../modelPicker/modelApi';
import type { ModelRef, PickerView } from '../modelPicker/modelTypes';
import { workspacesApi } from '../workspaces/api';
import type { WorkspaceListing } from '../workspaces/types';
import { AUTOMATIONS_CHANGED_EVENT, automationsApi, errorText } from './api';
import {
  DEFAULT_WHEN,
  WEEK_DAYS,
  WEEK_DAY_LABELS,
  callsText,
  estimateText,
  formOf,
  limitTag,
  modelKey,
  runOutcome,
  scheduleOf,
  shortTime,
  stateChip,
  whyRan,
} from './model';
import type { Automation, Run, RunDetail, Tone, WhenForm, WhenKind } from './model';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

const BUTTON = cn(
  'h-8 px-3 inline-flex items-center justify-center gap-1.5 rounded-lg text-small whitespace-nowrap shrink-0 transition-colors duration-micro disabled:opacity-50',
  FOCUS_RING,
);
const QUIET = cn(BUTTON, 'border border-shodh-border text-shodh-text-secondary hover:bg-shodh-raised');
const PRIMARY = cn(BUTTON, 'bg-shodh-accent text-shodh-on-accent font-semibold hover:bg-shodh-accent-hover');
const FIELD = cn('rounded-lg border border-shodh-border bg-shodh-ground px-3 text-small text-shodh-text', FOCUS_RING);

const TONE: Record<Tone, string> = {
  neutral: 'bg-shodh-raised text-shodh-text-secondary',
  active: 'bg-shodh-raised text-shodh-info',
  waiting: 'bg-shodh-raised text-shodh-text-secondary',
  needs_you: 'bg-shodh-raised text-shodh-warning',
  warning: 'bg-shodh-raised text-shodh-error',
  off: 'bg-shodh-raised text-shodh-text-faint',
};

const WHEN_LABELS: Record<WhenKind, string> = {
  manual: 'Only when I start it',
  daily: 'Every day',
  weekdays: 'Weekdays',
  weekly: 'Weekly',
  monthly: 'Monthly',
  every_hours: 'Every few hours',
};

type Pane = { kind: 'none' } | { kind: 'new' } | { kind: 'edit'; id: string } | { kind: 'detail'; id: string; runId: string | null };

/** The models the automations may use: what the picker already offers, without repeats. */
function modelChoices(view: PickerView | null): { model: ModelRef; name: string }[] {
  if (!view) return [];
  const seen = new Set<string>();
  const out: { model: ModelRef; name: string }[] = [];
  const add = (model: ModelRef | null, name?: string | null) => {
    if (!model || seen.has(modelKey(model))) return;
    seen.add(modelKey(model));
    out.push({ model, name: name || model.model });
  };
  for (const pick of view.picks) add(pick.model, pick.name ? `${pick.label}: ${pick.name}` : null);
  for (const model of view.prefs.favourites) add(model);
  for (const model of view.prefs.recent) add(model);
  for (const m of view.models) if (m.provider === 'lmstudio') add({ provider: m.provider, model: m.id }, m.name);
  return out;
}

function ModelControl({ chosen, onChange }: { chosen: ModelRef | null; onChange: (model: ModelRef | null) => Promise<void> }) {
  const id = useId();
  const [view, setView] = useState<PickerView | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    const load = () => modelApi.view().then(v => live && setView(v)).catch(() => live && setView(null));
    void load();
    const reload = () => void load();
    window.addEventListener(MODEL_CHANGED_EVENT, reload);
    return () => {
      live = false;
      window.removeEventListener(MODEL_CHANGED_EVENT, reload);
    };
  }, []);
  const choices = useMemo(() => {
    const list = modelChoices(view);
    if (chosen && !list.some(c => modelKey(c.model) === modelKey(chosen))) list.unshift({ model: chosen, name: chosen.model });
    return list;
  }, [view, chosen]);
  const chat = view?.active?.model ?? null;
  const tag = limitTag(chosen, chat);
  return (
    <div className="flex items-center gap-2 min-w-0">
      <label htmlFor={id} className="text-small text-shodh-text-muted shrink-0">Model</label>
      <select
        id={id}
        value={modelKey(chosen)}
        disabled={saving}
        onChange={e => {
          const next = choices.find(c => modelKey(c.model) === e.target.value)?.model ?? null;
          setSaving(true);
          setError(null);
          onChange(next)
            .catch(err => setError(errorText(err)))
            .finally(() => setSaving(false));
        }}
        className={cn(FIELD, 'h-8 w-56 truncate')}
      >
        <option value="">Same as chat</option>
        {choices.map(c => (
          <option key={modelKey(c.model)} value={modelKey(c.model)}>{c.name}</option>
        ))}
      </select>
      <span
        className="w-36 truncate text-caption text-shodh-text-secondary"
        title={error ?? tag.tooltip}
        role={error ? 'alert' : undefined}
      >
        {error ? 'Not changed' : tag.label}
      </span>
    </div>
  );
}

function Chip({ automation, now }: { automation: Automation; now: number }) {
  const chip = stateChip(automation, now);
  return (
    <span
      className={cn('w-24 h-6 shrink-0 inline-flex items-center justify-center rounded-full text-caption font-medium truncate', TONE[chip.tone])}
      title={chip.tooltip}
    >
      {chip.label}
    </span>
  );
}

function Row({
  automation,
  selected,
  now,
  onSelect,
  onChanged,
  onStarted,
}: {
  automation: Automation;
  selected: boolean;
  now: number;
  onSelect: () => void;
  onChanged: (message: string | null) => void;
  /** A run the user started: its conversation opens in Ask. */
  onStarted: (run: Run) => void;
}) {
  const [busy, setBusy] = useState<'run' | 'toggle' | null>(null);
  const running = automation.state === 'running' || automation.active !== null;
  const off = automation.state === 'off' || automation.state === 'on_hold';
  const act = (which: 'run' | 'toggle', work: () => Promise<unknown>) => {
    setBusy(which);
    work()
      .then(() => onChanged(null))
      .catch(e => onChanged(errorText(e)))
      .finally(() => setBusy(null));
  };
  const last = automation.last ? runOutcome(automation.last) : 'Not run yet';
  return (
    <li
      className={cn(
        'h-12 flex items-center gap-3 px-3 rounded-lg border',
        selected ? 'border-shodh-accent bg-shodh-accent-soft' : 'border-transparent hover:bg-shodh-raised',
      )}
    >
      <button type="button" onClick={onSelect} className={cn('flex-1 min-w-0 flex items-center gap-3 text-left h-full rounded-md', FOCUS_RING)}>
        <span className="w-44 shrink-0 truncate text-small font-medium text-shodh-text" title={automation.name}>{automation.name}</span>
        <span className="w-48 shrink-0 truncate text-small text-shodh-text-secondary" title={automation.when}>{automation.when}</span>
        <Chip automation={automation} now={now} />
        <span className="flex-1 min-w-0 truncate text-small text-shodh-text-muted" title={last}>{last}</span>
      </button>
      <button
        type="button"
        className={cn(QUIET, 'w-24')}
        disabled={running || off || busy !== null}
        title={off ? 'Turn it on first' : running ? 'A run is going' : 'Run it now'}
        onClick={() => act('run', () => automationsApi.runNow(automation.id).then(onStarted))}
      >
        {busy === 'run' || running ? <Loader2 className="w-3.5 h-3.5 animate-spin" aria-hidden="true" /> : <Play className="w-3.5 h-3.5" aria-hidden="true" />}
        {running ? 'Running' : 'Run now'}
      </button>
      <button
        type="button"
        className={cn(QUIET, 'w-24')}
        disabled={busy !== null}
        onClick={() => act('toggle', () => (off ? automationsApi.resume(automation.id) : automationsApi.pause(automation.id)))}
      >
        {busy === 'toggle' ? (
          <Loader2 className="w-3.5 h-3.5 animate-spin" aria-hidden="true" />
        ) : off ? (
          <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" />
        ) : (
          <Pause className="w-3.5 h-3.5" aria-hidden="true" />
        )}
        {off ? 'Resume' : 'Pause'}
      </button>
    </li>
  );
}

function AutomationForm({
  initial,
  workspaces,
  onSaved,
  onCancel,
}: {
  initial: Automation | null;
  workspaces: WorkspaceListing[];
  onSaved: (automation: Automation) => void;
  onCancel: () => void;
}) {
  const nameId = useId();
  const textId = useId();
  const whenId = useId();
  const atId = useId();
  const whereId = useId();
  const [name, setName] = useState(initial?.name ?? '');
  const [instruction, setInstruction] = useState(initial?.instruction ?? '');
  const [when, setWhen] = useState<WhenForm>(initial ? formOf(initial.schedule) : DEFAULT_WHEN);
  const [workspaceId, setWorkspaceId] = useState<string>(initial?.workspaceId ?? '');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const planned = scheduleOf(when, new Date().toISOString());
  const schedule = 'schedule' in planned ? planned.schedule : null;
  const save = async () => {
    if ('error' in planned) {
      setError(planned.error);
      return;
    }
    setSaving(true);
    setError(null);
    const draft = { name, instruction, schedule: planned.schedule, workspaceId: workspaceId || null };
    try {
      onSaved(initial ? await automationsApi.update(initial.id, draft) : await automationsApi.create(draft));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setSaving(false);
    }
  };
  return (
    <form
      className="flex flex-col gap-3"
      aria-label={initial ? 'Change automation' : 'New automation'}
      onSubmit={e => {
        e.preventDefault();
        void save();
      }}
    >
      <h2 className="m-0 text-body-lg font-semibold text-shodh-text">{initial ? 'Change automation' : 'New automation'}</h2>
      <label htmlFor={nameId} className="text-small font-medium text-shodh-text">Name</label>
      <input id={nameId} value={name} maxLength={80} onChange={e => setName(e.target.value)} className={cn(FIELD, 'h-9')} />
      <label htmlFor={textId} className="text-small font-medium text-shodh-text">What should Shodh do?</label>
      <textarea
        id={textId}
        value={instruction}
        maxLength={4000}
        rows={5}
        onChange={e => setInstruction(e.target.value)}
        className={cn(FIELD, 'py-2 resize-none')}
      />
      <label htmlFor={whenId} className="text-small font-medium text-shodh-text">When</label>
      <div className="flex items-center gap-2">
        <select id={whenId} value={when.kind} onChange={e => setWhen({ ...when, kind: e.target.value as WhenKind })} className={cn(FIELD, 'h-9 w-48')}>
          {(Object.keys(WHEN_LABELS) as WhenKind[]).map(k => (
            <option key={k} value={k}>{WHEN_LABELS[k]}</option>
          ))}
        </select>
        {when.kind !== 'manual' && when.kind !== 'every_hours' && (
          <input
            id={atId}
            type="time"
            aria-label="At"
            value={when.at}
            onChange={e => setWhen({ ...when, at: e.target.value })}
            className={cn(FIELD, 'h-9 w-28')}
          />
        )}
        {when.kind === 'every_hours' && (
          <input
            type="number"
            aria-label="Hours"
            min={1}
            max={24}
            value={when.hours}
            onChange={e => setWhen({ ...when, hours: Number(e.target.value) })}
            className={cn(FIELD, 'h-9 w-20')}
          />
        )}
        {when.kind === 'monthly' && (
          <select
            aria-label="Day"
            value={String(when.day)}
            onChange={e => setWhen({ ...when, day: e.target.value === 'last' ? 'last' : Number(e.target.value) })}
            className={cn(FIELD, 'h-9 w-24')}
          >
            {Array.from({ length: 31 }, (_, i) => i + 1).map(d => (
              <option key={d} value={d}>Day {d}</option>
            ))}
            <option value="last">Last day</option>
          </select>
        )}
      </div>
      {when.kind === 'weekly' && (
        <div className="flex gap-1" role="group" aria-label="Days">
          {WEEK_DAYS.map(d => {
            const on = when.days.includes(d);
            return (
              <button
                key={d}
                type="button"
                aria-pressed={on}
                onClick={() => setWhen({ ...when, days: on ? when.days.filter(x => x !== d) : [...when.days, d] })}
                className={cn(BUTTON, 'h-7 w-11 px-0 border', on ? 'border-shodh-accent bg-shodh-accent-soft text-shodh-text' : 'border-shodh-border text-shodh-text-secondary')}
              >
                {WEEK_DAY_LABELS[d]}
              </button>
            );
          })}
        </div>
      )}
      <label htmlFor={whereId} className="text-small font-medium text-shodh-text">Where</label>
      <select id={whereId} value={workspaceId} onChange={e => setWorkspaceId(e.target.value)} className={cn(FIELD, 'h-9')}>
        <option value="">Whole library</option>
        {workspaces.filter(w => !w.archived).map(w => (
          <option key={w.id} value={w.id}>{w.name}</option>
        ))}
      </select>
      <p className="m-0 text-caption text-shodh-text-muted">{estimateText(schedule, 4)}. Changes wait for your OK.</p>
      <p className="m-0 min-h-[1.25rem] text-caption text-shodh-warning" role="alert">{error ?? ''}</p>
      <div className="flex justify-end gap-2">
        <button type="button" className={cn(QUIET, 'w-24')} onClick={onCancel}>Cancel</button>
        <button type="submit" className={cn(PRIMARY, 'w-24')} disabled={saving || !name.trim() || !instruction.trim()}>
          {saving ? <Loader2 className="w-3.5 h-3.5 animate-spin" aria-hidden="true" /> : 'Save'}
        </button>
      </div>
    </form>
  );
}

function RunDetailView({ runId, onOpenChat }: { runId: string; onOpenChat: (conversationId: string) => void }) {
  const [run, setRun] = useState<RunDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [chosen, setChosen] = useState<number[] | null>(null);
  const [deciding, setDeciding] = useState(false);
  const load = useCallback(() => {
    automationsApi
      .run(runId)
      .then(r => {
        setRun(r);
        setError(null);
      })
      .catch(e => setError(errorText(e)));
  }, [runId]);
  useEffect(() => {
    load();
    let unlisten: (() => void) | null = null;
    let live = true;
    listen(AUTOMATIONS_CHANGED_EVENT, () => load())
      .then(fn => (live ? (unlisten = fn) : fn()))
      .catch(() => undefined);
    return () => {
      live = false;
      unlisten?.();
    };
  }, [load]);
  if (error) return <p className="m-0 text-small text-shodh-warning" role="alert">{error}</p>;
  if (!run) return <p className="m-0 text-small text-shodh-text-muted" role="status">Loading the run…</p>;
  const approval = run.approval;
  const open = approval && approval.decision === null;
  const picked = chosen ?? approval?.writes.map((_, i) => i) ?? [];
  const decide = (indices: number[]) => {
    if (!approval) return;
    setDeciding(true);
    automationsApi
      .decide(approval.id, indices)
      .then(r => {
        setRun(r);
        setChosen(null);
      })
      .catch(e => setError(errorText(e)))
      .finally(() => setDeciding(false));
  };
  return (
    <section aria-label="Run" className="flex flex-col gap-3">
      <div className="flex items-center justify-between gap-2">
        <p className="m-0 text-small text-shodh-text-secondary truncate" title={whyRan(run)}>{whyRan(run)}</p>
        <span className="text-caption text-shodh-text-muted shrink-0">{callsText(run)}</span>
      </div>
      <p className="m-0 text-small text-shodh-text">{runOutcome(run)}</p>
      <ol className="m-0 p-0 list-none flex flex-col gap-1">
        {run.steps.map(step => (
          <li key={step.idx} className="h-7 flex items-center gap-2 text-small text-shodh-text-secondary">
            <span className="w-16 shrink-0 font-medium text-shodh-text">{step.kind === 'notify' ? 'Told you' : 'Agent'}</span>
            <span className="flex-1 truncate" title={String(step.output.summary ?? '')}>{String(step.output.summary ?? step.state)}</span>
            {step.kind === 'agent' && <span className="shrink-0 text-caption text-shodh-text-muted">{step.calls} calls</span>}
          </li>
        ))}
      </ol>
      {approval && (
        <div className="flex flex-col gap-2 rounded-lg border border-shodh-border p-3" role="group" aria-label={approval.title}>
          <p className="m-0 text-small font-medium text-shodh-text">{approval.title}</p>
          <ul className="m-0 p-0 list-none flex flex-col gap-1">
            {approval.writes.map((w, i) => (
              <li key={`${w.stepId}-${i}`} className="flex items-center gap-2 text-small">
                <input
                  type="checkbox"
                  aria-label={w.label}
                  disabled={!open || deciding}
                  checked={picked.includes(i)}
                  onChange={e => setChosen(e.target.checked ? [...picked, i] : picked.filter(x => x !== i))}
                />
                <span className="truncate text-shodh-text-secondary" title={w.label}>{w.label}</span>
              </li>
            ))}
          </ul>
          {open ? (
            <div className="flex justify-end gap-2">
              <button type="button" className={cn(QUIET, 'w-24')} disabled={deciding} onClick={() => decide([])}>Deny</button>
              <button type="button" className={cn(PRIMARY, 'w-36')} disabled={deciding || picked.length === 0} onClick={() => decide(picked)}>
                {deciding ? <Loader2 className="w-3.5 h-3.5 animate-spin" aria-hidden="true" /> : 'Approve selected'}
              </button>
            </div>
          ) : (
            <p className="m-0 text-caption text-shodh-text-muted">
              {approval.decision === 'expired' ? 'Expired' : approval.decision === 'denied' ? 'Declined' : 'Decided'}
            </p>
          )}
        </div>
      )}
      {run.conversationId && (
        <button type="button" className={cn(QUIET, 'self-start w-40')} onClick={() => onOpenChat(run.conversationId as string)}>
          Open conversation
        </button>
      )}
    </section>
  );
}

function Detail({
  automation,
  runId,
  now,
  onRun,
  onEdit,
  onDeleted,
  onOpenChat,
}: {
  automation: Automation;
  runId: string | null;
  now: number;
  onRun: (id: string | null) => void;
  onEdit: () => void;
  onDeleted: () => void;
  onOpenChat: (conversationId: string) => void;
}) {
  const [runs, setRuns] = useState<Run[]>([]);
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    const load = () => automationsApi.runs(automation.id).then(r => live && setRuns(r)).catch(e => live && setError(errorText(e)));
    void load();
    let unlisten: (() => void) | null = null;
    listen(AUTOMATIONS_CHANGED_EVENT, () => void load())
      .then(fn => (live ? (unlisten = fn) : fn()))
      .catch(() => undefined);
    return () => {
      live = false;
      unlisten?.();
    };
  }, [automation.id]);
  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center gap-2">
        <h2 className="m-0 flex-1 min-w-0 truncate text-body-lg font-semibold text-shodh-text" title={automation.name}>{automation.name}</h2>
        <button type="button" className={cn(QUIET, 'w-20')} onClick={onEdit}>Edit</button>
        <button
          type="button"
          className={cn(QUIET, 'w-28', confirming && 'border-shodh-error text-shodh-error')}
          onClick={() => {
            if (!confirming) {
              setConfirming(true);
              return;
            }
            automationsApi.remove(automation.id).then(onDeleted).catch(e => setError(errorText(e)));
          }}
          onBlur={() => setConfirming(false)}
        >
          {confirming ? 'Delete it' : 'Delete'}
        </button>
      </div>
      <p className="m-0 text-small text-shodh-text-secondary">{automation.when}</p>
      <p className="m-0 text-small text-shodh-text max-h-24 overflow-hidden" title={automation.instruction}>{automation.instruction}</p>
      <p className="m-0 min-h-[1.25rem] text-caption text-shodh-warning" role="alert">{error ?? ''}</p>
      <h3 className="m-0 text-small font-semibold text-shodh-text">Runs</h3>
      {runs.length === 0 ? (
        <p className="m-0 text-small text-shodh-text-muted">Not run yet</p>
      ) : (
        <ul className="m-0 p-0 list-none flex flex-col gap-px">
          {runs.map(run => (
            <li key={run.id}>
              <button
                type="button"
                onClick={() => onRun(run.id === runId ? null : run.id)}
                aria-expanded={run.id === runId}
                className={cn(
                  'w-full h-8 flex items-center gap-2 px-2 rounded-md text-left text-small',
                  run.id === runId ? 'bg-shodh-raised' : 'hover:bg-shodh-raised',
                  FOCUS_RING,
                )}
              >
                <span className="w-32 shrink-0 text-shodh-text-secondary">{shortTime(run.createdAt, now)}</span>
                <span className="flex-1 truncate text-shodh-text" title={runOutcome(run)}>{runOutcome(run)}</span>
              </button>
              {run.id === runId && (
                <div className="px-2 py-3">
                  <RunDetailView runId={run.id} onOpenChat={onOpenChat} />
                </div>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

export function AutomationsView({ onOpenChat }: { onOpenChat: (conversationId: string) => void }) {
  const [automations, setAutomations] = useState<Automation[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [rowError, setRowError] = useState<string | null>(null);
  const [pane, setPane] = useState<Pane>({ kind: 'none' });
  const [model, setModel] = useState<ModelRef | null>(null);
  const [workspaces, setWorkspaces] = useState<WorkspaceListing[]>([]);
  const [now, setNow] = useState(() => Date.now());

  const load = useCallback(() => {
    automationsApi
      .list()
      .then(list => {
        setAutomations(list);
        setLoadError(null);
      })
      .catch(e => setLoadError(errorText(e)));
  }, []);

  const loadModel = useCallback(() => {
    invoke<{ automations?: { model?: ModelRef | null } }>('get_app_settings')
      .then(s => setModel(s?.automations?.model ?? null))
      .catch(() => setModel(null));
  }, []);

  useEffect(() => {
    load();
    loadModel();
    workspacesApi.list().then(setWorkspaces).catch(() => setWorkspaces([]));
    const timer = window.setInterval(() => setNow(Date.now()), 60_000);
    const unlisteners: (() => void)[] = [];
    let live = true;
    for (const [event, handler] of [
      [AUTOMATIONS_CHANGED_EVENT, load],
      [APP_SETTINGS_CHANGED, loadModel],
    ] as const) {
      listen(event, () => handler())
        .then(fn => (live ? unlisteners.push(fn) : fn()))
        .catch(() => undefined);
    }
    return () => {
      live = false;
      window.clearInterval(timer);
      unlisteners.forEach(fn => fn());
    };
  }, [load, loadModel]);

  useNavigationTarget('automation', target => {
    setPane({ kind: 'detail', id: target.automationId, runId: target.runId });
    load();
  });

  const selectedId = pane.kind === 'detail' || pane.kind === 'edit' ? pane.id : null;
  const selected = automations?.find(a => a.id === selectedId) ?? null;

  return (
    <div className="h-full flex flex-col bg-shodh-ground">
      <header className="h-14 shrink-0 flex items-center gap-4 px-6 border-b border-shodh-border">
        <h1 className="m-0 text-body-lg font-semibold text-shodh-text">Automations</h1>
        <div className="flex-1" />
        <ModelControl
          chosen={model}
          onChange={async next => {
            await automationsApi.setModel(next);
            setModel(next);
          }}
        />
        <button type="button" className={cn(PRIMARY, 'w-40')} onClick={() => setPane({ kind: 'new' })}>
          <Plus className="w-3.5 h-3.5" aria-hidden="true" />
          New automation
        </button>
      </header>
      <div className="flex-1 min-h-0 flex">
        <div className="flex-1 min-w-0 flex flex-col">
          <div className="flex-1 min-h-0 overflow-y-auto scrollbar-thin px-4 py-3">
            {loadError ? (
              <p className="m-0 text-small text-shodh-warning" role="alert">{loadError}</p>
            ) : automations === null ? (
              <p className="m-0 text-small text-shodh-text-muted" role="status">Loading…</p>
            ) : automations.length === 0 ? (
              <div className="h-full flex flex-col items-center justify-center gap-3">
                <p className="m-0 text-small text-shodh-text-muted">No automations yet</p>
                <button type="button" className={cn(PRIMARY, 'w-40')} onClick={() => setPane({ kind: 'new' })}>
                  <Plus className="w-3.5 h-3.5" aria-hidden="true" />
                  New automation
                </button>
              </div>
            ) : (
              <ul className="m-0 p-0 list-none flex flex-col gap-1" aria-label="Automations">
                {automations.map(a => (
                  <Row
                    key={a.id}
                    automation={a}
                    selected={a.id === selectedId}
                    now={now}
                    onSelect={() => setPane({ kind: 'detail', id: a.id, runId: null })}
                    onChanged={message => {
                      setRowError(message);
                      load();
                    }}
                    onStarted={run => onOpenChat(run.conversationId ?? `auto-${run.id}`)}
                  />
                ))}
              </ul>
            )}
          </div>
          <p className="m-0 h-6 shrink-0 px-6 text-caption text-shodh-warning truncate" role="alert">{rowError ?? ''}</p>
          <footer className="h-10 shrink-0 flex items-center px-6 border-t border-shodh-border">
            <CallsToday />
          </footer>
        </div>
        <aside className="w-[420px] shrink-0 border-l border-shodh-border overflow-y-auto scrollbar-thin p-5" aria-label="Automation">
          {pane.kind === 'new' && (
            <AutomationForm
              initial={null}
              workspaces={workspaces}
              onCancel={() => setPane({ kind: 'none' })}
              onSaved={a => {
                setPane({ kind: 'detail', id: a.id, runId: null });
                load();
              }}
            />
          )}
          {pane.kind === 'edit' && selected && (
            <AutomationForm
              initial={selected}
              workspaces={workspaces}
              onCancel={() => setPane({ kind: 'detail', id: selected.id, runId: null })}
              onSaved={a => {
                setPane({ kind: 'detail', id: a.id, runId: null });
                load();
              }}
            />
          )}
          {pane.kind === 'detail' && selected && (
            <Detail
              automation={selected}
              runId={pane.runId}
              now={now}
              onRun={runId => setPane({ kind: 'detail', id: selected.id, runId })}
              onEdit={() => setPane({ kind: 'edit', id: selected.id })}
              onDeleted={() => {
                setPane({ kind: 'none' });
                load();
              }}
              onOpenChat={onOpenChat}
            />
          )}
          {(pane.kind === 'none' || ((pane.kind === 'detail' || pane.kind === 'edit') && !selected)) && (
            <p className="m-0 text-small text-shodh-text-muted">Pick an automation</p>
          )}
        </aside>
      </div>
    </div>
  );
}
