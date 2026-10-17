// The edit: which shot plays between which bars, and what it says.
// Every shot starts on a bar (see beats.ts). The track is edited from two
// sections joined on matching bars (README), at bar 38: the second half,
// from Tasks on, rides the second peak, with three-bar shots instead of four.
import { barBeat, beatFrame } from "./beats";
import { FPS } from "./theme";

export type ShotId =
  | "hero"
  | "library"
  | "ask"
  | "answer"
  | "cite"
  | "ground"
  | "deeper"
  | "visual"
  | "graph"
  | "workspaces"
  | "tasks"
  | "create"
  | "palette"
  | "models"
  | "activity"
  | "close";

export type Shot = {
  id: ShotId;
  /** Bar the shot starts on (the first shot starts on the first frame). */
  bar: number;
  /** Chapter label above the headline. */
  label: string;
  /** The headline, one entry per line, revealed line by line as the shot lands. */
  lines: string[];
  /** One supporting sentence under the headline. */
  detail: string;
};

export const SHOTS: Shot[] = [
  { id: "hero", bar: 0, label: "Shodh", lines: ["Your papers,", "answered."], detail: "A research assistant that shows where every answer comes from." },
  { id: "library", bar: 4, label: "Library", lines: ["Start with", "your folder."], detail: "Add a folder of PDFs. It is indexed on your machine." },
  { id: "ask", bar: 8, label: "Ask", lines: ["Ask in", "plain words."], detail: "Questions about your papers, answered from them." },
  { id: "answer", bar: 12, label: "Answer", lines: ["Every claim,", "cited."], detail: "Each statement carries the source it came from." },
  { id: "cite", bar: 16, label: "Verify", lines: ["One click.", "The exact page."], detail: "The cited passage opens, outlined in the PDF." },
  { id: "ground", bar: 20, label: "Check", lines: ["Unsupported?", "It says so."], detail: "Every statement is checked against its passage." },
  { id: "deeper", bar: 24, label: "Explore", lines: ["Go deeper.", "Stay in place."], detail: "Select any phrase and ask beside the answer." },
  { id: "visual", bar: 28, label: "Visualize", lines: ["See it,", "don't just read it."], detail: "Diagrams drawn from the answer, with their sources." },
  { id: "graph", bar: 32, label: "Graph", lines: ["Who cites", "whom."], detail: "Your papers' citations, mapped from their own bibliographies." },
  { id: "workspaces", bar: 35, label: "Workspaces", lines: ["One project,", "one space."], detail: "Its own sources, instructions, memory and chats." },
  { id: "tasks", bar: 38, label: "Tasks", lines: ["Plans beside", "the research."], detail: "Tasks and events, in the same app as your papers." },
  { id: "create", bar: 41, label: "Create", lines: ["Everything,", "one shortcut."], detail: "Ctrl+Shift+N makes a chat, task or event from anywhere." },
  { id: "palette", bar: 44, label: "Command", lines: ["Anything,", "from the keyboard."], detail: "Ctrl+K finds every view, action, chat and folder." },
  { id: "models", bar: 47, label: "Models", lines: ["Your model,", "your call."], detail: "A subscription, your own key, or entirely on this computer." },
  { id: "activity", bar: 50, label: "Record", lines: ["Nothing", "off the record."], detail: "Every question, tool call and retrieval, in a tamper-evident log." },
  { id: "close", bar: 53, label: "Shodh", lines: ["Research you can verify."], detail: "Free and open source, for Windows and Mac." },
];

/** The video ends with the edited track (its last two bars fade out). */
export const END_FRAME = Math.round(107.03 * FPS);

export type Slot = Shot & { start: number; frames: number };

const startOf = (shot: Shot, i: number) => (i === 0 ? 0 : beatFrame(barBeat(shot.bar)));

/** Frame ranges for the shots, ending at `END_FRAME`. */
export function slots(shots: Shot[] = SHOTS): Slot[] {
  return shots.map((shot, i) => {
    const start = startOf(shot, i);
    const next = i + 1 < shots.length ? startOf(shots[i + 1], i + 1) : END_FRAME;
    return { ...shot, start, frames: next - start };
  });
}

/** The footage shots, in order (the close has none). */
export const FOOTAGE_SHOTS = SHOTS.filter((s) => s.id !== "close");

/** README loop: the answer, the citation and the grounding check, back to back. */
export const LOOP_SHOTS: ShotId[] = ["answer", "cite", "ground"];
