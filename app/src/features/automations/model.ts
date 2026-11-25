/**
 * Automations, in the words and shapes the view shows (`automations.rs` on the wire):
 * each automation's state chip and tooltip, why a run ran, its calls, the form's
 * schedule, the estimate, and which limit the automations' model counts against.
 *
 * Pure (type-only imports), so `node --test` loads it directly
 * (`app/tests/automations.test.ts`).
 */

import type { ModelRef, ProviderId } from '../modelPicker/modelTypes.ts';

export type WeekDay = 'mon' | 'tue' | 'wed' | 'thu' | 'fri' | 'sat' | 'sun';
export const WEEK_DAYS: readonly WeekDay[] = ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'];
export const WEEK_DAY_LABELS: Record<WeekDay, string> = {
  mon: 'Mon',
  tue: 'Tue',
  wed: 'Wed',
  thu: 'Thu',
  fri: 'Fri',
  sat: 'Sat',
  sun: 'Sun',
};

/** `Schedule` (Rust, tagged by `kind`). */
export type Schedule =
  | { kind: 'once'; at: string }
  | { kind: 'daily'; at: string }
  | { kind: 'weekdays'; at: string }
  | { kind: 'weekly'; days: WeekDay[]; at: string }
  | { kind: 'monthly'; day: number | 'last'; at: string }
  | { kind: 'every_hours'; hours: number; from: string };

export type RunState = 'queued' | 'running' | 'waiting' | 'succeeded' | 'failed' | 'skipped' | 'cancelled';
export type WaitReason = 'approval' | 'decision' | 'yield' | 'quota' | 'table_job' | 'model_unavailable' | 'offline' | 'backoff';
export type Trigger = 'schedule' | 'catch_up' | 'manual' | 'event' | 'retry';
export type AutomationState =
  | 'scheduled'
  | 'manual'
  | 'running'
  | 'waiting'
  | 'paused'
  | 'offline'
  | 'retrying'
  | 'needs_you'
  | 'on_hold'
  | 'off';

export interface Run {
  id: string;
  jobId: string;
  trigger: Trigger;
  scheduledFor: string | null;
  missed: number;
  state: RunState;
  waitReason: WaitReason | null;
  attempt: number;
  nextAttemptAt: string | null;
  plannedCalls: number;
  usedCalls: number;
  conversationId: string | null;
  summary: string | null;
  errorCode: string | null;
  errorDetail: string | null;
  startedAt: string | null;
  endedAt: string | null;
  createdAt: string;
}

export interface Step {
  runId: string;
  idx: number;
  kind: 'input' | 'agent' | 'notify';
  state: 'running' | 'done' | 'failed' | 'skipped';
  output: Record<string, unknown>;
  calls: number;
}

/** One change a run's agent asked for (`DeferredCall`). */
export interface Write {
  stepId: string;
  tool: string;
  label: string;
  tier: string;
  args: unknown;
  preview: unknown;
}

export interface Approval {
  id: string;
  jobId: string;
  runId: string;
  title: string;
  writes: Write[];
  createdAt: string;
  expiresAt: string;
  decision: 'approved' | 'denied' | 'expired' | 'partly' | null;
  result: Record<string, unknown>;
}

export interface RunDetail extends Run {
  steps: Step[];
  approval: Approval | null;
}

export interface Automation {
  id: string;
  name: string;
  instruction: string;
  workspaceId: string | null;
  schedule: Schedule | null;
  when: string;
  nextDueAt: string | null;
  enabled: boolean;
  holdReason: string | null;
  state: AutomationState;
  active: Run | null;
  last: Run | null;
  plannedCalls: number;
  createdBy: 'user' | 'agent' | 'system';
}

