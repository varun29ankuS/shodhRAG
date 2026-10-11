// Reads public/footage.json: which files each footage scene shows and how
// the camera moves over them. Swapping footage needs no code change.
import { interpolate } from "remotion";
import manifest from "../public/footage.json";
import { calm } from "./theme";

export type FocusRect = { x: number; y: number; w: number; h: number };
export type Keyframe = { at: number; scale: number; focus: FocusRect };
export type Sequence = { pattern: string; count: number; start?: number; fps?: number };

export type FootageEntry = {
  label: string;
  files: string[];
  sequence?: Sequence;
  aspect?: number;
  keyframes: Keyframe[];
};

export type Media =
  | { kind: "placeholder" }
  | { kind: "video"; src: string }
  | { kind: "stills"; srcs: string[] }
  | { kind: "frames"; srcs: string[]; fps: number };

const VIDEO = /\.(mp4|webm|mov|mkv)$/i;
const DEFAULT_ASPECT = 16 / 9;

export function footageFor(scene: number): FootageEntry {
  const scenes = manifest.scenes as Record<string, FootageEntry>;
  const entry = scenes[String(scene)];
  if (!entry) {
    throw new Error(`public/footage.json has no entry for scene ${scene}`);
  }
  if (entry.keyframes.length === 0) {
    throw new Error(`public/footage.json scene ${scene} needs at least one keyframe`);
  }
  return entry;
}

export const aspectOf = (entry: FootageEntry): number => entry.aspect ?? DEFAULT_ASPECT;

/** Expand `frame-%04d.png` with `count` frames from `start`. */
export function expandSequence(seq: Sequence): string[] {
  const match = /%0(\d+)d/.exec(seq.pattern);
  if (!match) {
    throw new Error(`Sequence pattern needs a %0Nd placeholder: ${seq.pattern}`);
  }
  const digits = Number(match[1]);
  const start = seq.start ?? 1;
  return Array.from({ length: seq.count }, (_, i) =>
    seq.pattern.replace(match[0], String(start + i).padStart(digits, "0")),
  );
}

export function mediaOf(scene: number, entry: FootageEntry): Media {
  const path = (file: string) => `footage/${scene}/${file}`;
  if (entry.sequence && entry.sequence.count > 0) {
    return {
      kind: "frames",
      srcs: expandSequence(entry.sequence).map(path),
      fps: entry.sequence.fps ?? 30,
    };
  }
  if (entry.files.length === 0) return { kind: "placeholder" };
  const video = entry.files.find((f) => VIDEO.test(f));
  if (video) return { kind: "video", src: path(video) };
  return { kind: "stills", srcs: entry.files.map(path) };
}

/** Camera at `progress` (0..1 of the scene): eased between keyframes. */
export function cameraAt(keyframes: Keyframe[], progress: number): { scale: number; cx: number; cy: number } {
  const sorted = [...keyframes].sort((a, b) => a.at - b.at);
  const centre = (k: Keyframe) => ({ cx: k.focus.x + k.focus.w / 2, cy: k.focus.y + k.focus.h / 2 });
  const first = sorted[0];
  const last = sorted[sorted.length - 1];
  if (sorted.length === 1 || progress <= first.at) return { scale: first.scale, ...centre(first) };
  if (progress >= last.at) return { scale: last.scale, ...centre(last) };
  const i = sorted.findIndex((k) => k.at > progress);
  const a = sorted[i - 1];
  const b = sorted[i];
  const mix = (from: number, to: number) =>
    interpolate(progress, [a.at, b.at], [from, to], { easing: calm });
  const ca = centre(a);
  const cb = centre(b);
  return { scale: mix(a.scale, b.scale), cx: mix(ca.cx, cb.cx), cy: mix(ca.cy, cb.cy) };
}

