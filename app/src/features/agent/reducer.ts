/**
 * Transcript reducer: folds the `AgentEvent` stream of one run into the
 * state the transcript renders. Pure, no React, type-only imports, so it is
 * unit-tested directly with Node (`app/tests/reducer.test.ts`).
 *
 * Besides backend events it accepts a few local actions (prefixed `local_`)
 * for things only the UI knows: the user steered, answered an approval, the
 * request failed before the run started, or an interrupt timed out.
 */

import type { AgentEvent, GroundingReport, PlanItem, RevisionReason, RiskTier } from './events';
import type { PdfRegion } from '../ask/types';
import { parseRegions } from '../ask/viewer/regionGeometry.ts';
import { readProviderError } from '../modelPicker/modelTypes.ts';
import type { ProviderError, ProviderErrorKind } from '../modelPicker/modelTypes.ts';

/**
 * `retried`: the call was refused for its arguments and a later call of the
 * same tool succeeded, so the failure was the model correcting itself, not
 * a failure of the answer.
 */
export type StepStatus = 'running' | 'awaiting_approval' | 'done' | 'failed' | 'retried';

export type ApprovalDecision = 'pending' | 'approved' | 'denied';

export interface StepApproval {
  label: string;
  tier: RiskTier;
  preview: unknown;
  decision: ApprovalDecision;
}

export interface TranscriptStep {
  id: string;
  parentId: string | null;
  tool: string;
  label: string;
  args: unknown;
  tier: RiskTier;
  startedAtMs: number;
  status: StepStatus;
  summary: string | null;
  detail: unknown;
  durationMs: number | null;
  /** Latest progress text while running. */
  progress: string | null;
  approval: StepApproval | null;
  /** Ids of nested sub-steps, in start order. */
  children: string[];
  /** Failed because its arguments did not fit the tool (see `StepStatus` `retried`). */
  invalidArguments?: boolean;
}

/**
 * One numbered source the model cites as `[n]`: a passage from the user's
 * documents (`search_documents`) or, with `web` set, a web page or paper
 * (`web_search`, `fetch_url`, `search_papers`) whose `path` is its URL.
 */
export interface Passage {
  n: number;
  file: string;
  path: string;
  page: string | null;
  heading: string | null;
  score: number;
  text: string;
  /** Untrusted web content; `path` is an http(s) URL. Absent in older transcripts. */
  web?: boolean;
  /** Heading chain ("3 Method > 3.2 Chunkwise form"); absent for unstructured sources. */
  section?: string | null;
  /** Layout boxes on the cited pages; absent for unstructured sources. */
  regions?: PdfRegion[] | null;
  /** `path` as the file system spells it (`path` is lowercased on Windows); absent when unknown. */
  displayPath?: string | null;
}

/** A web source as returned in a web tool's `detail.webSources`. */
export interface WebSource {
  n: number;
  title: string;
  url: string;
  snippet: string;
}

export type TranscriptBlock =
  | { kind: 'text'; id: string; text: string }
  | { kind: 'step'; stepId: string }
  | { kind: 'steer'; id: string; text: string }
  /** The app asked the model to fix flagged statements or search for missing parts. */
  | { kind: 'revision'; id: string; round: number; reason: RevisionReason; flagged: number; missingNeeds: string[] };

export type TranscriptStatus = 'starting' | 'running' | 'completed' | 'aborted' | 'error';

export interface UsageTotals {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  costUsd: number;
}

/** Machine-readable reason a run could not start (from `AgentCommandError.code`). */
export type FailureCode =
  | 'runtime_missing'
  | 'runtime_invalid'
  | 'model_config'
  | 'busy'
  | 'invalid_request'
  | 'session_closed'
  | 'runtime_error';

