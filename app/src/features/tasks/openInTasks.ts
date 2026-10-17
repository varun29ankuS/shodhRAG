import { publishTarget } from '../agent/navigation';

/**
 * Show a task or event in Tasks with its sheet open, from any view: the
 * Tasks store applies the calendar target once the view is shown.
 */
export function openInTasks(target: { taskId: string } | { eventId: string }): void {
  publishTarget({
    kind: 'calendar',
    date: null,
    taskId: 'taskId' in target ? target.taskId : null,
    eventId: 'eventId' in target ? target.eventId : null,
  });
  window.dispatchEvent(new CustomEvent('switchTab', { detail: 'tasks' }));
}
