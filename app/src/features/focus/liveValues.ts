/**
 * Slider positions of the interactive visual shown in the pop-out, as the
 * reader has them now. A plot or simulation target carries the positions it
 * was opened with; the stage reports every change here (per pop-out level),
 * so Refine and "new version" start from what the reader is looking at
 * rather than from where the sliders were when the pop-out opened.
 *
 * Pure module, unit-tested with Node (`app/tests/focusLiveValues.test.ts`).
 */

import type { FocusParamValue, FocusTarget } from './focusTypes.ts';

const live = new Map<number, FocusParamValue[]>();
/** Levels kept; older entries go first (a pop-out holds a handful of levels). */
const MAX_LEVELS = 64;

function clean(values: readonly FocusParamValue[]): FocusParamValue[] {
  return values
    .filter(v => typeof v.name === 'string' && v.name && Number.isFinite(v.value))
    .map(v => ({ name: v.name, value: v.value }));
}

/** Records the current slider positions of pop-out level `seq`. */
export function setLiveValues(seq: number, values: readonly FocusParamValue[]): void {
  live.delete(seq);
  live.set(seq, clean(values));
  while (live.size > MAX_LEVELS) {
    const oldest = live.keys().next().value;
    if (oldest === undefined) break;
    live.delete(oldest);
  }
}

/** Forgets a level (its pop-out closed). */
export function clearLiveValues(seq: number): void {
  live.delete(seq);
}

/**
 * The slider positions to use for a level's target: what the reader has
 * now, else what the target was opened with. Empty for other kinds.
 */
export function currentValues(seq: number, target: FocusTarget): FocusParamValue[] {
  if (target.kind !== 'plot' && target.kind !== 'simulation') return [];
  const now = live.get(seq);
  // The stage reports every parameter of the spec it draws.
  return now && now.length > 0 ? now : target.values;
}