export interface TranscriptState {
  /** Run id (equals the request id the UI sent). */
  runId: string;
  sessionId: string | null;
  model: string | null;
  /** Data-handling warning for the model, shown for the whole run. */
  warning: string | null;
  status: TranscriptStatus;
  /** Local clock, ms since epoch, when the user sent the message. */
  startedAtMs: number;
  /** Wall time of the finished run. */
  durationMs: number | null;
  error: string | null;
  errorCode: FailureCode | null;
  blocks: TranscriptBlock[];
  steps: Record<string, TranscriptStep>;
  plan: PlanItem[] | null;
  usage: UsageTotals | null;
  /** Passages from every search of the run, ordered by `n`. */
  passages: Passage[];
  /** Grounding checks of the answer, one per round; the final one describes the answer. */
  groundings: GroundingReport[];
  /**
   * Text blocks that are the model's working notes before its tool calls, not the answer
   * (absent in transcripts saved before notes were told apart).
   */
  interimIds?: string[];
  /** The answer is being checked against its sources (not persisted). */
  checking: boolean;
  /** Latest reasoning text, for the live status line (not persisted). */
  thinking: string;
  /**
   * The reasoning text the provider streamed alongside the answer, kept so it
   * can be read afterwards (the "Reasoning" panel). Never part of the answer,
   * its citations or its check. Absent when the provider sent none, and in
   * transcripts saved before it was kept.
   */
  reasoning?: ReasoningLog | null;
  /** The user asked to interrupt; waiting for the run to stop. */
  interrupting: boolean;
  /** The provider failure behind an `error` status (absent in transcripts saved before it existed). */
  providerError?: ProviderError | null;
  /** This answer was retried with a fallback model (absent when it was not). */
  fallback?: TranscriptFallback | null;
}

/** Most characters of reasoning kept per answer (about 32 KB); the rest is dropped and marked. */
export const MAX_REASONING_CHARS = 32 * 1024;

/** One stretch of reasoning, placed among the run's blocks. */
export interface ReasoningSegment {
  /** How many transcript blocks there were when it began: it comes after those. */
  at: number;
  text: string;
}

/** The reasoning text of one answer, in order with its tool steps. */
export interface ReasoningLog {
  segments: ReasoningSegment[];
  /** More reasoning arrived than `MAX_REASONING_CHARS` keeps. */
  truncated: boolean;
}

/** `log` with `delta` appended at block position `at`, within `MAX_REASONING_CHARS`. */
function appendReasoning(log: ReasoningLog | null | undefined, at: number, delta: string): ReasoningLog {
  const current = log ?? { segments: [], truncated: false };
  if (current.truncated) return current;
  const used = current.segments.reduce((n, seg) => n + seg.text.length, 0);
  const room = MAX_REASONING_CHARS - used;
  const kept = delta.length <= room ? delta : delta.slice(0, Math.max(0, room));
  const truncated = kept.length < delta.length;
  if (kept.length === 0) return { ...current, truncated };
  const last = current.segments[current.segments.length - 1];
  const segments = last && last.at === at
    ? [...current.segments.slice(0, -1), { at, text: last.text + kept }]
    : [...current.segments, { at, text: kept }];
  return { segments, truncated };
}

/** A stored reasoning log, validated and capped; null when there is none. */
function readReasoning(value: unknown): ReasoningLog | null {
  if (!isRecord(value) || !Array.isArray(value.segments)) return null;
  let log: ReasoningLog | null = null;
  for (const seg of value.segments) {
    if (!isRecord(seg) || typeof seg.text !== 'string' || typeof seg.at !== 'number') continue;
    if (!Number.isInteger(seg.at) || seg.at < 0 || seg.text.length === 0) continue;
    log = appendReasoning(log, seg.at, seg.text);
  }
  if (!log) return null;
  return { ...log, truncated: log.truncated || value.truncated === true };
}

/** One section of the reasoning panel: the reasoning before the first step, or after a step. */
export interface ReasoningSection {
  /** Label of the tool step it follows; null before any step. */
  afterStep: string | null;
  text: string;
}

/**
 * The reasoning of `state` as the panel shows it: consecutive segments with
 * no step between them are joined, and each section names the step it
 * follows. Empty when the provider sent no reasoning.
 */
export function reasoningSections(state: TranscriptState): ReasoningSection[] {
  const out: ReasoningSection[] = [];
  for (const seg of state.reasoning?.segments ?? []) {
    let afterStep: string | null = null;
    for (let i = Math.min(seg.at, state.blocks.length) - 1; i >= 0; i--) {
      const block = state.blocks[i];
      if (block.kind !== 'step') continue;
      afterStep = state.steps[block.stepId]?.label ?? null;
      break;
    }
    const last = out[out.length - 1];
    if (last && last.afterStep === afterStep) last.text += seg.text;
    else out.push({ afterStep, text: seg.text });
  }
  return out.filter(section => section.text.trim().length > 0);
}