/** What the form saves (`Draft`). */
export interface Draft {
  name: string;
  instruction: string;
  schedule: Schedule | null;
  workspaceId: string | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

const str = (value: unknown): string | null => (typeof value === 'string' ? value : null);
const num = (value: unknown): number => (typeof value === 'number' && Number.isFinite(value) ? value : 0);

export function readRun(value: unknown): Run | null {
  if (!isRecord(value) || typeof value.id !== 'string' || typeof value.state !== 'string') return null;
  return {
    id: value.id,
    jobId: str(value.jobId) ?? '',
    trigger: (str(value.trigger) ?? 'manual') as Trigger,
    scheduledFor: str(value.scheduledFor),
    missed: num(value.missed),
    state: value.state as RunState,
    waitReason: str(value.waitReason) as WaitReason | null,
    attempt: num(value.attempt),
    nextAttemptAt: str(value.nextAttemptAt),
    plannedCalls: num(value.plannedCalls),
    usedCalls: num(value.usedCalls),
    conversationId: str(value.conversationId),
    summary: str(value.summary),
    errorCode: str(value.errorCode),
    errorDetail: str(value.errorDetail),
    startedAt: str(value.startedAt),
    endedAt: str(value.endedAt),
    createdAt: str(value.createdAt) ?? '',
  };
}

export function readRunDetail(value: unknown): RunDetail | null {
  const run = readRun(value);
  if (!run || !isRecord(value)) return null;
  const steps = Array.isArray(value.steps)
    ? value.steps.filter(isRecord).map(s => ({
        runId: str(s.runId) ?? run.id,
        idx: num(s.idx),
        kind: (str(s.kind) ?? 'agent') as Step['kind'],
        state: (str(s.state) ?? 'done') as Step['state'],
        output: isRecord(s.output) ? s.output : {},
        calls: num(s.calls),
      }))
    : [];
  const a = value.approval;
  const approval: Approval | null = isRecord(a) && typeof a.id === 'string'
    ? {
        id: a.id,
        jobId: str(a.jobId) ?? '',
        runId: str(a.runId) ?? run.id,
        title: str(a.title) ?? '',
        writes: Array.isArray(a.writes)
          ? a.writes.filter(isRecord).map(w => ({
              stepId: str(w.stepId) ?? '',
              tool: str(w.tool) ?? '',
              label: str(w.label) ?? '',
              tier: str(w.tier) ?? '',
              args: w.args,
              preview: w.preview,
            }))
          : [],
        createdAt: str(a.createdAt) ?? '',
        expiresAt: str(a.expiresAt) ?? '',
        decision: str(a.decision) as Approval['decision'],
        result: isRecord(a.result) ? a.result : {},
      }
    : null;
  return { ...run, steps, approval };
}

export function readAutomation(value: unknown): Automation | null {
  if (!isRecord(value) || typeof value.id !== 'string' || typeof value.name !== 'string') return null;
  return {
    id: value.id,
    name: value.name,
    instruction: str(value.instruction) ?? '',
    workspaceId: str(value.workspaceId),
    schedule: isRecord(value.schedule) ? (value.schedule as unknown as Schedule) : null,
    when: str(value.when) ?? '',
    nextDueAt: str(value.nextDueAt),
    enabled: value.enabled !== false,
    holdReason: str(value.holdReason),
    state: (str(value.state) ?? 'manual') as AutomationState,
    active: readRun(value.active),
    last: readRun(value.last),
    plannedCalls: num(value.plannedCalls),
    createdBy: (str(value.createdBy) ?? 'user') as Automation['createdBy'],
  };
}

export function readAutomations(value: unknown): Automation[] {
  return Array.isArray(value) ? value.map(readAutomation).filter((a): a is Automation => a !== null) : [];
}

export type Tone = 'neutral' | 'active' | 'waiting' | 'needs_you' | 'warning' | 'off';

/** "05:30", "05:30 tomorrow", "Mon 1 Dec 05:30". */
export function shortTime(iso: string, now: number, options: { locale?: string; timeZone?: string } = {}): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return '';
  const time = new Intl.DateTimeFormat(options.locale, { hour: '2-digit', minute: '2-digit', hourCycle: 'h23', timeZone: options.timeZone }).format(at);
  const day = (ms: number) => new Intl.DateTimeFormat('en-CA', { year: 'numeric', month: '2-digit', day: '2-digit', timeZone: options.timeZone }).format(new Date(ms));
  if (day(at.getTime()) === day(now)) return time;
  if (day(at.getTime()) === day(now + 86_400_000)) return `${time} tomorrow`;
  const date = new Intl.DateTimeFormat(options.locale, { weekday: 'short', day: 'numeric', month: 'short', timeZone: options.timeZone }).format(at);
  return `${date} ${time}`;
}

