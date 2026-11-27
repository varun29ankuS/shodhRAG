/**
 * Automations, in the words and shapes the view shows (`automations.rs` on the wire):
 * each automation's state chip and tooltip, why a run ran, its calls, the form's
 * schedule or event, the estimate, which limit the automations' model counts against,
 * a draft and the templates as the form takes them, and the replay of where a run's
 * agent would have taken the user.
 *
 * Pure (type-only imports), so `node --test` loads it directly
 * (`app/tests/automations.test.ts`).
 */

import type { ModelRef, ProviderId } from '../modelPicker/modelTypes.ts';
import { isoDate } from '../../i18n/format.ts';

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

/** What starts an automation instead of a schedule (`EventTrigger` in Rust). */
export type EventTrigger = { kind: 'files_added'; sourceId: string } | { kind: 'paper_found'; alertId: string };

export type RunState = 'queued' | 'running' | 'waiting' | 'succeeded' | 'failed' | 'skipped' | 'cancelled';
export type WaitReason = 'approval' | 'decision' | 'yield' | 'quota' | 'table_job' | 'model_unavailable' | 'offline' | 'backoff';
export type Trigger = 'schedule' | 'catch_up' | 'manual' | 'event' | 'retry';
export type AutomationState =
  | 'scheduled'
  | 'manual'
  | 'watching'
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
  /** What the step was given (an event's run: `subjects` and `more` at step 0). */
  input: Record<string, unknown>;
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
  trigger: EventTrigger | null;
  /** Its agent answers from what the run gives it, with no tools (one call a run). */
  answerOnly: boolean;
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
  trigger: EventTrigger | null;
  workspaceId: string | null;
  answerOnly: boolean;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

const str = (value: unknown): string | null => (typeof value === 'string' ? value : null);
const num = (value: unknown): number => (typeof value === 'number' && Number.isFinite(value) ? value : 0);

export function readTrigger(value: unknown): EventTrigger | null {
  if (!isRecord(value)) return null;
  if (value.kind === 'files_added' && typeof value.sourceId === 'string') return { kind: 'files_added', sourceId: value.sourceId };
  if (value.kind === 'paper_found' && typeof value.alertId === 'string') return { kind: 'paper_found', alertId: value.alertId };
  return null;
}

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
        input: isRecord(s.input) ? s.input : {},
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
    trigger: readTrigger(value.trigger),
    answerOnly: value.answerOnly === true,
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
  const day = (ms: number) => isoDate(ms, options.timeZone);
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
    case 'watching':
      return { label: 'Watching', tooltip: a.when || 'Runs when something arrives', tone: 'neutral' };
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
        tooltip:
          a.holdReason === 'model_unavailable'
            ? 'Model not answering; check the model'
            : a.last?.state === 'skipped' && a.last.summary?.startsWith('On hold:')
              ? a.last.summary
              : 'Failed 3 times in a row',
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
  const line = perRun === 1 ? 'Each run: 1 model call' : `Each run: up to ${perRun} model calls`;
  return runs === null ? line : `${line} (about ${runs * perRun} a month)`;
}

/** What the form's "When" offers. */
export type WhenKind = 'manual' | 'daily' | 'weekdays' | 'weekly' | 'monthly' | 'every_hours' | 'files_added' | 'paper_found';

/** The kinds of When that are an event, not a time. */
export function isEventKind(kind: WhenKind): kind is 'files_added' | 'paper_found' {
  return kind === 'files_added' || kind === 'paper_found';
}

export interface WhenForm {
  kind: WhenKind;
  at: string;
  days: WeekDay[];
  day: number | 'last';
  hours: number;
  /** The watched folder of "When files arrive". */
  sourceId: string;
  /** The paper alert of "When an alert finds papers". */
  alertId: string;
}

export const DEFAULT_WHEN: WhenForm = { kind: 'weekly', at: '09:00', days: ['mon'], day: 1, hours: 6, sourceId: '', alertId: '' };

