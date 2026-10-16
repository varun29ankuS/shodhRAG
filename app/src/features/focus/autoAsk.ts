/**
 * Whether a focus level opened from a "go deeper" mark sends its prepared
 * question now: only on a fresh thread (a reopened one already has the
 * answer), and only once per level (`seq` is unique per opened level).
 *
 * Pure module, unit-tested with Node (`app/tests/focusAutoAsk.test.ts`).
 */
export function autoAskQuestion(
  level: { question: string | null; seq: number },
  turnCount: number,
  asked: ReadonlySet<number>,
): string | null {
  const question = level.question?.trim() ?? '';
  if (!question || turnCount > 0 || asked.has(level.seq)) return null;
  return question;
}