/** An automation's chip: one or two words, with its tooltip in one line. */
export function stateChip(a: Automation, now: number, options: { locale?: string; timeZone?: string } = {}): { label: string; tooltip: string; tone: Tone } {
  const next = a.active?.nextAttemptAt ? shortTime(a.active.nextAttemptAt, now, options) : '';
  switch (a.state) {
    case 'scheduled':
      return { label: 'Scheduled', tooltip: a.nextDueAt ? `Runs ${shortTime(a.nextDueAt, now, options)}` : a.when, tone: 'neutral' };
    case 'manual':
      return { label: 'Manual', tooltip: 'Runs when you start it', tone: 'neutral' };
    case 'running':
      return {
        label: 'Running',
        tooltip: a.active ? `${a.active.usedCalls} of ${a.active.plannedCalls || a.plannedCalls} calls` : 'Running',
        tone: 'active',
      };
    case 'waiting':
      return {
        label: 'Waiting',
        tooltip: a.active?.waitReason === 'model_unavailable' ? `Model not answering; tries again ${next}`.trim() : 'Waiting while you chat',
        tone: 'waiting',
      };
    case 'paused':
      return { label: 'Paused', tooltip: next ? `Your provider's limit; goes on after ${next}` : "Your provider's limit", tone: 'waiting' };
    case 'offline':
      return { label: 'Offline', tooltip: "Tries again when you're online", tone: 'waiting' };
    case 'retrying':
      return { label: 'Retrying', tooltip: next ? `Tries again ${next}` : 'Tries again soon', tone: 'waiting' };
    case 'needs_you':
      return { label: 'Needs you', tooltip: 'Wants your OK on its changes', tone: 'needs_you' };
    case 'on_hold':
      return {
        label: 'On hold',
        tooltip: a.holdReason === 'model_unavailable' ? 'Model not answering; check the model' : 'Failed 3 times in a row',
        tone: 'warning',
      };
    case 'off':
      return { label: 'Off', tooltip: 'Paused by you', tone: 'off' };
  }
}

/** Why a run ran, in one line. */
export function whyRan(run: Run): string {
  switch (run.trigger) {
    case 'manual':
      return 'Started by you';
    case 'catch_up':
      return run.missed > 0
        ? `Ran when Shodh opened; ${run.missed} earlier ${run.missed === 1 ? 'time was' : 'times were'} missed`
        : 'Ran when Shodh opened';
    case 'event':
      return 'Ran when something it watches changed';
    default:
      return run.missed > 0 ? `On schedule; ${run.missed} earlier ${run.missed === 1 ? 'time was' : 'times were'} missed` : 'On schedule';
  }
}

/** What a run came to, in one line (never empty). */
export function runOutcome(run: Run): string {
  if (run.state === 'waiting') {
    switch (run.waitReason) {
      case 'approval':
        return 'Waiting for your OK';
      case 'yield':
        return 'Waiting while you chat';
      case 'quota':
        return "Waiting for your provider's limit";
      case 'offline':
        return 'Waiting for the network';
      case 'model_unavailable':
        return 'Waiting for its model';
      default:
        return 'Trying again soon';
    }
  }
  if (run.state === 'queued' || run.state === 'running') return 'Running';
  if (run.state === 'failed') return run.errorDetail || 'Failed';
  return run.summary || (run.state === 'succeeded' ? 'Done' : run.state === 'skipped' ? 'Skipped' : 'Cancelled');
}

/** "3 of 4 calls". */
export function callsText(run: Run): string {
  return `${run.usedCalls} of ${run.plannedCalls} ${run.plannedCalls === 1 ? 'call' : 'calls'}`;
}

/** About how many times a schedule runs in a month (`runs_a_month` in Rust). */
export function runsAMonth(schedule: Schedule | null): number | null {
  if (!schedule) return null;
  switch (schedule.kind) {
    case 'once':
      return null;
    case 'daily':
      return 30;
    case 'weekdays':
      return 22;
    case 'weekly':
      return schedule.days.length * 4;
    case 'monthly':
      return 1;
    case 'every_hours':
      return Math.floor(24 / Math.max(1, schedule.hours)) * 30;
  }
}

/** The form's estimate (`estimate_line` in Rust). */
export function estimateText(schedule: Schedule | null, perRun: number): string {
  const runs = runsAMonth(schedule);
  const line = `Each run: up to ${perRun} model calls`;
  return runs === null ? line : `${line} (about ${runs * perRun} a month)`;
}

