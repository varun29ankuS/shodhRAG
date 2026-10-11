/**
 * First-run setup state: which step the user is on, and whether setup was
 * finished or skipped. Persisted per viewer so setup resumes where it was
 * left. Honors the keys written by the previous onboarding modal so people
 * who already finished it are not onboarded again.
 *
 * Pure module (no runtime imports) so it is unit-tested directly with Node
 * (`app/tests/firstRun.test.ts`).
 */

export const FIRST_RUN_STEPS = ['welcome', 'search', 'model', 'folder', 'done'] as const;

export type FirstRunStep = typeof FIRST_RUN_STEPS[number];

export const FIRST_RUN_STEP_LABELS: Record<FirstRunStep, string> = {
  welcome: 'Welcome',
  search: 'Search',
  model: 'Model',
  folder: 'First folder',
  done: 'Ready',
};

export type FirstRunStatus = 'pending' | 'skipped' | 'completed';

export interface FirstRunState {
  status: FirstRunStatus;
  step: FirstRunStep;
}

export const FIRST_RUN_KEYS = {
  status: 'shodh.firstRun.status',
  step: 'shodh.firstRun.step',
  legacyCompleted: 'onboarding_completed',
  legacySkipped: 'onboarding_skipped',
} as const;

/** Minimal storage surface (localStorage satisfies it). */
export interface KeyValueStore {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function isStep(value: unknown): value is FirstRunStep {
  return typeof value === 'string' && (FIRST_RUN_STEPS as readonly string[]).includes(value);
}

/**
 * Saved state. Unreadable storage counts as completed: never trap someone
 * in setup because storage is unavailable.
 */
export function readFirstRun(store: KeyValueStore | null): FirstRunState {
  if (!store) return { status: 'completed', step: 'welcome' };
  try {
    const status = store.getItem(FIRST_RUN_KEYS.status);
    const savedStep = store.getItem(FIRST_RUN_KEYS.step);
    const step = isStep(savedStep) ? savedStep : 'welcome';
    if (status === 'completed' || status === 'skipped' || status === 'pending') return { status, step };
    if (store.getItem(FIRST_RUN_KEYS.legacyCompleted)) return { status: 'completed', step: 'welcome' };
    if (store.getItem(FIRST_RUN_KEYS.legacySkipped)) return { status: 'skipped', step: 'welcome' };
    return { status: 'pending', step };
  } catch {
    return { status: 'completed', step: 'welcome' };
  }
}

export function writeFirstRun(store: KeyValueStore | null, state: FirstRunState): void {
  if (!store) return;
  try {
    store.setItem(FIRST_RUN_KEYS.status, state.status);
    store.setItem(FIRST_RUN_KEYS.step, state.step);
  } catch {
    // Storage unavailable: progress lasts for this session only.
  }
}

export function stepIndex(step: FirstRunStep): number {
  return FIRST_RUN_STEPS.indexOf(step);
}

export function nextStep(step: FirstRunStep): FirstRunStep {
  return FIRST_RUN_STEPS[Math.min(FIRST_RUN_STEPS.length - 1, stepIndex(step) + 1)];
}

export function previousStep(step: FirstRunStep): FirstRunStep {
  return FIRST_RUN_STEPS[Math.max(0, stepIndex(step) - 1)];
}

/** Where resuming should start: the saved step, but never the final summary. */
export function resumeStep(state: FirstRunState): FirstRunStep {
  return state.step === 'done' ? 'welcome' : state.step;
}
