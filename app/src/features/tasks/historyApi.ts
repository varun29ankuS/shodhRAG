/** The task history and issue key commands (`calendar_commands.rs`), by their literal names. */
import { invoke } from '@tauri-apps/api/core';
import type { TaskChange } from './types';

/** What changed on a task, oldest first. */
export function taskHistory(taskId: string): Promise<TaskChange[]> {
  return invoke<TaskChange[]>('task_history', { taskId });
}

/** The key a workspace's tasks are numbered under (`THE` of `THE-12`). */
export function issueKey(workspaceId: string): Promise<string> {
  return invoke<string>('task_issue_key_get', { workspaceId });
}

/** Changes the key of a workspace's new tasks; returns it as stored. */
export function setIssueKey(workspaceId: string, key: string): Promise<string> {
  return invoke<string>('task_issue_key_set', { workspaceId, key });
}

/** Links a task to another; a block that would make a task wait for itself is refused. */
export function linkTasks(taskId: string, kind: 'blocks' | 'relates' | 'duplicate_of', target: string): Promise<void> {
  return invoke<void>('task_link', { taskId, kind, target });
}

/** Deletes a project; its tasks stay in their workspace. Returns how many it had. */
export function deleteProject(projectId: string): Promise<number> {
  return invoke<number>('task_project_delete', { projectId });
}
