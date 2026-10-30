/**
 * The cue shown when the assistant changes the view ("Shodh opened Library"):
 * only for a navigation the assistant made (its `navigated` events carry a
 * sequence number), only once per navigation, and only when the view really
 * changed, so a user's own clicks and a navigation to the view already open
 * show nothing.
 *
 * No imports: unit-tested under Node (`app/tests/viewCue.test.ts`).
 */

/** How long the cue stays before it is gone (it fades over the last part). */
export const VIEW_CUE_MS = 2800;

export interface CueInput<V extends string> {
  /** Sequence number of the assistant's latest navigation (0: none yet). */
  seq: number;
  /** The view that navigation asked for. */
  view: V | null;
  /** The view open now. */
  tab: V;
}

export interface CueTracker<V extends string> {
  /** The latest navigation already looked at. */
  seq: number;
  /** The view open when the tracker last looked. */
  tab: V;
}

export function initialCueTracker<V extends string>(input: CueInput<V>): CueTracker<V> {
  return { seq: input.seq, tab: input.tab };
}

/**
 * Look at the current state: the next tracker, and the view to cue (or
 * `null`). A navigation is looked at once, whether or not it cued.
 */
export function nextCue<V extends string>(
  tracker: CueTracker<V>,
  input: CueInput<V>,
): { tracker: CueTracker<V>; cue: V | null } {
  const fresh = input.seq > tracker.seq;
  const cue = fresh && input.view === input.tab && tracker.tab !== input.tab ? input.tab : null;
  return { tracker: { seq: Math.max(tracker.seq, input.seq), tab: input.tab }, cue };
}

/** What the cue says, for screen readers in full. */
export function cueAnnouncement(label: string): string {
  return `Shodh opened ${label}`;
}