/** The schedule a form says, or why it cannot be saved. */
export function scheduleOf(form: WhenForm, nowIso: string): { schedule: Schedule | null } | { error: string } {
  if (isEventKind(form.kind)) return { schedule: null };
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

/** What a form says starts the automation: a schedule, an event or neither, or why it cannot be saved. */
export function whenOf(form: WhenForm, nowIso: string): { schedule: Schedule | null; trigger: EventTrigger | null } | { error: string } {
  if (form.kind === 'files_added') {
    return form.sourceId ? { schedule: null, trigger: { kind: 'files_added', sourceId: form.sourceId } } : { error: 'Pick a folder.' };
  }
  if (form.kind === 'paper_found') {
    return form.alertId ? { schedule: null, trigger: { kind: 'paper_found', alertId: form.alertId } } : { error: 'Pick a paper alert.' };
  }
  const planned = scheduleOf(form, nowIso);
  return 'error' in planned ? planned : { schedule: planned.schedule, trigger: null };
}

/** The form for an automation's schedule or event (the default for a new one). */
export function formOf(schedule: Schedule | null, trigger: EventTrigger | null = null): WhenForm {
  if (!schedule && trigger?.kind === 'files_added') return { ...DEFAULT_WHEN, kind: 'files_added', sourceId: trigger.sourceId };
  if (!schedule && trigger?.kind === 'paper_found') return { ...DEFAULT_WHEN, kind: 'paper_found', alertId: trigger.alertId };
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

/** The model calls one run plans (`planned_calls` in Rust). */
export function plannedCalls(answerOnly: boolean): number {
  return answerOnly ? 1 : 4;
}

/** What the form opens with: a draft, a template or a skill. */
export interface FormPrefill {
  name: string;
  instruction: string;
  when: WhenForm;
  workspaceId: string | null;
  answerOnly: boolean;
  /** One line under the form's buttons (why a draft was not used). */
  note: string | null;
}

/** An automation the model drafted (`Drafted`). */
export interface Drafted {
  name: string;
  instruction: string;
  schedule: Schedule | null;
  trigger: EventTrigger | null;
  workspaceId: string | null;
  note: string | null;
}

export function readDrafted(value: unknown): Drafted | null {
  if (!isRecord(value) || typeof value.instruction !== 'string') return null;
  return {
    name: str(value.name) ?? '',
    instruction: value.instruction,
    schedule: isRecord(value.schedule) ? (value.schedule as unknown as Schedule) : null,
    trigger: readTrigger(value.trigger),
    workspaceId: str(value.workspaceId),
    note: str(value.note),
  };
}

/** The form a draft fills. A draft that was not used leaves When as the user had it. */
export function prefillFromDraft(drafted: Drafted, current: WhenForm): FormPrefill {
  return {
    name: drafted.name,
    instruction: drafted.instruction,
    when: drafted.note ? current : formOf(drafted.schedule, drafted.trigger),
    workspaceId: drafted.workspaceId,
    answerOnly: false,
    note: drafted.note,
  };
}

/** An automation ready to set up (`Template`). */
export interface Template {
  id: string;
  name: string;
  when: string;
  instruction: string;
  schedule: Schedule | null;
  triggerKind: 'files_added' | 'paper_found' | null;
  needsWorkspace: boolean;
  answerOnly: boolean;
  plannedCalls: number;
  estimate: string;
}

export function readTemplates(value: unknown): Template[] {
  if (!Array.isArray(value)) return [];
  return value.filter(isRecord).flatMap(t => {
    if (typeof t.id !== 'string' || typeof t.name !== 'string' || typeof t.instruction !== 'string') return [];
    const kind = t.triggerKind === 'files_added' || t.triggerKind === 'paper_found' ? t.triggerKind : null;
    return [{
      id: t.id,
      name: t.name,
      when: str(t.when) ?? '',
      instruction: t.instruction,
      schedule: isRecord(t.schedule) ? (t.schedule as unknown as Schedule) : null,
      triggerKind: kind,
      needsWorkspace: t.needsWorkspace === true,
      answerOnly: t.answerOnly === true,
      plannedCalls: num(t.plannedCalls),
      estimate: str(t.estimate) ?? '',
    }];
  });
}

/**
 * The form a template opens: its name, instruction and time; the first alert, folder
 * or workspace there is, for the user to change (none: the form says to pick one).
 */
export function prefillFromTemplate(
  template: Template,
  have: { folderIds: string[]; alertIds: string[]; workspaceIds: string[] },
): FormPrefill {
  const when: WhenForm =
    template.triggerKind === 'paper_found'
      ? { ...DEFAULT_WHEN, kind: 'paper_found', alertId: have.alertIds[0] ?? '' }
      : template.triggerKind === 'files_added'
        ? { ...DEFAULT_WHEN, kind: 'files_added', sourceId: have.folderIds[0] ?? '' }
        : formOf(template.schedule);
  return {
    name: template.name,
    instruction: template.instruction,
    when,
    workspaceId: template.needsWorkspace ? (have.workspaceIds[0] ?? null) : null,
    answerOnly: template.answerOnly,
    note: null,
  };
}

/**
 * The form "Automate" opens from an installed skill: the skill's name, and an
 * instruction that says what the skill is for, which the user completes. Only when the
 * user starts it, until they pick a time. Installing or changing skills stays in
 * Settings: an automation's agent has no tool for it.
 */
export function prefillFromSkill(skill: { name: string; description: string }): FormPrefill {
  const name = skill.name.trim().slice(0, 80);
  const what = skill.description.trim();
  return {
    name,
    instruction: (what ? `Do what the skill "${name}" is for: ${what}` : `Do what the skill "${name}" is for.`).slice(0, 4000),
    when: { ...DEFAULT_WHEN, kind: 'manual' },
    workspaceId: null,
    answerOnly: false,
    note: null,
  };
}

/** A step's row on the run page: what it was, and what it came to. */
export function stepLine(step: Step): { label: string; text: string } {
  if (step.kind === 'input') {
    const subjects = Array.isArray(step.input.subjects) ? step.input.subjects.length : 0;
    const count = subjects + num(step.input.more);
    return { label: 'Arrived', text: count === 1 ? '1 item' : `${count} items` };
  }
  const text = str(step.output.summary) ?? step.state;
  return { label: step.kind === 'notify' ? 'Told you' : 'Agent', text };
}

/** Somewhere a run's agent would have taken the user (kept in its step's output). */
export interface SavedMove {
  view: unknown;
  target: unknown;
  label: string | null;
}

/** Where a run's agent would have taken the user, in order. */
export function savedMoves(steps: Step[]): SavedMove[] {
  return steps.flatMap(step => {
    const list = step.kind === 'agent' && Array.isArray(step.output.navigations) ? step.output.navigations : [];
    return list.filter(isRecord).map(move => ({ view: move.view, target: move.target, label: str(move.label) }));
  });
}

/** How long each place stays on screen during a replay. */
export const REPLAY_GAP_MS = 2500;

/**
 * "Show me": goes to each place the run's agent opened, one at a time, with a pause
 * between them. Each move goes through the same gate as the assistant's own: while the
 * user is typing nothing moves, and the replay ends there. Returns how many places
 * were shown.
 */
export async function replayMoves(
  moves: SavedMove[],
  deps: { go: (move: SavedMove) => void; wait: (ms: number) => Promise<void>; typing: () => boolean; gapMs?: number },
): Promise<number> {
  let shown = 0;
  for (const move of moves) {
    if (shown > 0) await deps.wait(deps.gapMs ?? REPLAY_GAP_MS);
    if (deps.typing()) break;
    deps.go(move);
    shown += 1;
  }
  return shown;
}
