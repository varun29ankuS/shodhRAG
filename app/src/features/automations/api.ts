/**
 * Tauri commands of the Automations view (`automations.rs`). Every call is the user's
 * own action; changes are broadcast as `automations-changed`.
 */
import { invoke } from '@tauri-apps/api/core';
import type { ModelRef } from '../modelPicker/modelTypes';
import { readAutomation, readAutomations, readRun, readRunDetail } from './model';
import type { Automation, Draft, Run, RunDetail } from './model';

/** Emitted when an automation or one of its runs changed. */
export const AUTOMATIONS_CHANGED_EVENT = 'automations-changed';

function must<T>(value: T | null, what: string): T {
  if (value === null) throw new Error(`${what} could not be read.`);
  return value;
}

export function errorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  return 'Something went wrong.';
}

export const automationsApi = {
  list: async (): Promise<Automation[]> => readAutomations(await invoke<unknown>('automations_list')),
  get: async (id: string): Promise<Automation> => must(readAutomation(await invoke<unknown>('automation_get', { id })), 'The automation'),
  runs: async (id: string, limit = 20): Promise<Run[]> => {
    const value = await invoke<unknown>('automation_runs', { id, limit });
    return Array.isArray(value) ? value.map(readRun).filter((r): r is Run => r !== null) : [];
  },
  run: async (runId: string): Promise<RunDetail> => must(readRunDetail(await invoke<unknown>('automation_run_get', { runId })), 'The run'),
  create: async (draft: Draft): Promise<Automation> => must(readAutomation(await invoke<unknown>('automation_create', { draft })), 'The automation'),
  update: async (id: string, draft: Draft): Promise<Automation> =>
    must(readAutomation(await invoke<unknown>('automation_update', { id, draft })), 'The automation'),
  remove: (id: string) => invoke<boolean>('automation_delete', { id }),
  pause: async (id: string): Promise<Automation> => must(readAutomation(await invoke<unknown>('automation_pause', { id })), 'The automation'),
  resume: async (id: string): Promise<Automation> => must(readAutomation(await invoke<unknown>('automation_resume', { id })), 'The automation'),
  runNow: async (id: string): Promise<Run> => must(readRun(await invoke<unknown>('automation_run_now', { id })), 'The run'),
  decide: async (approvalId: string, chosen: number[]): Promise<RunDetail> =>
    must(readRunDetail(await invoke<unknown>('engine_approval_decide', { decision: { approvalId, chosen } })), 'The run'),
  setModel: (model: ModelRef | null) => invoke<unknown>('automations_model_set', { model }),
};
