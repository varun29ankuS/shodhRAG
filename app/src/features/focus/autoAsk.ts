/**
 * Whether a focus level opened from a "go deeper" mark sends its prepared
 * question now: unless its thread already asked it (a reopened thread holds
 * that answer; "Go deeper" and "Ask about this" on one node share a thread, so
 * other questions may be there too), and only once per level (`seq` is unique
 * per opened level).
 *
 * Pure module, unit-tested with Node (`app/tests/focusAutoAsk.test.ts`).
 */

function normalized(text: string): string {
  return text.replace(/\s+/g, ' ').trim();
}

/**
 * @param askedQuestions The thread's existing user questions, as typed.
 * @param asked Levels (by `seq`) that already sent their question.
 */
export function autoAskQuestion(
  level: { question: string | null; seq: number },
  askedQuestions: readonly string[],
  asked: ReadonlySet<number>,
): string | null {
  const question = level.question?.trim() ?? '';
  if (!question || asked.has(level.seq)) return null;
  const wanted = normalized(question);
  if (askedQuestions.some(q => normalized(q) === wanted)) return null;
  return question;
}
