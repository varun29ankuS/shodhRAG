// The edit: which shot plays between which beats, and what it says.
// Shots cut on bar starts (beats 3, 7, 11, …; see beats.ts). The close lands
// on beat 79 and the video ends one bar later, as the music's first section
// ends (the track's energy drops at 40 s).
import { beatFrame } from "./beats";
import { FPS } from "./theme";

export type ShotId = "ask" | "answer" | "cite" | "ground" | "deeper" | "create" | "library" | "close";

export type Shot = {
  id: ShotId;
  /** Beat index the shot starts on (0 means the first frame); it runs to the next shot. */
  from: number;
  /** A few words, shown word by word as the shot lands. */
  caption: string;
};

export const SHOTS: Shot[] = [
  { id: "ask", from: 0, caption: "Ask your papers anything." },
  { id: "answer", from: 11, caption: "Every claim, cited." },
  { id: "cite", from: 19, caption: "One click. The exact page." },
  { id: "ground", from: 27, caption: "Not in the source? It says so." },
  { id: "deeper", from: 35, caption: "Go deeper without losing your place." },
  { id: "create", from: 47, caption: "Chats, tasks, events. One shortcut." },
  { id: "library", from: 59, caption: "Your library stays on your machine." },
  { id: "close", from: 67, caption: "Research you can verify." },
];

/** The video ends a bar after the close's final hit (beat 79). */
export const END_FRAME = Math.round(40 * FPS);

export type Slot = Shot & { start: number; frames: number };

/** Frame ranges for the shots, ending at `END_FRAME`. */
export function slots(shots: Shot[] = SHOTS): Slot[] {
  return shots.map((shot, i) => {
    const start = shot.from === 0 ? 0 : beatFrame(shot.from);
    const next = i + 1 < shots.length ? beatFrame(shots[i + 1].from) : END_FRAME;
    return { ...shot, start, frames: next - start };
  });
}

/** README loop: the answer, the citation and the grounding check, back to back. */
export const LOOP_SHOTS: ShotId[] = ["answer", "cite", "ground"];