/** What the form's "When" offers. */
export type WhenKind = 'manual' | 'daily' | 'weekdays' | 'weekly' | 'monthly' | 'every_hours';

export interface WhenForm {
  kind: WhenKind;
  at: string;
  days: WeekDay[];
  day: number | 'last';
  hours: number;
}

export const DEFAULT_WHEN: WhenForm = { kind: 'weekly', at: '09:00', days: ['mon'], day: 1, hours: 6 };

/** The schedule a form says, or why it cannot be saved. */
export function scheduleOf(form: WhenForm, nowIso: string): { schedule: Schedule | null } | { error: string } {
  if (form.kind !== 'manual' && form.kind !== 'every_hours' && !/^([01]\d|2[0-3]):[0-5]\d$/.test(form.at)) {
    return { error: 'Pick a time.' };
  }
  switch (form.kind) {
    case 'manual':
      return { schedule: null };
    case 'daily':
      return { schedule: { kind: 'daily', at: form.at } };
    case 'weekdays':
      return { schedule: { kind: 'weekdays', at: form.at } };
    case 'weekly': {
      const days = WEEK_DAYS.filter(d => form.days.includes(d));
      return days.length === 0 ? { error: 'Pick at least one day.' } : { schedule: { kind: 'weekly', days, at: form.at } };
    }
    case 'monthly':
      return form.day === 'last' || (Number.isInteger(form.day) && form.day >= 1 && form.day <= 31)
        ? { schedule: { kind: 'monthly', day: form.day, at: form.at } }
        : { error: 'Pick a day from 1 to 31.' };
    case 'every_hours':
      return Number.isInteger(form.hours) && form.hours >= 1 && form.hours <= 24
        ? { schedule: { kind: 'every_hours', hours: form.hours, from: nowIso } }
        : { error: 'Every 1 to 24 hours.' };
  }
}

/** The form for an automation's schedule (the default for a new one). */
export function formOf(schedule: Schedule | null): WhenForm {
  if (!schedule) return { ...DEFAULT_WHEN, kind: 'manual' };
  switch (schedule.kind) {
    case 'daily':
    case 'weekdays':
      return { ...DEFAULT_WHEN, kind: schedule.kind, at: schedule.at };
    case 'weekly':
      return { ...DEFAULT_WHEN, kind: 'weekly', at: schedule.at, days: schedule.days };
    case 'monthly':
      return { ...DEFAULT_WHEN, kind: 'monthly', at: schedule.at, day: schedule.day };
    case 'every_hours':
      return { ...DEFAULT_WHEN, kind: 'every_hours', hours: schedule.hours };
    case 'once':
      return { ...DEFAULT_WHEN, kind: 'manual' };
  }
}

/** The quota pool a model counts against (`ModelRef::quota_pool` in Rust). */
export function poolOf(model: ModelRef): string {
  if (model.provider === 'openrouter') return model.model.trim().endsWith(':free') ? 'openrouter:free' : 'openrouter:paid';
  return model.provider;
}

const LOCAL_PROVIDERS: readonly ProviderId[] = ['lmstudio', 'ollama'];

/** The tag next to the automations' model: whose limit it counts against, in a word or two. */
export function limitTag(model: ModelRef | null, chat: ModelRef | null): { label: string; tooltip: string } {
  const runs = model ?? chat;
  if (!runs) return { label: 'No model', tooltip: 'Set up a model first' };
  if (LOCAL_PROVIDERS.includes(runs.provider)) return { label: 'Local', tooltip: 'Runs on this computer; no quota' };
  if (!model || (chat && poolOf(model) === poolOf(chat))) {
    return {
      label: "Shares chat's limit",
      tooltip:
        runs.provider === 'openrouter' && poolOf(runs) === 'openrouter:free'
          ? "Every free OpenRouter model shares one 50-a-day limit; automations wait for your chat"
          : 'Automations wait for your chat',
    };
  }
  return { label: 'Own limit', tooltip: "Counts against its own provider's limit" };
}

/** A model as the menu names it. */
export function modelKey(model: ModelRef | null): string {
  return model ? `${model.provider}/${model.model}` : '';
}
