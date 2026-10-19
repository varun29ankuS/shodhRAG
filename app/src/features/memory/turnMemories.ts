/**
 * What the chat shows under each turn about memory: "Remembered: “…” · Undo" under a
 * message that asked Shodh to remember something, "Learned: “…” · Undo" under an answer
 * the learner saved memories from, and "Shodh suggests remembering “…” · Review" for what
 * waits for approval. Built from the turn's suggestions as the backend has them now
 * (`memory_suggestions_for_turns`): the conversation keeps no memory's words, so an undone
 * or forgotten memory shows as such, without its text. Pure functions only, so they run
 * under `node --test` without React or Tauri.
 */
import type { Suggestion } from './suggestions.ts';

/** Most turns asked about at once (the backend's `MAX_TURNS_LISTED`). */
export const MAX_TURNS = 200;

/** Lines shown one by one under an answer; more are summed up ("Learned 3 things · Show"). */
export const MAX_LINES = 2;

export type TurnMemoryKind = 'remembered' | 'learned' | 'suggested' | 'undone' | 'forgotten';

/** One memory of a turn, as the chat shows it. */
export interface TurnMemoryLine {
  /** The suggestion (Undo and Review act on it). */
  id: string;
  kind: TurnMemoryKind;
  /** The user asked for it (shown under their message), else the learner found it. */
  asked: boolean;
  /** The memory's words, read live; empty for undone and forgotten memories. */
  text: string;
  /** Undo would remove it. */
  undoable: boolean;
}

/** The memory lines of one turn. */
export interface TurnMemories {
  /** Under the user's message: what they asked Shodh to remember. */
  asked: TurnMemoryLine[];
  /** Under the answer: what was learned from the turn, or suggested from it. */
  learned: TurnMemoryLine[];
}

/** The line of one suggestion, or `null` when the chat shows nothing for it. */
export function lineOf(s: Suggestion): TurnMemoryLine | null {
  if (s.action.kind !== 'remember' || s.origin !== 'turn') return null;
  const base = { id: s.id, asked: s.action.by_user, text: '', undoable: false };
  switch (s.status) {
    case 'accepted':
    case 'learned':
      // Its words were erased with the memory.
      if (s.forgotten) return { ...base, kind: 'forgotten' };
      // A reinforcement, or words already remembered: nothing was saved, nothing to undo.
      if (!s.undoable) return null;
      return { ...base, kind: s.status === 'learned' ? 'learned' : 'remembered', text: s.action.text, undoable: true };
    case 'pending':
    case 'failed':
      return s.forgotten ? null : { ...base, kind: 'suggested', text: s.action.text };
    case 'undone':
      return { ...base, kind: 'undone' };
    default:
      // Rejected or stale: the user said no, or it no longer applies.
      return null;
  }
}

/** The lines of each turn (by run id), in the order the suggestions came. */
export function groupByTurn(suggestions: readonly Suggestion[]): Map<string, TurnMemories> {
  const turns = new Map<string, TurnMemories>();
  for (const s of suggestions) {
    if (s.turnId === null) continue;
    const line = lineOf(s);
    if (!line) continue;
    let turn = turns.get(s.turnId);
    if (!turn) {
      turn = { asked: [], learned: [] };
      turns.set(s.turnId, turn);
    }
    (line.asked ? turn.asked : turn.learned).push(line);
  }
  return turns;
}

function sameLines(a: readonly TurnMemoryLine[], b: readonly TurnMemoryLine[]): boolean {
  return (
    a.length === b.length &&
    a.every((x, i) => {
      const y = b[i];
      return x.id === y.id && x.kind === y.kind && x.asked === y.asked && x.text === y.text && x.undoable === y.undoable;
    })
  );
}

/**
 * `next`, keeping the entries of `previous` that did not change, so answers whose lines
 * stayed the same are not re-rendered.
 */
export function withStableTurns(
  previous: ReadonlyMap<string, TurnMemories>,
  next: Map<string, TurnMemories>,
): Map<string, TurnMemories> {
  for (const [turn, memories] of next) {
    const before = previous.get(turn);
    if (before && sameLines(before.asked, memories.asked) && sameLines(before.learned, memories.learned)) {
      next.set(turn, before);
    }
  }
  return next;
}

/** One line in words: what happened, the memory's words (if shown) and what can be done. */
export interface LineText {
  /** "Remembered", "Learned · Undone", "Remembered: a forgotten memory", ... */
  lead: string;
  /** The memory's words, shown in quotes after the lead. */
  quote: string | null;
  action: 'undo' | 'review' | null;
}

export function lineText(line: TurnMemoryLine): LineText {
  const saved = line.asked ? 'Remembered' : 'Learned';
  switch (line.kind) {
    case 'remembered':
      return { lead: 'Remembered', quote: line.text, action: line.undoable ? 'undo' : null };
    case 'learned':
      return { lead: 'Learned', quote: line.text, action: line.undoable ? 'undo' : null };
    case 'suggested':
      return { lead: 'Shodh suggests remembering', quote: line.text, action: 'review' };
    case 'undone':
      return { lead: `${saved} · Undone`, quote: null, action: null };
    case 'forgotten':
      return { lead: `${saved}: a forgotten memory`, quote: null, action: null };
  }
}

/** "Learned 3 things", "Learned 2 things, 1 to review": several lines summed up. */
export function summaryText(lines: readonly TurnMemoryLine[]): string {
  const learned = lines.filter(l => l.kind !== 'suggested').length;
  const suggested = lines.length - learned;
  const things = (n: number) => `${n} ${n === 1 ? 'thing' : 'things'}`;
  if (learned === 0) return `Shodh suggests remembering ${things(suggested)}`;
  return suggested === 0 ? `Learned ${things(learned)}` : `Learned ${things(learned)}, ${suggested} to review`;
}

/** The run ids of a conversation's answers, newest last, at most [`MAX_TURNS`]. */
export function turnIdsOf(runIds: readonly (string | null | undefined)[]): string[] {
  const ids = runIds.filter((id): id is string => typeof id === 'string' && id.length > 0);
  return [...new Set(ids)].slice(-MAX_TURNS);
}

/** The turns a suggestions change was about, when it named them. */
export interface TurnsChanged {
  conversationId: string;
  turnIds: string[];
}

/** The turns a `memory-suggestions-changed` payload names, or `null` when it names none. */
export function turnsChanged(payload: unknown): TurnsChanged | null {
  if (typeof payload !== 'object' || payload === null) return null;
  const p = payload as { conversationId?: unknown; turnIds?: unknown };
  if (typeof p.conversationId !== 'string' || !Array.isArray(p.turnIds)) return null;
  return { conversationId: p.conversationId, turnIds: p.turnIds.filter((t): t is string => typeof t === 'string') };
}

/** Whether a change (`turns`, `null` when it named none) may change what `conversationId`'s turns show. */
export function concernsTurns(turns: TurnsChanged | null, conversationId: string, shown: readonly string[]): boolean {
  if (turns === null) return true;
  return turns.conversationId === conversationId && turns.turnIds.some(t => shown.includes(t));
}
