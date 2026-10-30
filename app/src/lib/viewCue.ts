/**
 * The cue shown when the assistant changes the view ("Shodh opened Library"):
 * only for a navigation the assistant made (its `navigated` events carry a
 * sequence number), only once per navigation, and only when the view really
 * changed, so a user's own clicks and a navigation to the view already open
 * show nothing.
 *
 * The navigation and the view switch it causes may reach the shell in one
 * render or in two (the sequence first, the view a render later): a
 * navigation is kept pending until the view it asked for is open, and
 * dropped if the user opens another view first.
 *
 * No imports: unit-tested under Node (`app/tests/viewCue.test.ts`).
 */

/** How long the cue is shown, from when the view it names is on screen (it fades over the last part). */
export const VIEW_CUE_MS = 2800;

export interface CueInput<V extends string> {
  /** Sequence number of the assistant's latest navigation (0: none yet). */
  seq: number;
  /** The view that navigation asked for. */
  view: V | null;
  /** The view asked for now (the sidebar's current item). */
  tab: V;
}

export interface CueTracker<V extends string> {
  /** The latest navigation already looked at. */
  seq: number;
  /** The view asked for when the tracker last looked. */
  tab: V;
  /** A navigation whose view is not asked for yet, with the view it left. */
  pending: { view: V; from: V } | null;
}

export function initialCueTracker<V extends string>(input: CueInput<V>): CueTracker<V> {
  return { seq: input.seq, tab: input.tab, pending: null };
}

/** Look at the current state: the next tracker, and the view to cue (or `null`). */
export function nextCue<V extends string>(
  tracker: CueTracker<V>,
  input: CueInput<V>,
): { tracker: CueTracker<V>; cue: V | null } {
  let pending = tracker.pending;
  if (input.seq > tracker.seq) {
    pending = input.view === null ? null : { view: input.view, from: tracker.tab };
  } else if (pending && input.tab !== tracker.tab && input.tab !== pending.view) {
    // The user opened another view before the asked-for one: nothing to cue.
    pending = null;
  }
  let cue: V | null = null;
  if (pending && input.tab === pending.view) {
    if (pending.from !== pending.view) cue = pending.view;
    pending = null;
  }
  return { tracker: { seq: Math.max(tracker.seq, input.seq), tab: input.tab, pending }, cue };
}

/** The cue to show now: only while the view it names is the one on screen. */
export function visibleCue<C extends { view: string }>(cue: C | null, shown: string): C | null {
  return cue && cue.view === shown ? cue : null;
}

/** What the cue says, for screen readers in full. */
export function cueAnnouncement(label: string): string {
  return `Shodh opened ${label}`;
}