/** Why an answer came from a fallback model. */
export interface TranscriptFallback {
  /** The model that failed (`provider/model`, as runs name it). */
  from: string;
  /** Its failure. */
  kind: ProviderErrorKind;
  /** Retried by the "always fall back" setting rather than a click. */
  automatic: boolean;
}

export type LocalAction =
  | { type: 'local_steer'; id: string; text: string }
  | { type: 'local_approval'; stepId: string; approved: boolean }
  | { type: 'local_failed'; error: string; code: FailureCode; atMs: number }
  | { type: 'local_interrupt_requested' }
  | { type: 'local_interrupted'; atMs: number };

export type TranscriptAction = AgentEvent | LocalAction;

export function initialTranscript(runId: string, startedAtMs: number, fallback: TranscriptFallback | null = null): TranscriptState {
  return {
    providerError: null,
    fallback,
    runId,
    sessionId: null,
    model: null,
    warning: null,
    status: 'starting',
    startedAtMs,
    durationMs: null,
    error: null,
    errorCode: null,
    blocks: [],
    steps: {},
    plan: null,
    usage: null,
    passages: [],
    groundings: [],
    interimIds: [],
    checking: false,
    thinking: '',
    interrupting: false,
  };
}

export function isLive(state: TranscriptState): boolean {
  return state.status === 'starting' || state.status === 'running';
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function str(value: unknown): string | null {
  return typeof value === 'string' && value.length > 0 ? value : null;
}

function isHttpUrl(value: string): boolean {
  return /^https?:\/\//i.test(value);
}

/** Web sources carried by a web tool step's detail (only http(s) URLs). */
export function webSourcesFromDetail(detail: unknown): WebSource[] {
  if (!isRecord(detail) || !Array.isArray(detail.webSources)) return [];
  const out: WebSource[] = [];
  for (const entry of detail.webSources) {
    if (!isRecord(entry)) continue;
    const n = entry.n;
    const url = str(entry.url);
    if (typeof n !== 'number' || !Number.isInteger(n) || n < 1 || !url || !isHttpUrl(url)) continue;
    out.push({
      n,
      title: str(entry.title) ?? url,
      url,
      snippet: typeof entry.snippet === 'string' ? entry.snippet : '',
    });
  }
  return out;
}

/** Numbered sources carried by a step's detail: document passages and web sources. */
export function passagesFromDetail(detail: unknown): Passage[] {
  const web: Passage[] = webSourcesFromDetail(detail).map(s => ({
    n: s.n,
    file: s.title,
    path: s.url,
    page: null,
    heading: null,
    score: 0,
    text: s.snippet,
    web: true,
  }));
  if (!isRecord(detail) || !Array.isArray(detail.passages)) return web;
  const out: Passage[] = web;
  for (const entry of detail.passages) {
    if (!isRecord(entry)) continue;
    const n = entry.n;
    const path = str(entry.path);
    if (typeof n !== 'number' || !Number.isInteger(n) || n < 1 || !path) continue;
    const page = typeof entry.page === 'number' ? String(entry.page) : str(entry.page);
    const displayPath = str(entry.display_path);
    out.push({
      n,
      file: str(entry.file) ?? path,
      path,
      page,
      heading: str(entry.heading),
      score: typeof entry.score === 'number' ? entry.score : 0,
      text: typeof entry.text === 'string' ? entry.text : '',
      section: str(entry.section),
      regions: parseRegions(entry.regions),
      ...(displayPath ? { displayPath } : {}),
    });
  }
  return out;
}

/** A plan item from an event; items from older builds lack the need fields. */
function planItem(item: PlanItem): PlanItem {
  return {
    id: item.id,
    text: item.text,
    status: item.status,
    need: item.need === true,
    coverage: item.coverage === 'covered' || item.coverage === 'missing' ? item.coverage : null,
    evidence: Array.isArray(item.evidence) ? item.evidence.filter(n => Number.isInteger(n)) : [],
  };
}

function mergePassages(existing: Passage[], incoming: Passage[]): Passage[] {
  if (incoming.length === 0) return existing;
  const byNumber = new Map<number, Passage>();
  for (const p of existing) byNumber.set(p.n, p);
  for (const p of incoming) byNumber.set(p.n, p);
  return [...byNumber.values()].sort((a, b) => a.n - b.n);
}

function appendText(blocks: TranscriptBlock[], id: string, delta: string): TranscriptBlock[] {
  const last = blocks[blocks.length - 1];
  if (last && last.kind === 'text' && last.id === id) {
    return [...blocks.slice(0, -1), { ...last, text: last.text + delta }];
  }
  const index = blocks.findIndex(b => b.kind === 'text' && b.id === id);
  if (index >= 0) {
    const block = blocks[index] as { kind: 'text'; id: string; text: string };
    const next = blocks.slice();
    next[index] = { ...block, text: block.text + delta };
    return next;
  }
  return [...blocks, { kind: 'text', id, text: delta }];
}

function updateStep(
  state: TranscriptState,
  stepId: string,
  patch: (step: TranscriptStep) => TranscriptStep,
): TranscriptState {
  const step = state.steps[stepId];
  if (!step) return state;
  return { ...state, steps: { ...state.steps, [stepId]: patch(step) } };
}

/**
 * A call of `tool` succeeded: its earlier calls refused for their arguments
 * were the model correcting itself, and show as retried.
 */
function markRetried(steps: Record<string, TranscriptStep>, tool: string): Record<string, TranscriptStep> {
  let changed = false;
  const next: Record<string, TranscriptStep> = {};
  for (const [id, step] of Object.entries(steps)) {
    if (step.tool === tool && step.status === 'failed' && step.invalidArguments) {
      changed = true;
      next[id] = { ...step, status: 'retried' };
    } else {
      next[id] = step;
    }
  }
  return changed ? next : steps;
}

/**
 * Whether `step` was refused for its arguments in a run that is still going:
 * the model normally corrects the call next, so it is shown as being retried,
 * not as a failure. Once the run ends without a corrected call it is a failure.
 */
export function awaitingRetry(step: TranscriptStep, live: boolean): boolean {
  return live && step.status === 'failed' && step.invalidArguments === true;
}

/** Close any step still open (the run ended without reporting it). */
function closeOpenSteps(steps: Record<string, TranscriptStep>, summary: string, atMs: number) {
  let changed = false;
  const next: Record<string, TranscriptStep> = {};
  for (const [id, step] of Object.entries(steps)) {
    if (step.status === 'running' || step.status === 'awaiting_approval') {
      changed = true;
      next[id] = {
        ...step,
        status: 'failed',
        summary: step.summary ?? summary,
        durationMs: step.durationMs ?? Math.max(0, atMs - step.startedAtMs),
        approval: step.approval && step.approval.decision === 'pending'
          ? { ...step.approval, decision: 'denied' }
          : step.approval,
      };
    } else {
      next[id] = step;
    }
  }
  return changed ? next : steps;
}

function forThisRun(state: TranscriptState, action: TranscriptAction): boolean {
  if (action.type.startsWith('local_')) return true;
  return (action as AgentEvent).runId === state.runId;
}

export function reduceTranscript(state: TranscriptState, action: TranscriptAction): TranscriptState {
  if (!forThisRun(state, action)) return state;
  // A finished run only accepts late usage totals.
  if (!isLive(state) && action.type !== 'usage') return state;

  switch (action.type) {
    case 'run_started':
      return {
        ...state,
        sessionId: action.sessionId,
        model: action.model,
        warning: action.warning,
        status: 'running',
      };

    case 'text_delta':
      if (action.delta.length === 0) return state;
      return { ...state, status: 'running', blocks: appendText(state.blocks, action.messageId, action.delta), thinking: '' };

    case 'text_interim': {
      const ids = (state.interimIds ?? []).filter(id => id !== action.messageId);
      return { ...state, interimIds: action.interim ? [...ids, action.messageId] : ids };
    }

    case 'thinking':
      if (action.delta.length === 0) return state;
      return {
        ...state,
        status: 'running',
        thinking: (state.thinking + action.delta).slice(-600),
        reasoning: appendReasoning(state.reasoning, state.blocks.length, action.delta),
      };

    case 'step_started': {
      if (state.steps[action.stepId]) return state;
      const parentId = action.parentStepId && state.steps[action.parentStepId] ? action.parentStepId : null;
      const step: TranscriptStep = {
        id: action.stepId,
        parentId,
        tool: action.tool,
        label: action.label,
        args: action.args,
        tier: action.tier,
        startedAtMs: action.atMs,
        status: 'running',
        summary: null,
        detail: null,
        durationMs: null,
        progress: null,
        approval: null,
        children: [],
      };
      const steps = { ...state.steps, [step.id]: step };
      if (parentId) {
        const parent = steps[parentId];
        steps[parentId] = { ...parent, children: [...parent.children, step.id] };
        return { ...state, status: 'running', steps, thinking: '' };
      }
      return {
        ...state,
        status: 'running',
        steps,
        blocks: [...state.blocks, { kind: 'step', stepId: step.id }],
        thinking: '',
      };
    }

    case 'step_progress':
      return updateStep(state, action.stepId, s => ({ ...s, progress: action.text }));

    case 'approval_requested':
      return updateStep(state, action.stepId, s => ({
        ...s,
        status: 'awaiting_approval',
        approval: { label: action.label, tier: action.tier, preview: action.preview, decision: 'pending' },
      }));

    case 'step_finished': {
      const step = state.steps[action.stepId];
      if (!step) return state;
      const invalidArguments = !action.ok && action.invalidArguments === true;
      const next = updateStep(state, action.stepId, s => ({
        ...s,
        status: action.ok ? 'done' : 'failed',
        summary: action.summary,
        detail: action.detail,
        durationMs: action.durationMs,
        progress: null,
        approval: s.approval && s.approval.decision === 'pending'
          ? { ...s.approval, decision: action.ok ? 'approved' : 'denied' }
          : s.approval,
        ...(invalidArguments ? { invalidArguments: true } : {}),
      }));
      if (!action.ok) return next;
      return {
        ...next,
        steps: markRetried(next.steps, step.tool),
        passages: mergePassages(next.passages, passagesFromDetail(action.detail)),
      };
    }

    case 'plan_updated':
      return { ...state, plan: action.items.length > 0 ? action.items.map(planItem) : null };

    case 'grounding_started':
      return { ...state, checking: true };

    case 'grounding':
      return { ...state, groundings: [...state.groundings, action.report], checking: false };

    case 'revision_started':
      return {
        ...state,
        checking: false,
        blocks: [
          ...state.blocks,
          {
            kind: 'revision',
            id: `revision-${action.round}`,
            round: action.round,
            reason: action.reason,
            flagged: action.flagged,
            missingNeeds: action.missingNeeds,
          },
        ],
      };

    case 'navigated':
      return state;

    case 'usage':
      return {
        ...state,
        usage: {
          inputTokens: action.inputTokens,
          outputTokens: action.outputTokens,
          cacheReadTokens: action.cacheReadTokens,
          costUsd: action.costUsd,
        },
      };

    case 'run_finished': {
      const status: TranscriptStatus =
        action.status === 'completed' ? 'completed' : action.status === 'aborted' ? 'aborted' : 'error';
      const endMs = state.startedAtMs + action.durationMs;
      return {
        ...state,
        status,
        durationMs: action.durationMs,
        error: action.error,
        errorCode: status === 'error' ? 'runtime_error' : null,
        providerError: status === 'error' ? readProviderError(action.providerError) : null,
        steps: closeOpenSteps(state.steps, status === 'aborted' ? 'Interrupted' : 'Did not complete', endMs),
        thinking: '',
        interrupting: false,
        checking: false,
      };
    }

    case 'local_interrupt_requested':
      return { ...state, interrupting: true };

    case 'local_steer':
      return { ...state, blocks: [...state.blocks, { kind: 'steer', id: action.id, text: action.text }] };

    case 'local_approval':
      return updateStep(state, action.stepId, s => (s.approval
        ? {
            ...s,
            status: action.approved ? 'running' : s.status,
            approval: { ...s.approval, decision: action.approved ? 'approved' : 'denied' },
          }
        : s));

    case 'local_failed':
      return {
        ...state,
        status: 'error',
        error: action.error,
        errorCode: action.code,
        durationMs: Math.max(0, action.atMs - state.startedAtMs),
        steps: closeOpenSteps(state.steps, 'Did not complete', action.atMs),
        thinking: '',
        interrupting: false,
      };

    case 'local_interrupted':
      return {
        ...state,
        status: 'aborted',
        durationMs: Math.max(0, action.atMs - state.startedAtMs),
        steps: closeOpenSteps(state.steps, 'Interrupted', action.atMs),
        thinking: '',
        interrupting: false,
      };

    default:
      return state;
  }
}

/** Fold a batch of actions (one animation frame's worth). */
export function reduceAll(state: TranscriptState, actions: readonly TranscriptAction[]): TranscriptState {
  let next = state;
  for (const action of actions) next = reduceTranscript(next, action);
  return next;
}

/** Text blocks a revised answer replaced (kept in the transcript as an earlier draft). */
export function supersededBlocks(state: TranscriptState): Set<string> {
  const out = new Set<string>();
  for (const report of state.groundings) for (const id of report.supersededMessageIds) out.add(id);
  return out;
}

/** Text blocks that are working notes before tool calls ("I'll search the contracts"). */
export function interimBlocks(state: TranscriptState): Set<string> {
  return new Set(state.interimIds ?? []);
}

/** Text blocks that are not the answer: replaced drafts and working notes. */
export function notAnswerBlocks(state: TranscriptState): Set<string> {
  const out = supersededBlocks(state);
  for (const id of state.interimIds ?? []) out.add(id);
  return out;
}

/**
 * The answer text: every text block of the run, in order, without replaced drafts and
 * without the working notes that led into tool calls.
 */
export function answerText(state: TranscriptState): string {
  const excluded = notAnswerBlocks(state);
  return state.blocks
    .filter((b): b is { kind: 'text'; id: string; text: string } => b.kind === 'text' && !excluded.has(b.id))
    .map(b => b.text.trim())
    .filter(t => t.length > 0)
    .join('\n\n');
}

/** The step to show as "what the agent is doing now", if any. */
export function currentStep(state: TranscriptState): TranscriptStep | null {
  let latest: TranscriptStep | null = null;
  for (const step of Object.values(state.steps)) {
    if (step.status !== 'running' && step.status !== 'awaiting_approval') continue;
    if (!latest || step.startedAtMs >= latest.startedAtMs) latest = step;
  }
  return latest;
}

/** A pending approval, if the run is waiting on the user. */
export function pendingApproval(state: TranscriptState): TranscriptStep | null {
  for (const step of Object.values(state.steps)) {
    if (step.approval?.decision === 'pending') return step;
  }
  return null;
}

/** Form stored with the conversation: no transient status text (the reasoning log is kept). */
export function toPersisted(state: TranscriptState): TranscriptState {
  return { ...state, thinking: '', interrupting: false, checking: false };
}

const STATUSES: readonly TranscriptStatus[] = ['starting', 'running', 'completed', 'aborted', 'error'];

/**
 * Restore a stored transcript. A run stored while live can only come from an
 * interrupted app session, so it is shown as interrupted, never spinning.
 */
export function fromPersisted(value: unknown): TranscriptState | null {
  if (!isRecord(value) || typeof value.runId !== 'string' || typeof value.startedAtMs !== 'number') return null;
  if (!STATUSES.includes(value.status as TranscriptStatus)) return null;
  if (!Array.isArray(value.blocks) || !isRecord(value.steps)) return null;
  const { reasoning: storedReasoning, ...state } = value as unknown as TranscriptState;
  const reasoning = readReasoning(storedReasoning);
  const restored: TranscriptState = {
    ...initialTranscript(state.runId, state.startedAtMs),
    ...state,
    ...(reasoning ? { reasoning } : {}),
    passages: Array.isArray(state.passages) ? state.passages : [],
    plan: Array.isArray(state.plan) ? state.plan.map(planItem) : null,
    groundings: Array.isArray(state.groundings) ? state.groundings : [],
    interimIds: Array.isArray(state.interimIds) ? state.interimIds.filter(id => typeof id === 'string') : [],
    checking: false,
    thinking: '',
    interrupting: false,
  };
  if (!isLive(restored)) return restored;
  const atMs = restored.startedAtMs + (restored.durationMs ?? 0);
  return {
    ...restored,
    status: 'aborted',
    error: 'The app closed before this answer finished.',
    steps: closeOpenSteps(restored.steps, 'Interrupted', atMs),
  };
}

/** Plan progress, e.g. { done: 3, total: 5 }. */
export function planProgress(plan: readonly PlanItem[]): { done: number; total: number } {
  return { done: plan.filter(i => i.status === 'done').length, total: plan.length };
}
