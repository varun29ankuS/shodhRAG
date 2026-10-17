// The music's beat map (src/beats.json, from scripts/beats.py): a constant
// tempo grid fitted to the track's onsets. Every cut and reveal is placed on
// one of these beats.
import map from "./beats.json";
import { FPS } from "./theme";

export const BEATS: readonly number[] = map.beats;

/** Beats `BAR_PHASE`, `BAR_PHASE + 4`, … start a bar (where the track's sections change). */
export const BAR_PHASE: number = map.barPhase;

/** The beat index that starts bar `bar` (bar 0 is the first full bar). */
export const barBeat = (bar: number): number => BAR_PHASE + 4 * bar;

/** The frame a beat falls on. */
export function beatFrame(index: number): number {
  const t = BEATS[index];
  if (t === undefined) {
    throw new Error(`The beat map has no beat ${index} (it has ${BEATS.length})`);
  }
  return Math.round(t * FPS);
}
