/**
 * Showing the archived workspaces from another view (the sidebar's "every chat is
 * in an archived workspace" state). The request is kept until the Workspaces view
 * takes it, so it works whether that view is mounted (event) or mounts after the
 * navigation (initial state), as `memory/navigation` does for Settings sections,
 * and expires if that view never shows (lib/oneShot). Unit-tested under Node.
 */
import { OneShot } from '../../lib/oneShot.ts';

export const SHOW_ARCHIVED_WORKSPACES = 'shodh:show-archived-workspaces';

const requested = new OneShot<true>();

/** Ask the Workspaces view to show its archived list. */
export function requestArchivedWorkspaces(target: Pick<EventTarget, 'dispatchEvent'> = window): void {
  requested.set(true);
  target.dispatchEvent(new Event(SHOW_ARCHIVED_WORKSPACES));
}

/**
 * Whether the archived list was asked for, without taking it: for the view's
 * first render, which may be rendered again before it shows (a deferred view
 * switch). The view takes it once mounted.
 */
export function peekArchivedRequest(): boolean {
  return requested.peek() !== null;
}

/** Whether the archived list was asked for, once. */
export function takeArchivedRequest(): boolean {
  return requested.take() !== null;
}
