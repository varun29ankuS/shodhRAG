/**
 * Showing the archived workspaces from another view (the sidebar's "every chat is
 * in an archived workspace" state). The request is kept until the Workspaces view
 * takes it, so it works whether that view is mounted (event) or mounts after the
 * navigation (initial state), as `memory/navigation` does for Settings sections.
 * No imports: unit-tested under Node.
 */

export const SHOW_ARCHIVED_WORKSPACES = 'shodh:show-archived-workspaces';

let requested = false;

/** Ask the Workspaces view to show its archived list. */
export function requestArchivedWorkspaces(target: Pick<EventTarget, 'dispatchEvent'> = window): void {
  requested = true;
  target.dispatchEvent(new Event(SHOW_ARCHIVED_WORKSPACES));
}

/**
 * Whether the archived list was asked for, without taking it: for the view's
 * first render, which may be rendered again before it shows (a deferred view
 * switch). The view takes it once mounted.
 */
export function peekArchivedRequest(): boolean {
  return requested;
}

/** Whether the archived list was asked for, once. */
export function takeArchivedRequest(): boolean {
  const was = requested;
  requested = false;
  return was;
}
