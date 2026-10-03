/**
 * Transcript reducer: folds the `AgentEvent` stream of one run into the
 * state the transcript renders. Pure, no React, type-only imports, so it is
 * unit-tested directly with Node (`app/tests/reducer.test.ts`).
 *
 * Besides backend events it accepts a few local actions (prefixed `local_`)
 * for things only the UI knows: the user steered, answered an approval, the
 * request failed before the run started, or an interrupt timed out.
 */

import type { AgentEvent, PlanItem, RiskTier } from './events';

export type StepStatus = 'running' | 'awaiting_approval' | 'done' | 'failed';

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
}

/** One passage returned by `search_documents`; the model cites it as `[n]`. */
export interface Passage {
  n: number;
  file: string;
  path: string;
  page: string | null;
  heading: string | null;
  score: number;
  text: string;
}

export type TranscriptBlock =
  | { kind: 'text'; id: string; text: string }
  | { kind: 'step'; stepId: string }
  | { kind: 'steer'; id: string; text: string };

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
  /** Latest reasoning text (not persisted). */
  thinking: string;
  /** The user asked to interrupt; waiting for the run to stop. */
  interrupting: boolean;
}

export type LocalAction =
  | { type: 'local_steer'; id: string; text: string }
  | { type: 'local_approval'; stepId: string; approved: boolean }
  | { type: 'local_failed'; error: string; code: FailureCode; atMs: number }
  | { type: 'local_interrupt_requested' }
  | { type: 'local_interrupted'; atMs: number };

export type TranscriptAction = AgentEvent | LocalAction;

export function initialTranscript(runId: string, startedAtMs: number): TranscriptState {
  return {
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

/** Passages carried by a `search_documents` step's detail. */
export function passagesFromDetail(detail: unknown): Passage[] {
  if (!isRecord(detail) || !Array.isArray(detail.passages)) return [];
  const out: Passage[] = [];
  for (const entry of detail.passages) {
    if (!isRecord(entry)) continue;
    const n = entry.n;
    const path = str(entry.path);
    if (typeof n !== 'number' || !Number.isInteger(n) || n < 1 || !path) continue;
    const page = typeof entry.page === 'number' ? String(entry.page) : str(entry.page);
    out.push({
      n,
      file: str(entry.file) ?? path,
      path,
      page,
      heading: str(entry.heading),
      score: typeof entry.score === 'number' ? entry.score : 0,
      text: typeof entry.text === 'string' ? entry.text : '',
    });
  }
  return out;
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

    case 'thinking':
      return { ...state, status: 'running', thinking: (state.thinking + action.delta).slice(-600) };

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
      }));
      return action.ok
        ? { ...next, passages: mergePassages(next.passages, passagesFromDetail(action.detail)) }
        : next;
    }

    case 'plan_updated':
      return { ...state, plan: action.items.length > 0 ? action.items : null };

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
        steps: closeOpenSteps(state.steps, status === 'aborted' ? 'Interrupted' : 'Did not complete', endMs),
        thinking: '',
        interrupting: false,
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

/** The answer text: every text block of the run, in order. */
export function answerText(state: TranscriptState): string {
  return state.blocks
    .filter((b): b is { kind: 'text'; id: string; text: string } => b.kind === 'text')
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

/** Form stored with the conversation: no transient reasoning text. */
export function toPersisted(state: TranscriptState): TranscriptState {
  return { ...state, thinking: '', interrupting: false };
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
  const state = value as unknown as TranscriptState;
  const restored: TranscriptState = {
    ...initialTranscript(state.runId, state.startedAtMs),
    ...state,
    passages: Array.isArray(state.passages) ? state.passages : [],
    plan: Array.isArray(state.plan) ? state.plan : null,
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
