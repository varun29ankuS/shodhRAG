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
 * and its own searches number on after the highest.
 *
 * Pure module so it is unit-tested with Node (`app/tests/carriedPassages.test.ts`).
 */

import { citedNumbersIn } from '../agent/grounding.ts';
import type { Passage } from '../agent/reducer.ts';

/** Most passages carried into one question (the backend enforces the same). */
export const MAX_CARRIED_PASSAGES = 40;

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
  return ranked.slice(0, MAX_CARRIED_PASSAGES).map(n => byNumber.get(n) as Passage);
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
