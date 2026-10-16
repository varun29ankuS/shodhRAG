// The music's beat map (src/beats.json, from scripts/beats.py). Every cut,
// caption and pulse is placed on a detected beat, not on a fixed grid: the
// tracker's beats wander a few milliseconds and a grid drifts off them.
import map from "./beats.json";
import { FPS } from "./theme";

export const BEATS: readonly number[] = map.beats;

/** Beats 3, 7, 11, … start a bar: they carry the strongest kick and the chord changes. */
export const BAR_PHASE = 3;

/** The frame a beat falls on. */
export function beatFrame(index: number): number {
  const t = BEATS[index];
  if (t === undefined) {
    throw new Error(`The beat map has no beat ${index} (it has ${BEATS.length})`);
  }
  return Math.round(t * FPS);
}

const BEAT_FRAMES = BEATS.map((t) => Math.round(t * FPS));

/** Frames since the last beat at or before `frame`, and that beat's index (-1 before the first). */
export function sinceBeat(frame: number): { index: number; frames: number } {
  let lo = 0;
  let hi = BEAT_FRAMES.length - 1;
  if (frame < BEAT_FRAMES[0]) return { index: -1, frames: Infinity };
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (BEAT_FRAMES[mid] <= frame) lo = mid;
    else hi = mid - 1;
  }
  return { index: lo, frames: frame - BEAT_FRAMES[lo] };
}

/** A decaying 1 → 0 hit on every beat (bar starts hit harder), for pulses. */
export function pulse(frame: number, decayFrames = 9): number {
  const { index, frames } = sinceBeat(frame);
  if (index < 0) return 0;
  const strength = index % 4 === BAR_PHASE ? 1 : 0.45;
  return strength * Math.exp(-frames / (decayFrames / 3));
}
