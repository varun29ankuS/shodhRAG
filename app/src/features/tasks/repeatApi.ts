/**
 * The `task_repeat_*` commands (`task_repeat.rs`). Literal command names: the agent
 * coverage and window-grant gates read every one.
 */
import { invoke } from '@tauri-apps/api/core';
import type { RepeatChoice, RepeatRun, RepeatView, Schedule } from './repeat';

/** Emitted when a series, its runs or its question changed. */
export const REPEAT_CHANGED_EVENT = 'task-repeat-changed';

export function repeatGet(taskId: string): Promise<RepeatView | null> {
  return invoke<RepeatView | null>('task_repeat_get', { taskId });
}

export function repeatSet(taskId: string, schedule: Schedule): Promise<RepeatView> {
  return invoke<RepeatView>('task_repeat_set', { taskId, schedule });
}

export function repeatClear(taskId: string): Promise<boolean> {
  return invoke<boolean>('task_repeat_clear', { taskId });
}

export function repeatRuns(taskId: string, limit?: number): Promise<RepeatRun[]> {
  return invoke<RepeatRun[]>('task_repeat_runs', { taskId, limit: limit ?? null });
}

export function repeatDecide(jobId: string, choice: RepeatChoice): Promise<void> {
  return invoke<void>('task_repeat_decide', { jobId, choice });
}
