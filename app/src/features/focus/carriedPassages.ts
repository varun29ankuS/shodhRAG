/**
 * The passages a side question carries from the answers it continues.
 *
 * A side thread's agent session is given the main conversation (up to the
 * answer the thread is anchored to) and the thread's earlier answers, which
 * cite passages as `[21]`, `[28]`. Each run numbers its own passages, so
 * without these the side answer would cite numbers its run never issued:
 * the grounding check reports them as sources that do not exist, and the
 * transcript cannot open them. The passages those answers cite are sent with
 * the question under the same numbers (`AnswerScope.passages`); the backend
 * starts the run with them, so the side answer's citations of them resolve
 * and its own searches number on after the highest. Every cited passage is
 * sent, also the ones the backend's limits leave out: their numbers stay
 * taken. The side transcript starts with the ones it keeps
 * (`keptCarriedPassages`).
 *
 * Pure module so it is unit-tested with Node (`app/tests/carriedPassages.test.ts`).
 */

import { citedNumbersIn } from '../agent/grounding.ts';
import type { Passage } from '../agent/reducer.ts';

/**
 * Limits of the passages one question carries, as the backend applies them
 * (`MAX_CARRIED_PASSAGES`, `MAX_CARRIED_CHARS` and `MAX_CARRIED_PASSAGE_CHARS`
 * in `app/src-tauri/src/agent_session_commands.rs`; a test keeps them equal).
 * Characters are counted as Rust counts them: code points.
 */
export const MAX_CARRIED_PASSAGES = 40;
export const MAX_CARRIED_CHARS = 24_000;
export const MAX_CARRIED_PASSAGE_CHARS = 4_000;

/** An answer a side question continues: its text and the passages of its run. */
export interface ContinuedAnswer {
  text: string;
  passages: readonly Passage[];
}

/** A carried passage as `agent_send` takes it (`SendScope.passages`). */
export interface CarriedPassageWire {
  n: number;
  file: string;
  path: string;
  page: string | null;
  text: string;
  web: boolean;
}

/**
 * The passages `answers` cite, each under its number: those cited in
 * `focus` (the text the question is about) first, then in the order the
 * answers cite them. A number is taken from the first answer that cites it
 * and has a passage for it (the anchored answer comes first). Passages no
 * answer cites are not carried.
 */
export function carriedPassages(answers: readonly ContinuedAnswer[], focus = ''): Passage[] {
  const byNumber = new Map<number, Passage>();
  const order: number[] = [];
  for (const answer of answers) {
    const own = new Map(answer.passages.map(p => [p.n, p]));
    for (const n of citedNumbersIn(answer.text)) {
      const passage = own.get(n);
      if (!passage || byNumber.has(n) || !passage.text.trim()) continue;
      byNumber.set(n, passage);
      order.push(n);
    }
  }
  const first = [...citedNumbersIn(focus)].filter(n => byNumber.has(n));
  const ranked = [...first, ...order.filter(n => !first.includes(n))];
  return ranked.map(n => byNumber.get(n) as Passage);
}

/** `text` cut to `max` code points, the last one an ellipsis (as the backend cuts). */
function truncateChars(text: string, max: number): string {
  const chars = Array.from(text);
  if (chars.length <= max) return text;
  return `${chars.slice(0, Math.max(0, max - 1)).join('')}…`;
}

/**
 * The carried passages the side run holds: the backend's rules applied to
 * `passages` (in the order sent). Each text is trimmed and cut to
 * `MAX_CARRIED_PASSAGE_CHARS`; a blank text or a repeated number is skipped;
 * nothing is kept from the first passage that would pass
 * `MAX_CARRIED_PASSAGES` or `MAX_CARRIED_CHARS` on. The side transcript starts
 * with these, so it never shows a passage the model was not given.
 */
export function keptCarriedPassages(passages: readonly Passage[]): Passage[] {
  const kept: Passage[] = [];
  let used = 0;
  for (const passage of passages) {
    const text = truncateChars(passage.text.trim(), MAX_CARRIED_PASSAGE_CHARS);
    if (!text || kept.some(k => k.n === passage.n)) continue;
    const size = Array.from(text).length;
    if (kept.length === MAX_CARRIED_PASSAGES || used + size > MAX_CARRIED_CHARS) break;
    used += size;
    kept.push(text === passage.text ? passage : { ...passage, text });
  }
  return kept;
}

/** `passages` in the form `agent_send` takes. */
export function toWire(passages: readonly Passage[]): CarriedPassageWire[] {
  return passages.map(p => ({
    n: p.n,
    file: p.file,
    path: p.path,
    page: p.page,
    text: p.text,
    web: p.web === true,
  }));
}
