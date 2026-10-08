import { FPS } from "./theme";

export type SceneId = 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10;

export type Orientation = "landscape" | "portrait";

/** Scene lengths in seconds, per the approved storyboard (75 s in all). */
export const SCENE_SECONDS: Record<SceneId, number> = {
  1: 6,
  2: 6,
  3: 4,
  4: 12,
  5: 8,
  6: 8,
  7: 10,
  8: 8,
  9: 8,
  10: 5,
};

export type Slot = { scene: SceneId; from: number; durationInFrames: number };

/** Consecutive slots (in frames) for the given scenes and lengths. */
export function slots(scenes: SceneId[], seconds: Partial<Record<SceneId, number>> = {}): Slot[] {
  let from = 0;
  return scenes.map((scene) => {
    const durationInFrames = Math.round((seconds[scene] ?? SCENE_SECONDS[scene]) * FPS);
    const slot = { scene, from, durationInFrames };
    from += durationInFrames;
    return slot;
  });
}

export const totalFrames = (list: Slot[]): number =>
  list.reduce((sum, s) => sum + s.durationInFrames, 0);

export const LAUNCH: SceneId[] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
/** Vertical cut: scene 8 dropped. */
export const VERTICAL: SceneId[] = [1, 2, 3, 4, 5, 6, 7, 9, 10];
/** README loop: scenes 4 to 6, condensed. */
export const LOOP: SceneId[] = [4, 5, 6];
export const LOOP_SECONDS: Partial<Record<SceneId, number>> = { 4: 5, 5: 5, 6: 5 };

export const CAPTIONS: Record<SceneId, string> = {
  1: "Research moves faster than anyone can read.",
  2: "And most AI answers sound sure — even when they aren't.",
  3: "Shodh. Research you can verify.",
  4: "Ask across your papers. Every claim comes with its source.",
  5: "One click takes you to the exact page.",
  6: "And when a claim isn't supported, Shodh tells you.",
  7: "Select anything to go deeper — without losing your place.",
  8: "Keep each project in its own workspace, with its own sources and memory.",
  9: "Your library stays on your machine. Only the passages an answer needs go to the model you choose — or nowhere, if you run locally.",
  10: "Shodh. Free, open source. For Windows and Mac.",
};
